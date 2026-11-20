//! Shared MoQ media plumbing for live-call handlers.
//!
//! Extracted from the GPT-Live handler so a second handler (the cascade voice
//! agent) can reuse the transport without duplicating it: parse a
//! `IDFON-LIVE/1` invite, subscribe the caller's broadcast as 20 ms PCM frames,
//! publish the return leg, and sign the return-leg invite.
//!
//! Deliberately provider-neutral: this crate knows nothing about GPT-Live or
//! the voice engines. It is the media plane only.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use idfon_core::IdentityKey;
use idfon_core::transport::{IrohTransport, MessageTransport};
use idfon_protocol::MessageContent;
use iroh::protocol::Router;
use iroh::EndpointAddr;
use iroh_live::layout::LiveLayout;
use iroh_live::media::{AudioFormat, AudioSource, LocalBroadcast, SubscriptionExt as _};
use iroh_live::{EndpointConfig, Node, NodeConfig};
use moq_audio::{encode::Codec as AudioCodec, Format, Frame as AudioFrame};
/// The broadcast ticket an `IDFON-LIVE/1` invite carries (`ilv1:...`, a peer
/// plus its broadcast path). Kept under the old name for callers.
pub use iroh_live::BroadcastTicket as LiveTicket;
use tokio::sync::mpsc;

/// 20 ms of 24 kHz mono PCM — the call plane's frame.
pub const CHUNK_SAMPLES: usize = 480;
/// Frame duration in milliseconds.
pub const CHUNK_MS: u64 = 20;
/// 500 ms at 24 kHz — the caller-input queue bound.
pub const MAX_INPUT_BUFFER: usize = 12_000;
const MAX_QUEUE_SAMPLES: usize = 240_000; // ~10 s; drop oldest past this

/// Caller audio codec/rate advertised on the invite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioProfile {
    pub codec: AudioCodec,
    pub sample_rate: u32,
}

impl Default for AudioProfile {
    fn default() -> Self {
        Self {
            codec: AudioCodec::Opus,
            sample_rate: 48_000,
        }
    }
}

/// Parse the `audio_codec`/`audio_sample_rate` fields; absent = the legacy
/// Opus/48 kHz default.
pub fn parse_audio_profile(codec: Option<&str>, sample_rate: Option<u32>) -> Result<AudioProfile> {
    match (codec, sample_rate) {
        (None, None) => Ok(AudioProfile::default()),
        (Some("opus"), Some(48_000)) => Ok(AudioProfile {
            codec: AudioCodec::Opus,
            sample_rate: 48_000,
        }),
        // Opus on the 24 kHz agent plane: the caller's per-contact choice
        // (docs/voice-agent-modes.md). Same rate the programme/TTS queue
        // produces, so nothing has to resample.
        (Some("opus"), Some(24_000)) => Ok(AudioProfile {
            codec: AudioCodec::Opus,
            sample_rate: 24_000,
        }),
        (Some("pcm"), Some(24_000)) => Ok(AudioProfile {
            codec: AudioCodec::Pcm,
            sample_rate: 24_000,
        }),
        // LiquidCodec needs a registered
        // implementation; a holder without one refuses the call up front.
        (Some("liquid"), Some(24_000)) => {
            anyhow::ensure!(
                moq_audio::external::liquid_available(),
                "liquid caller audio needs a LiquidCodec implementation, and this holder has none"
            );
            Ok(AudioProfile {
                codec: AudioCodec::Liquid,
                sample_rate: 24_000,
            })
        }
        _ => {
            anyhow::bail!("unsupported caller audio profile: codec={codec:?} rate={sample_rate:?}")
        }
    }
}

/// Parsed `IDFON-LIVE/1` control.
#[derive(Debug, Clone)]
pub struct LiveInvite {
    pub is_start: bool,
    pub is_stop: bool,
    pub ticket: Option<LiveTicket>,
    pub return_addr: Option<EndpointAddr>,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    /// Chosen STT option id from the holder's `idfon.json` catalog.
    pub stt: Option<String>,
    /// Chosen TTS option id from the holder's `idfon.json` catalog.
    pub tts: Option<String>,
    /// Caller-pushed per-turn context (base64 UTF-8), e.g. the active
    /// per-contact speech settings. Untrusted data, never instructions.
    pub context: Option<String>,
    /// Per-call ownership override for STT: `client` or `server`. The caller
    /// sets `client` when it runs that half on-device (a per-contact hybrid),
    /// so the holder does not also run it.
    pub stt_side: Option<String>,
    /// Per-call ownership override for TTS (`client` / `server`).
    pub tts_side: Option<String>,
}

