//! Chat responder: answers a holder's text turns through an OpenAI-compatible
//! chat endpoint (AI Gateway by default), in place of the Node bridge + Eve
//! agent. For a holder whose only job is to talk, such as the App Review demo
//! contact (idfon-edge `deploy/reviewer/`); anything that needs tools stays on
//! Eve.
//!
//! ```text
//! idfon-chat-responder --socket <holder.sock> --model <provider/model>
//!     [--instructions-file FILE] [--url URL] [--api-key-env NAME]
//! ```
//!
//! It is the holder's IPC client (`turn.in` in, `reply.out` back; frames are a
//! little-endian u32 length + JSON, as in `eve_idfon`). History is the last
//! few turns per (peer, conversation), in memory only. Message text is never
//! logged. Turns are answered one at a time, in arrival order.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const HISTORY_TURNS: usize = 12;
const MAX_REPLY_TOKENS: u32 = 500;
const DEFAULT_URL: &str = "https://ai-gateway.vercel.sh/v1";
const DEFAULT_INSTRUCTIONS: &str = "You are a friendly assistant reached through Idfon, a \
    peer-to-peer messaging and calling app. Reply briefly and plainly.";
const FALLBACK_REPLY: &str = "Sorry, I couldn't answer just now. Please try again in a moment.";
const NON_TEXT_REPLY: &str = "I can only read text messages.";

#[derive(Debug)]
struct Args {
    socket: PathBuf,
    model: String,
    url: String,
    api_key_env: String,
    instructions: String,
}

