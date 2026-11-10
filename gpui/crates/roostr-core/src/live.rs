//! An agent's live progress while it works on a reply: its extended-thinking
//! summaries and tool steps, as the harness publishes them. Nothing is stored;
//! the persisted record rides on the reply block (`thinking` / `thought_ms`).
//!
//! The payload is the same on every path: an `/api/events` frame
//! (`{"type":"live","live":{…}}`), a relay event the replica's engine decrypts
//! (`ingest` → `live`).

use serde::Deserialize;
use serde_json::Value as Json;

/// What one live event carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveKind {
    /// A thinking-summary delta to append.
    Thinking,
    /// A tool step: one-line summary (`shell_exec: git status`).
    Step,
    /// A reply-text delta to append.
    Text,
    /// The run finished.
    Done,
    /// The run failed; `text` is the message.
    Error,
}

/// One live progress event of an agent's run.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LiveEvent {
    pub run: String,
    pub agent: String,
    /// The object whose chat it is.
    pub object: String,
    #[serde(default)]
    pub thread: String,
    /// Increasing per run; events can arrive late, twice or out of order.
    pub seq: u64,
    pub kind: LiveKind,
    #[serde(default)]
    pub text: String,
    /// Publisher clock, ms.
    #[serde(default)]
    pub ts: i64,
}

impl LiveEvent {
    /// A version-1 payload; `None` for any other version or shape.
    pub fn parse(payload: &Json) -> Option<LiveEvent> {
        if payload.get("v").and_then(Json::as_u64) != Some(1) {
            return None;
        }
        let event = LiveEvent::deserialize(payload).ok()?;
        (!event.run.is_empty() && !event.object.is_empty()).then_some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_version_one_payload_parses_and_others_do_not() {
        let payload = json!({"v":1,"run":"r1","agent":"a","object":"o","thread":"__discussion__",
            "seq":3,"kind":"step","text":"shell_exec: git status","ts":1000});
        let event = LiveEvent::parse(&payload).expect("parses");
        assert_eq!(event.kind, LiveKind::Step);
        assert_eq!((event.seq, event.ts), (3, 1000));
        assert_eq!(event.text, "shell_exec: git status");
        let mut v2 = payload.clone();
        v2["v"] = json!(2);
        assert_eq!(LiveEvent::parse(&v2), None);
        let mut unknown = payload;
        unknown["kind"] = json!("dance");
        assert_eq!(LiveEvent::parse(&unknown), None);
    }
}