/// Parse a live control, or `None` for non-live text.
pub fn parse_invite(text: &str) -> Option<LiveInvite> {
    let body = text.strip_prefix("IDFON-LIVE/1\naction=")?;
    let (action, rest) = body.split_once('\n').unwrap_or((body, ""));
    let ticket = rest
        .lines()
        .find_map(|line| line.strip_prefix("ticket="))
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse().ok());
    let return_addr = rest
        .lines()
        .find_map(|line| line.strip_prefix("return_addr="))
        .and_then(|value| BASE64.decode(value).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let audio_codec = rest
        .lines()
        .find_map(|line| line.strip_prefix("audio_codec="))
        .map(str::to_owned);
    let audio_sample_rate = rest
        .lines()
        .find_map(|line| line.strip_prefix("audio_sample_rate="))
        .and_then(|value| value.parse().ok());
    let stt = rest
        .lines()
        .find_map(|line| line.strip_prefix("stt="))
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let tts = rest
        .lines()
        .find_map(|line| line.strip_prefix("tts="))
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let context = rest
        .lines()
        .find_map(|line| line.strip_prefix("context_b64="))
        .filter(|value| !value.is_empty())
        .and_then(|value| BASE64.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let side = |name: &str| {
        rest.lines()
            .find_map(|line| line.strip_prefix(name))
            .filter(|value| *value == "client" || *value == "server")
            .map(str::to_owned)
    };
    Some(LiveInvite {
        is_start: action == "start",
        is_stop: action == "stop",
        ticket,
        return_addr,
        audio_codec,
        audio_sample_rate,
        stt,
        tts,
        context,
        stt_side: side("stt_side="),
        tts_side: side("tts_side="),
    })
}

/// Bounded, thread-safe PCM hand-off into the return-leg encoder.
#[derive(Clone, Default)]
pub struct AudioQueue {
    samples: Arc<Mutex<VecDeque<i16>>>,
    trailing_byte: Arc<Mutex<Option<u8>>>,
}

impl AudioQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append s16le bytes (odd byte carried across calls).
    pub fn push_bytes(&self, bytes: &[u8]) {
        let Ok(mut samples) = self.samples.lock() else {
            return;
        };
        let mut trailing = self.trailing_byte.lock().map(|mut t| t.take()).unwrap_or(None);
        let mut offset = 0;
        if let Some(previous) = trailing.take() {
            if let Some(&first) = bytes.first() {
                samples.push_back(i16::from_le_bytes([previous, first]));
                offset = 1;
            } else {
                trailing = Some(previous);
            }
        }
        while offset + 1 < bytes.len() {
            samples.push_back(i16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
            offset += 2;
        }
        if offset < bytes.len() {
            trailing = Some(bytes[offset]);
        }
        if let Ok(mut guard) = self.trailing_byte.lock() {
            *guard = trailing;
        }
        while samples.len() > MAX_QUEUE_SAMPLES {
            samples.pop_front();
        }
    }

    /// Append interleaved s16 samples.
    pub fn push_samples(&self, pcm: &[i16]) {
        let Ok(mut samples) = self.samples.lock() else {
            return;
        };
        samples.extend(pcm.iter().copied());
        while samples.len() > MAX_QUEUE_SAMPLES {
            samples.pop_front();
        }
    }

    /// Drop everything still queued (barge-in: stop playback now).
    pub fn clear(&self) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }
        if let Ok(mut trailing) = self.trailing_byte.lock() {
            *trailing = None;
        }
    }

    /// Whether the return-leg queue has drained (playback finished).
    pub fn is_empty(&self) -> bool {
        self.samples.lock().map(|samples| samples.is_empty()).unwrap_or(true)
    }

    /// Samples waiting for the encoder; a source tops up against this rather
    /// than a wall clock, so it cannot drift from the publisher's cadence.
    pub fn queued_samples(&self) -> usize {
        self.samples.lock().map(|samples| samples.len()).unwrap_or(0)
    }

    fn fill(&self, output: &mut [i16]) -> usize {
        let Ok(mut samples) = self.samples.lock() else {
            output.fill(0);
            return output.len();
        };
        fill_audio_frame(&mut samples, output)
    }
}