fn parse_args(raw: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut socket = None;
    let mut model = None;
    let mut url = DEFAULT_URL.to_string();
    let mut api_key_env = "AI_GATEWAY_API_KEY".to_string();
    let mut instructions = DEFAULT_INSTRUCTIONS.to_string();
    let mut raw = raw.into_iter();
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| anyhow!("{flag} needs a value"));
        match flag.as_str() {
            "--socket" => socket = Some(PathBuf::from(value()?)),
            "--model" => model = Some(value()?),
            "--url" => url = value()?.trim_end_matches('/').to_string(),
            "--api-key-env" => api_key_env = value()?,
            "--instructions-file" => {
                let path = value()?;
                instructions = std::fs::read_to_string(&path)
                    .with_context(|| format!("read {path}"))?
                    .trim()
                    .to_string();
            }
            "-h" | "--help" => bail!(
                "usage: idfon-chat-responder --socket <holder.sock> --model <provider/model> \
                 [--instructions-file FILE] [--url URL] [--api-key-env NAME]"
            ),
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args {
        socket: socket.ok_or_else(|| anyhow!("--socket is required"))?,
        model: model.ok_or_else(|| anyhow!("--model is required"))?,
        url,
        api_key_env,
        instructions,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = parse_args(std::env::args().skip(1))?;
    let key = std::env::var(&args.api_key_env)
        .ok()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| anyhow!("{} is not set", args.api_key_env))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("http client")?;
    log("info", &format!("model {}; holder socket {}", args.model, args.socket.display()));
    // The holder may start after us or restart under us: keep reconnecting.
    loop {
        match serve(&args, &client, &key).await {
            Ok(()) => log("warn", "holder closed the socket; reconnecting"),
            Err(error) => log("warn", &format!("{error:#}; reconnecting")),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn serve(args: &Args, client: &reqwest::Client, key: &str) -> Result<()> {
    let stream = UnixStream::connect(&args.socket)
        .await
        .with_context(|| format!("connect {}", args.socket.display()))?;
    let (mut reader, mut writer) = stream.into_split();
    log("info", "connected to the holder");
    let mut history = History::default();
    while let Some(frame) = read_frame(&mut reader).await? {
        let Some(turn) = Turn::from_frame(&frame) else { continue };
        let reply = if turn.text.trim().is_empty() {
            NON_TEXT_REPLY.to_string()
        } else {
            let body = request_body(&args.model, &args.instructions, history.get(&turn.key), &turn.text);
            match complete(client, &args.url, key, &body).await {
                Ok(reply) => {
                    history.push(&turn.key, &turn.text, &reply);
                    reply
                }
                Err(error) => {
                    log("error", &format!("turn {}: {error:#}", turn.message_id));
                    FALLBACK_REPLY.to_string()
                }
            }
        };
        let out = json!({ "type": "reply.out", "in_reply_to": turn.message_id, "text": reply });
        write_frame(&mut writer, &out).await?;
        log("info", &format!("answered turn {}", turn.message_id));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Turn {
    message_id: String,
    key: String,
    text: String,
}

impl Turn {
    /// A `turn.in` from a real sender; every other frame (acks, status, voice
    /// info) and holder-injected turns are not ours to answer.
    fn from_frame(frame: &Value) -> Option<Self> {
        if frame["type"] != "turn.in" || frame.get("source").is_some_and(|s| !s.is_null()) {
            return None;
        }
        let message_id = frame["message_id"].as_str()?.to_string();
        let peer = frame["peer_id"].as_str()?;
        let conversation = frame["conversation"].as_str().unwrap_or("");
        Some(Self {
            message_id,
            key: format!("{peer}|{conversation}"),
            text: frame["text"].as_str().unwrap_or("").to_string(),
        })
    }
}

#[derive(Default)]
struct History(HashMap<String, VecDeque<(String, String)>>);

impl History {
    fn get(&self, key: &str) -> &[(String, String)] {
        self.0.get(key).map(|turns| turns.as_slices().0).unwrap_or(&[])
    }

    fn push(&mut self, key: &str, said: &str, reply: &str) {
        let turns = self.0.entry(key.to_string()).or_default();
        turns.push_back((said.to_string(), reply.to_string()));
        while turns.len() > HISTORY_TURNS {
            turns.pop_front();
        }
        turns.make_contiguous();
    }
}

fn request_body(model: &str, instructions: &str, history: &[(String, String)], text: &str) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": instructions })];
    for (said, reply) in history {
        messages.push(json!({ "role": "user", "content": said }));
        messages.push(json!({ "role": "assistant", "content": reply }));
    }
    messages.push(json!({ "role": "user", "content": text }));
    json!({ "model": model, "messages": messages, "max_tokens": MAX_REPLY_TOKENS })
}

async fn complete(client: &reqwest::Client, url: &str, key: &str, body: &Value) -> Result<String> {
    let response = client
        .post(format!("{url}/chat/completions"))
        .bearer_auth(key)
        .json(body)
        .send()
        .await
        .context("chat request")?;
    let status = response.status();
    let value: Value = response.json().await.context("chat response")?;
    if !status.is_success() {
        bail!("chat endpoint answered {status}: {}", value["error"]);
    }
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::trim)
        .filter(|reply| !reply.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("chat response had no text"))
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Value>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME_BYTES {
        bail!("holder frame too large: {length} bytes");
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await.context("read frame")?;
    Ok(Some(serde_json::from_slice(&payload).context("decode frame")?))
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Value) -> Result<()> {
    let payload = serde_json::to_vec(frame)?;
    writer.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

fn log(level: &str, message: &str) {
    eprintln!(
        "{}",
        json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "level": level,
            "target": "idfon.chat-responder",
            "message": message,
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_need_socket_and_model() {
        let args = parse_args(
            ["--socket", "/tmp/h.sock", "--model", "openai/x", "--url", "https://g.test/v1/"]
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.socket, PathBuf::from("/tmp/h.sock"));
        assert_eq!(args.url, "https://g.test/v1");
        assert_eq!(args.api_key_env, "AI_GATEWAY_API_KEY");
        assert!(parse_args(["--model", "x"].map(String::from)).is_err());
        assert!(parse_args(["--bogus"].map(String::from)).is_err());
    }

    #[test]
    fn only_real_turns_are_answered() {
        let turn = json!({
            "type": "turn.in", "message_id": "m1", "peer_id": "p", "endpoint_id": "e",
            "idempotency_key": "k", "conversation": "c", "text": "hi",
        });
        assert_eq!(
            Turn::from_frame(&turn),
            Some(Turn { message_id: "m1".into(), key: "p|c".into(), text: "hi".into() })
        );
        let mut injected = turn.clone();
        injected["source"] = json!("gpt-live-delegation");
        assert_eq!(Turn::from_frame(&injected), None);
        assert_eq!(Turn::from_frame(&json!({ "type": "reply.ack", "in_reply_to": "m1" })), None);
    }

    #[test]
    fn history_is_capped_and_ordered() {
        let mut history = History::default();
        for i in 0..HISTORY_TURNS + 3 {
            history.push("p|", &format!("q{i}"), &format!("a{i}"));
        }
        let turns = history.get("p|");
        assert_eq!(turns.len(), HISTORY_TURNS);
        assert_eq!(turns[0].0, "q3");
        let body = request_body("m", "be brief", turns, "now");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1 + 2 * HISTORY_TURNS + 1);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages.last().unwrap()["content"], "now");
        assert_eq!(body["max_tokens"], MAX_REPLY_TOKENS);
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let frame = json!({ "type": "reply.out", "in_reply_to": "m1", "text": "ok" });
        write_frame(&mut a, &frame).await.unwrap();
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(frame));
        assert_eq!(read_frame(&mut b).await.unwrap(), None);
    }
}
