//! Human-first commands and helpers: name resolution, inbox, reply waiting,
//! and argument parsing that lets people (and agents) type what they mean.

use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idfon_protocol::{rpc, Peer, Response, ResponseBody};
use serde_json::Value;

use crate::{call_plain, request_error};

/// The result object of a successful response, or the error as `io::Error`.
pub fn ok_result(response: Response) -> io::Result<Value> {
    match response.body {
        ResponseBody::Success { result, .. } => Ok(result),
        ResponseBody::Failure { .. } => Err(io::Error::other(request_error(&response))),
    }
}

/// Looks up a saved peer by name, alias, id or endpoint id.
pub fn resolve_peer(socket: &str, identity: Option<&str>, reference: &str) -> io::Result<Peer> {
    let response = call_plain(
        socket,
        &rpc::PeerShow {
            peer: reference.to_owned(),
        },
        identity,
    )?;
    if !response.ok {
        return Err(io::Error::other(format!(
            "no peer named {reference:?} (see `idfon peer list`)"
        )));
    }
    let result = ok_result(response)?;
    serde_json::from_value(result["peer"].clone()).map_err(io::Error::other)
}

/// A peer id for capability params: a 64-hex key passes through, anything
/// else is resolved as a saved peer.
pub fn peer_key(socket: &str, identity: Option<&str>, reference: &str) -> io::Result<String> {
    if reference.len() == 64 && reference.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(reference.to_owned());
    }
    Ok(resolve_peer(socket, identity, reference)?.id)
}

pub fn peer_matches(peer: &Peer, id: &str) -> bool {
    peer.id == id || peer.knows_endpoint(id)
}

pub fn all_peers(socket: &str, identity: Option<&str>) -> Vec<Peer> {
    call_plain(socket, &rpc::PeerList {}, identity)
        .ok()
        .and_then(|response| ok_result(response).ok())
        .and_then(|result| serde_json::from_value(result["peers"].clone()).ok())
        .unwrap_or_default()
}

/// The display name for a sender id, or a short id.
pub fn sender_name(peers: &[Peer], id: &str) -> String {
    peers
        .iter()
        .find(|peer| peer_matches(peer, id))
        .map(|peer| peer.name.clone())
        .unwrap_or_else(|| id.chars().take(10).collect())
}

/// Received-message events, oldest first.
pub fn message_events(
    socket: &str,
    identity: Option<&str>,
    after: Option<String>,
) -> io::Result<Vec<Value>> {
    let result = ok_result(call_plain(
        socket,
        &rpc::EventList {
            after,
            event_type: Some("message.received".into()),
            ..Default::default()
        },
        identity,
    )?)?;
    Ok(result["events"].as_array().cloned().unwrap_or_default())
}

/// Cursor of the newest event, if any.
pub fn latest_cursor(socket: &str, identity: Option<&str>) -> Option<String> {
    let result = ok_result(call_plain(socket, &rpc::EventList::default(), identity).ok()?).ok()?;
    result["events"]
        .as_array()?
        .last()?
        .get("cursor")?
        .as_str()
        .map(str::to_owned)
}

/// Waits for the first received message after `after` that passes `keep`.
pub fn wait_message(
    socket: &str,
    identity: Option<&str>,
    mut after: Option<String>,
    timeout: Duration,
    keep: impl Fn(&Value) -> bool,
) -> io::Result<Option<Value>> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let result = ok_result(call_plain(
            socket,
            &rpc::EventWait {
                after: after.clone(),
                event_type: Some("message.received".into()),
                timeout_ms: Some(remaining.as_millis().clamp(1, 30_000) as u64),
                peer: None,
            },
            identity,
        )?)?;
        let events = result["events"].as_array().cloned().unwrap_or_default();
        if let Some(cursor) = events.last().and_then(|e| e["cursor"].as_str()) {
            after = Some(cursor.to_owned());
        }
        if let Some(event) = events.into_iter().find(|event| keep(event)) {
            return Ok(Some(event));
        }
    }
}

/// `IDFON-DATA/1` envelope → (blob ticket, declared size).
pub fn data_envelope(text: &str) -> Option<(String, Option<usize>)> {
    let mut ticket = None;
    let mut size = None;
    for line in text.strip_prefix("IDFON-DATA/1\n")?.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "ticket" => ticket = Some(value.to_string()),
            "size" => size = value.parse().ok(),
            _ => {}
        }
    }
    Some((ticket?, size))
}

/// One line describing a message body for people.
pub fn describe_text(text: &str) -> String {
    if let Some((_, size)) = data_envelope(text) {
        return match size {
            Some(size) => format!("[file, {size} bytes — save it with `idfon recv`]"),
            None => "[file — save it with `idfon recv`]".into(),
        };
    }
    match text.split_once('\n') {
        Some((head, _)) if head.starts_with("IDFON-") && head.ends_with("/1") => {
            format!("[{head}]")
        }
        _ => text.to_owned(),
    }
}