/// Fill `output` from the queue, zero-padding the shortfall. Returns missing.
pub fn fill_audio_frame(queue: &mut VecDeque<i16>, output: &mut [i16]) -> usize {
    let available = queue.len().min(output.len());
    for sample in &mut output[..available] {
        *sample = queue.pop_front().unwrap_or(0);
    }
    output[available..].fill(0);
    output.len() - available
}

/// One active call's media: the published return leg plus its stop flag.
pub struct CallSession {
    pub stop: Arc<AtomicBool>,
    pub audio: AudioQueue,
    pub own_ticket: String,
    router: Router,
    publisher: tokio::task::JoinHandle<()>,
}

impl CallSession {
    /// Publish the return leg, sign the return-leg invite, and send it to the
    /// caller. The `Live` handle is kept alive for the whole call (dropping it
    /// early kills subscriber sessions).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        caller_addr: &EndpointAddr,
        caller_peer_id: &str,
        holder_endpoint_id: &str,
        transport: &IrohTransport,
        key: &IdentityKey,
        profile: AudioProfile,
        broadcast_name: &str,
    ) -> Result<Self> {
        let endpoint = EndpointConfig::from_env()
            .context("live endpoint config")?
            .bind()
            .await
            .context("live endpoint")?;
        let layout = LiveLayout::default();
        let node = Node::new(endpoint.clone(), layout.config());
        let router = node.mount(Router::builder(endpoint)).spawn();
        let broadcast: LocalBroadcast = layout
            .publish(&node, broadcast_name)
            .context("publish call broadcast")?;
        let audio = AudioQueue::new();
        let stop = Arc::new(AtomicBool::new(false));
        let publisher = tokio::spawn(publish_audio(
            broadcast,
            audio.clone(),
            Arc::clone(&stop),
            profile,
        ));
        let own_ticket = layout
            .ticket(node.id(), broadcast_name)
            .context("call ticket")?
            .to_string();
        let call_id = rand_suffix();
        let envelope = idfon_core::sign_message(
            key,
            holder_endpoint_id.to_string(),
            format!("eve_call_return_{call_id}"),
            MessageContent::Text {
                text: format!(
                    "IDFON-LIVE/1\naction=start\nticket={own_ticket}\nreturn=1\naudio_codec={}\naudio_sample_rate={}",
                    profile.codec, profile.sample_rate
                ),
            },
            format!("eve-call-{caller_peer_id}-return-{call_id}"),
            None,
        )
        .context("sign return-leg invite")?;
        let mut sent = false;
        for attempt in 0..4 {
            match transport.send(caller_addr, &envelope).await {
                Ok(_) => {
                    sent = true;
                    break;
                }
                Err(error) => {
                    eprintln!("[live-media] return-leg send retry {attempt}: {error}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
        if !sent {
            stop.store(true, Ordering::Relaxed);
            let _ = publisher.await;
            let _ = router.shutdown().await;
            anyhow::bail!("return-leg invite never acknowledged");
        }
        Ok(Self {
            stop,
            audio,
            own_ticket,
            router,
            publisher,
        })
    }

    /// Stop the call: signal the publisher, wait for it, then shut the node
    /// and its endpoint down.
    pub async fn shutdown(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.publisher.await;
        let _ = self.router.shutdown().await;
    }
}

/// Subscribe the caller's broadcast and emit paced 20 ms PCM frames.
///
/// The returned receiver yields `Vec<i16>` frames (24 kHz mono). The task owns
/// the subscribe endpoint and ends when the receiver drops or `stop` is set.
pub async fn subscribe_caller(
    ticket: LiveTicket,
    expected_profile: AudioProfile,
    stop: Arc<AtomicBool>,
) -> Result<mpsc::Receiver<Vec<i16>>> {
    let mut sub = None;
    for attempt in 0..6 {
        if stop.load(Ordering::Relaxed) {
            anyhow::bail!("call stopped before subscribe");
        }
        // Relay selection at bind time (docs/relay-config.md), like every
        // other endpoint — the caller may be relay-only behind enterprise NAT.
        let endpoint = idfon_core::relay::endpoint_builder()
            .transport_config(iroh_live::Congestion::default().transport_config())
            .bind()
            .await
            .context("caller subscribe endpoint")?;
        // A viewer only dials: no router, no layout.
        let node = Node::new(endpoint.clone(), NodeConfig::default());
        match tokio::time::timeout(Duration::from_secs(8), node.subscribe_ticket(&ticket)).await {
            Ok(Ok(subscription)) => {
                sub = Some((node, endpoint, subscription));
                break;
            }
            Ok(Err(error)) => {
                eprintln!("[live-media] caller subscribe retry {attempt}: {error:#}");
            }
            Err(_) => {
                eprintln!("[live-media] caller subscribe retry {attempt}: timed out");
            }
        }
        node.shutdown().await;
        endpoint.close().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let Some((node, endpoint, subscription)) = sub else {
        anyhow::bail!("caller broadcast never announced");
    };
    let catalog = subscription
        .remote_broadcast()
        .catalog_within(Duration::from_secs(10))
        .await
        .map_err(|error| anyhow!("caller catalog: {error}"))?;
    // A caller may publish its codec plus an alternate (audio renditions):
    // take the one the invite names, else the best ranked.
    let (name, config) = catalog
        .audio
        .ranked()
        .find(|(_, config)| {
            parse_audio_profile(Some(&config.codec.to_string()), Some(config.sample_rate))
                .is_ok_and(|profile| profile == expected_profile)
        })
        .or_else(|| catalog.audio.ranked().next())
        .ok_or_else(|| anyhow!("caller broadcast has no audio track"))?;
    let (name, config) = (name.clone(), config.clone());
    let actual_profile = parse_audio_profile(Some(&config.codec.to_string()), Some(config.sample_rate))?;
    anyhow::ensure!(
        actual_profile == expected_profile,
        "caller invite requested {expected_profile:?}, but media catalog advertises {actual_profile:?}"
    );
    let mut decode = moq_audio::decode::Options::default();
    decode.output.format = Format::S16;
    decode.output.sample_rate = Some(24_000);
    decode.output.layout = Some(moq_audio::Layout::Mono);
    // A superseded group older than 100 ms is skipped (the old `max_age`).
    decode.delay.max = Some(Duration::from_millis(100));
    // Live playout rebuilds a lost packet from the next one's in-band FEC copy
    // (or DRED), concealing what neither covers; recordings and downloads must
    // show holes as-is.
    decode.recover = Some(true);
    let mut consumer =
        moq_audio::decode::Consumer::new(&subscription.broadcast(), &config, &name, decode).await?;
    let (frame_tx, frame_rx) = mpsc::channel(8);
    tokio::spawn(async move {
        let mut pacer = CallerPacer::new();
        let read_stop = Arc::clone(&stop);
        let mut raw = {
            let (raw_tx, raw_rx) = mpsc::channel(8);
            tokio::spawn(async move {
                while !read_stop.load(Ordering::Relaxed) {
                    match consumer.read().await {
                        Ok(Some(moq_audio::decode::Read::Frame(frame))) => {
                            if raw_tx.send(Ok(frame)).await.is_err() {
                                break;
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            let _ = raw_tx.send(Err("caller track ended".into())).await;
                            break;
                        }
                        Err(error) => {
                            let _ = raw_tx.send(Err(error.to_string())).await;
                            break;
                        }
                    }
                }
            });
            raw_rx
        };
        let mut tick = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_millis(CHUNK_MS),
            Duration::from_millis(CHUNK_MS),
        );
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if stop.load(Ordering::Relaxed) { break; }
                    let mut pcm = vec![0i16; CHUNK_SAMPLES];
                    let _ = pacer.tick(&mut pcm);
                    if frame_tx.send(pcm).await.is_err() { break; }
                }
                item = raw.recv() => {
                    match item {
                        Some(Ok(frame)) => {
                            let samples: Vec<i16> = frame.data
                                .chunks_exact(2)
                                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                                .collect();
                            pacer.accept_frame(frame.timestamp.as_micros().unwrap_or(0) as i128, &samples);
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            }
        }
        drop(subscription);
        node.shutdown().await;
        endpoint.close().await;
    });
    Ok(frame_rx)
}

/// Caller-audio pacing: consume one 20 ms frame per tick, account arriving
/// frames against their pts timeline (gap-fill, late-drop, overflow trim).
pub struct CallerPacer {
    pub input: VecDeque<i16>,
    pub sent_samples: u64,
    pub dropped_samples: u64,
    origin_pts_us: Option<i128>,
}

impl CallerPacer {
    pub fn new() -> Self {
        Self {
            input: VecDeque::with_capacity(MAX_INPUT_BUFFER),
            sent_samples: 0,
            dropped_samples: 0,
            origin_pts_us: None,
        }
    }

    /// Consume one 20 ms frame; returns (queue_samples, missing_samples).
    pub fn tick(&mut self, pcm: &mut [i16]) -> (usize, usize) {
        let missing = fill_audio_frame(&mut self.input, pcm);
        self.sent_samples += CHUNK_SAMPLES as u64;
        (self.input.len(), missing)
    }

    pub fn accept_frame(&mut self, pts_us: i128, data: &[i16]) -> u64 {
        let cursor = self.sent_samples + self.dropped_samples + self.input.len() as u64;
        let origin = *self
            .origin_pts_us
            .get_or_insert_with(|| pts_us - (cursor * 1_000_000 / 24_000) as i128);
        let mut target = ((pts_us - origin).max(0) as u64 * 24_000) / 1_000_000;
        if self.input.is_empty() && cursor > target {
            self.origin_pts_us = Some(pts_us - (cursor * 1_000_000 / 24_000) as i128);
            target = cursor;
        }
        let media_silence = target.saturating_sub(cursor);
        if media_silence > 0 {
            if media_silence > MAX_INPUT_BUFFER as u64 {
                self.dropped_samples += media_silence - MAX_INPUT_BUFFER as u64;
            }
            self.input.extend(
                std::iter::repeat(0i16).take(media_silence.min(MAX_INPUT_BUFFER as u64) as usize),
            );
        }
        let cursor_after_gap = self.sent_samples + self.dropped_samples + self.input.len() as u64;
        let late_samples = cursor_after_gap
            .saturating_sub(target)
            .min(data.len() as u64);
        self.input.extend(data.iter().skip(late_samples as usize).copied());
        while self.input.len() > MAX_INPUT_BUFFER {
            self.input.pop_front();
            self.dropped_samples += 1;
        }
        late_samples
    }
}

/// Publishes one 20 ms frame per clock tick; underflow is silence.
pub async fn publish_audio(
    broadcast: LocalBroadcast,
    audio: AudioQueue,
    stop: Arc<AtomicBool>,
    profile: AudioProfile,
) {
    let (sender, source) = AudioSource::push(AudioFormat {
        sample_rate: 24_000,
        layout: moq_audio::Layout::Mono,
    });
    let mut config = moq_audio::encode::Config::new(profile.sample_rate, moq_audio::Layout::Mono);
    config.codec = profile.codec;
    // Voice: Opus as VoIP with discontinuous transmission; the producer sends
    // nothing while the talker is silent (the old `dtx` knob).
    if profile.codec == AudioCodec::Opus {
        config.kind = Some(moq_audio::encode::Kind::Voice);
    }
    if profile.codec == AudioCodec::Liquid {
        config.frame_duration = std::time::Duration::from_millis(80);
    }
    // In-band FEC: the frame after a hole carries a copy of this one.
    config.fec = profile.codec == AudioCodec::Opus;
    // Deep redundancy, off unless an operator asks for it (`IDFON_DRED_MS`).
    // The DRED cover is not published in the catalog; the subscriber's
    // recovering decode uses whatever the next packet carries. Below ~48 kbps
    // libopus drops DRED outright, so pair a long cover with a bitrate that
    // affords it.
    config.dred = (profile.codec == AudioCodec::Opus)
        .then(|| {
            std::env::var("IDFON_DRED_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .flatten()
        .map(|ms| std::time::Duration::from_millis(ms.clamp(80, 1_000)));
    if let Err(error) = broadcast.set_audio(source, config) {
        eprintln!("[live-media] return-leg audio refused: {error:#}");
        return;
    }
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_millis(CHUNK_MS),
        Duration::from_millis(CHUNK_MS),
    );
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pts = 0u64;
    let mut last_tick: Option<std::time::Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        tick.tick().await;
        let mut data = vec![0i16; CHUNK_SAMPLES];
        let _missing = audio.fill(&mut data);
        let now = std::time::Instant::now();
        let tick_gap_us = last_tick
            .map(|last| now.duration_since(last).as_micros())
            .unwrap_or(CHUNK_MS as u128 * 1_000);
        last_tick = Some(now);
        let mut bytes = Vec::with_capacity(CHUNK_SAMPLES * 4);
        for sample in data {
            bytes.extend_from_slice(&(f32::from(sample) / 32768.0).to_le_bytes());
        }
        let Ok(timestamp) = moq_net::Timestamp::from_micros(pts) else {
            break;
        };
        if sender
            .push(AudioFrame::new(bytes::Bytes::from(bytes), timestamp))
            .is_err()
        {
            break;
        }
        pts += if tick_gap_us > 30_000 {
            tick_gap_us as u64
        } else {
            CHUNK_MS * 1_000
        };
    }
    drop(broadcast);
}

/// Eight hex digits from the sub-second clock; call-id suffix, not a secret.
pub fn rand_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_parsing_legacy_and_pcm() {
        assert_eq!(parse_audio_profile(None, None).unwrap(), AudioProfile::default());
        assert_eq!(
            parse_audio_profile(Some("pcm"), Some(24_000)).unwrap().codec,
            AudioCodec::Pcm
        );
        assert!(parse_audio_profile(Some("pcm"), Some(48_000)).is_err());
    }

    #[test]
    fn invite_parses_fields() {
        let opus = parse_invite("IDFON-LIVE/1\naction=start\nticket=abc\naudio_codec=opus\naudio_sample_rate=24000")
            .expect("opus 24k invite");
        assert_eq!(opus.audio_codec.as_deref(), Some("opus"));
        assert_eq!(opus.audio_sample_rate, Some(24_000));
        assert_eq!(
            parse_audio_profile(Some("opus"), Some(24_000)).expect("agent-plane opus"),
            AudioProfile {
                codec: AudioCodec::Opus,
                sample_rate: 24_000,
            }
        );
        let text = "IDFON-LIVE/1\naction=start\nticket=abc\naudio_codec=pcm\naudio_sample_rate=24000";
        let invite = parse_invite(text).unwrap();
        assert!(invite.is_start);
        assert_eq!(invite.audio_codec.as_deref(), Some("pcm"));
        assert_eq!(invite.audio_sample_rate, Some(24_000));
        assert_eq!(invite.stt, None);
        assert!(parse_invite("hello").is_none());
    }

    #[test]
    fn invite_parses_voice_selection() {
        let text = "IDFON-LIVE/1\naction=start\nticket=abc\nstt=deepgram:nova-3\ntts=elevenlabs:turbo";
        let invite = parse_invite(text).unwrap();
        assert_eq!(invite.stt.as_deref(), Some("deepgram:nova-3"));
        assert_eq!(invite.tts.as_deref(), Some("elevenlabs:turbo"));
        // Empty values are treated as "not selected".
        let none = parse_invite("IDFON-LIVE/1\naction=start\nticket=abc\nstt=\ntts=").unwrap();
        assert_eq!(none.stt, None);
        assert_eq!(none.tts, None);
    }

    #[test]
    fn invite_parses_caller_context() {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
        let encoded = BASE64.encode("Client cascade: STT=Apple Built-in");
        let text = format!("IDFON-LIVE/1\naction=start\nticket=abc\ncontext_b64={encoded}");
        let invite = parse_invite(&text).unwrap();
        assert_eq!(invite.context.as_deref(), Some("Client cascade: STT=Apple Built-in"));
        // Absent and non-UTF8/empty values leave it unset rather than failing.
        assert_eq!(parse_invite("IDFON-LIVE/1\naction=start\nticket=abc").unwrap().context, None);
        assert_eq!(parse_invite("IDFON-LIVE/1\naction=start\nticket=abc\ncontext_b64=").unwrap().context, None);
    }

    #[test]
    fn invite_parses_side_overrides() {
        let invite = parse_invite(
            "IDFON-LIVE/1\naction=start\nticket=abc\nstt_side=client\ntts_side=server",
        )
        .unwrap();
        assert_eq!(invite.stt_side.as_deref(), Some("client"));
        assert_eq!(invite.tts_side.as_deref(), Some("server"));
        // Junk values are ignored rather than propagated to the route.
        let junk = parse_invite("IDFON-LIVE/1\naction=start\nticket=abc\nstt_side=bogus").unwrap();
        assert_eq!(junk.stt_side, None);
    }

    #[test]
    fn audio_queue_carries_odd_bytes_and_caps() {
        let queue = AudioQueue::new();
        // A trailing odd byte is held until the next push completes the sample.
        queue.push_bytes(&[1, 0, 2]);
        queue.push_bytes(&[0, 3, 0]);
        let mut out = [0i16; 3];
        assert_eq!(queue.fill(&mut out), 0);
        assert_eq!(out, [1, 2, 3]);
    }
}
