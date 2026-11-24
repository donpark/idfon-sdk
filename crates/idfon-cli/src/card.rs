//! Cards (`IDFON-UI/1`) and their answers (`IDFON-UI-ACTION/1`), as the apps
//! exchange them (idfon-core `docs/chat-envelopes.md`): `idfon card send`
//! builds one, `idfon card wait` (and `send --wait`) reads the answer.

use serde_json::{Map, Value};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (what the apps use).
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.trim().bytes() {
        if byte == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|&c| c == byte)? as u32;
        buffer = buffer << 6 | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits & 0xff) as u8);
        }
    }
    Some(out)
}

/// The envelope for `card` (a JSON object with at least `kind` and `id`).
pub fn card_envelope(card: &Value) -> String {
    format!("IDFON-UI/1\npayload={}", b64_encode(card.to_string().as_bytes()))
}

/// The card inside an `IDFON-UI/1` envelope.
#[cfg(test)]
pub fn parse_card(text: &str) -> Option<Value> {
    let rest = text.strip_prefix("IDFON-UI/1\n")?;
    let payload = rest.lines().find_map(|line| line.strip_prefix("payload="))?;
    let card: Value = serde_json::from_slice(&b64_decode(payload)?).ok()?;
    (card["kind"].is_string() && card["id"].is_string()).then_some(card)
}

/// An answer: which card, which action, and what was typed.
#[derive(Debug, PartialEq)]
pub struct Answer {
    pub card: String,
    pub action: String,
    pub values: Map<String, Value>,
}

pub fn parse_answer(text: &str) -> Option<Answer> {
    let rest = text.strip_prefix("IDFON-UI-ACTION/1\n")?;
    let mut card = None;
    let mut action = None;
    let mut values = Map::new();
    for line in rest.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "card" => card = Some(value.to_owned()),
            "action" => action = Some(value.to_owned()),
            "values" => {
                // The value is base64, which may itself end in '='.
                let encoded = &line["values=".len()..];
                if let Some(Value::Object(map)) =
                    b64_decode(encoded).and_then(|bytes| serde_json::from_slice(&bytes).ok())
                {
                    values = map;
                }
            }
            _ => {}
        }
    }
    Some(Answer { card: card?, action: action?, values })
}

/// `ID=TITLE` (an action) or `ID=LABEL` (a field).
pub fn split_pair(spec: &str) -> Result<(String, String), String> {
    match spec.split_once('=') {
        Some((id, title)) if !id.is_empty() && !title.is_empty() => Ok((id.to_owned(), title.to_owned())),
        _ => Err(format!("expected ID=TITLE, got {spec:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for text in ["", "a", "ab", "abc", "abcd", "{\"name\":\"Ann\"}"] {
            assert_eq!(b64_decode(&b64_encode(text.as_bytes())).unwrap(), text.as_bytes());
        }
        assert_eq!(b64_encode(b"{\"name\":\"Ann\"}"), "eyJuYW1lIjoiQW5uIn0=");
    }

    #[test]
    fn a_card_round_trips() {
        let card = serde_json::json!({"kind": "poll", "id": "p1", "title": "Lunch?"});
        assert_eq!(parse_card(&card_envelope(&card)).unwrap(), card);
        assert!(parse_card("IDFON-UI/1\npayload=e30=").is_none(), "kind and id are required");
    }

    #[test]
    fn reads_what_the_phone_sends_back() {
        // The iPhone's answer to a booking form (idfon-ios CardSheetTests).
        let answer = parse_answer(
            "IDFON-UI-ACTION/1\ncard=book-1\naction=book\nvalues=eyJuYW1lIjoiQW5uIiwicGVvcGxlIjoiMiJ9",
        )
        .unwrap();
        assert_eq!(answer.card, "book-1");
        assert_eq!(answer.action, "book");
        assert_eq!(answer.values["name"], "Ann");
        assert_eq!(answer.values["people"], "2");
        let bare = parse_answer("IDFON-UI-ACTION/1\ncard=compose\naction=menu").unwrap();
        assert!(bare.values.is_empty());
        assert!(parse_answer("IDFON-UI-ACTION/1\naction=x").is_none());
    }

    #[test]
    fn pairs_need_both_halves() {
        assert_eq!(split_pair("yes=Sounds good").unwrap(), ("yes".into(), "Sounds good".into()));
        assert!(split_pair("yes").is_err());
        assert!(split_pair("=x").is_err());
    }
}