/// `idfon inbox`: received messages as `name: text`, optionally following.
pub fn cmd_inbox(
    socket: &str,
    identity: Option<&str>,
    from: Option<&str>,
    limit: usize,
    follow: bool,
    json: bool,
) -> io::Result<()> {
    let from = from
        .map(|reference| resolve_peer(socket, identity, reference))
        .transpose()?;
    let peers = all_peers(socket, identity);
    let keep = |event: &Value| {
        from.as_ref()
            .is_none_or(|peer| peer_matches(peer, event["data"]["peer_id"].as_str().unwrap_or("")))
    };
    let print = |event: &Value| {
        if json {
            println!("{event}");
            return;
        }
        let data = &event["data"];
        let name = sender_name(&peers, data["peer_id"].as_str().unwrap_or(""));
        let room = data["conversation"]
            .as_str()
            .map(|room| format!(" (room {})", room.chars().take(10).collect::<String>()))
            .unwrap_or_default();
        println!(
            "{name}{room}: {}",
            describe_text(data["text"].as_str().unwrap_or(""))
        );
    };
    let events: Vec<Value> = message_events(socket, identity, None)?
        .into_iter()
        .filter(|event| keep(event))
        .collect();
    let skip = events.len().saturating_sub(limit);
    for event in &events[skip..] {
        print(event);
    }
    if events.is_empty() && !follow && !json {
        eprintln!("no messages");
    }
    if follow {
        let mut after = latest_cursor(socket, identity);
        loop {
            if let Some(event) =
                wait_message(socket, identity, after.clone(), Duration::from_secs(30), |event: &Value| keep(event))?
            {
                after = event["cursor"].as_str().map(str::to_owned);
                print(&event);
            }
        }
    }
    Ok(())
}

/// Seconds since the epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}

/// RFC 3339 UTC timestamp for `secs` since the epoch.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// `--expires`: a date (`2026-12-31`, end of that day UTC), an RFC 3339
/// time, or a duration from now (`30d`, `12h`).
pub fn parse_expiry(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let is_date = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && value.chars().filter(char::is_ascii_digit).count() == 8;
    if is_date {
        return Ok(format!("{value}T23:59:59Z"));
    }
    if value.contains('T') {
        return Ok(value.to_owned());
    }
    crate::parse_duration(value)
        .map(|secs| rfc3339(unix_now() + secs))
        .map_err(|_| format!("invalid expiry {value:?}: use 2026-12-31, an RFC 3339 time, or 30d"))
}

/// `--timeout` style durations: `30s`, `5m`, `2h`; a bare number is seconds.
pub fn parse_wait(value: &str) -> Result<Duration, String> {
    crate::parse_duration(value).map(Duration::from_secs)
}

/// A path the daemon can open: relative paths are resolved here, because
/// the daemon runs with its own working directory.
pub fn daemon_path(path: &str) -> io::Result<String> {
    std::fs::canonicalize(path)
        .map(|path| path.display().to_string())
        .map_err(|error| io::Error::new(error.kind(), format!("cannot read {path}: {error}")))
}

/// A room id from a room name or id.
pub fn resolve_room(socket: &str, identity: Option<&str>, reference: &str) -> io::Result<String> {
    let result = ok_result(call_plain(socket, &rpc::RoomList {}, identity)?)?;
    let rooms = result["rooms"].as_array().cloned().unwrap_or_default();
    rooms
        .iter()
        .find(|room| room["id"] == reference || room["name"] == reference)
        .and_then(|room| room["id"].as_str().map(str::to_owned))
        .ok_or_else(|| io::Error::other(format!("no room named {reference:?} (see `idfon room list`)")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_798_761_599), "2026-12-31T23:59:59Z");
    }

    #[test]
    fn expiry_accepts_dates_times_and_durations() {
        assert_eq!(parse_expiry("2026-12-31").unwrap(), "2026-12-31T23:59:59Z");
        assert_eq!(parse_expiry("2026-12-31T10:00:00Z").unwrap(), "2026-12-31T10:00:00Z");
        assert!(parse_expiry("30d").unwrap().ends_with('Z'));
        assert!(parse_expiry("soon").is_err());
    }

    #[test]
    fn describe_text_hides_envelopes() {
        assert_eq!(
            describe_text("IDFON-DATA/1\nticket=blobx\nsize=22"),
            "[file, 22 bytes — save it with `idfon recv`]"
        );
        assert_eq!(describe_text("hello"), "hello");
    }
}
