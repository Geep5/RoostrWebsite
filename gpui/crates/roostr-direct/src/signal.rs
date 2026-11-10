//! The signal payload (kind 25079's plaintext): one step of introducing a
//! viewer device to the computer running an agent.
//!
//! ```json
//! {"v":1,"type":"hello"|"offer"|"answer"|"ice"|"bye","from":"<device>","to":"<device>",
//!  "session":"<offerer's id>","role":"viewer"|"host","sdp":"…","candidate":"…","mid":"…","ts":ms}
//! ```
//!
//! Every payload carries all ten keys (unused strings empty); the engine's
//! `signal_publish` holds the same limits as [`Signal::parse`].

use serde_json::{Value as Json, json};

/// Largest `sdp` accepted.
pub const SDP_LIMIT: usize = 16 * 1024;
/// Largest `candidate` accepted.
pub const CANDIDATE_LIMIT: usize = 2 * 1024;
/// Largest `from` / `to` / `session` / `mid` accepted.
pub const ID_LIMIT: usize = 256;

/// The step a signal takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalType {
    /// A viewer is open (`to` empty): hosts offer it a channel.
    Hello,
    Offer,
    Answer,
    /// One trickled ICE candidate.
    Ice,
    /// The session is over.
    Bye,
}

impl SignalType {
    fn parse(text: &str) -> Option<SignalType> {
        Some(match text {
            "hello" => SignalType::Hello,
            "offer" => SignalType::Offer,
            "answer" => SignalType::Answer,
            "ice" => SignalType::Ice,
            "bye" => SignalType::Bye,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SignalType::Hello => "hello",
            SignalType::Offer => "offer",
            SignalType::Answer => "answer",
            SignalType::Ice => "ice",
            SignalType::Bye => "bye",
        }
    }
}

/// Which end of a direct line the sender is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// An app device showing the progress.
    Viewer,
    /// The computer running the agent (the harness).
    Host,
}

impl Role {
    fn parse(text: &str) -> Option<Role> {
        match text {
            "viewer" => Some(Role::Viewer),
            "host" => Some(Role::Host),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Host => "host",
        }
    }
}

/// One signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub kind: SignalType,
    /// The sender's device id.
    pub from: String,
    /// The addressed device; empty for a hello.
    pub to: String,
    pub session: String,
    pub role: Role,
    pub sdp: String,
    pub candidate: String,
    pub mid: String,
    pub ts: i64,
}

impl Signal {
    /// A signal from `from` (playing `role`) with every other field empty.
    pub fn new(kind: SignalType, role: Role, from: &str, ts: i64) -> Signal {
        Signal {
            kind,
            from: from.to_string(),
            to: String::new(),
            session: String::new(),
            role,
            sdp: String::new(),
            candidate: String::new(),
            mid: String::new(),
            ts,
        }
    }

    /// A version-1 payload within the limits; `None` for anything else. A
    /// missing string reads as empty; a hello needs no `to` or `session`,
    /// every other type both, an offer or answer its `sdp`, an ice its
    /// `candidate`.
    pub fn parse(payload: &Json) -> Option<Signal> {
        if payload.get("v").and_then(Json::as_u64) != Some(1) {
            return None;
        }
        let text = |key: &str, limit: usize| -> Option<String> {
            match payload.get(key) {
                None | Some(Json::Null) => Some(String::new()),
                Some(Json::String(s)) if s.len() <= limit => Some(s.clone()),
                Some(_) => None,
            }
        };
        let signal = Signal {
            kind: SignalType::parse(payload.get("type")?.as_str()?)?,
            from: text("from", ID_LIMIT)?,
            to: text("to", ID_LIMIT)?,
            session: text("session", ID_LIMIT)?,
            role: Role::parse(payload.get("role")?.as_str()?)?,
            sdp: text("sdp", SDP_LIMIT)?,
            candidate: text("candidate", CANDIDATE_LIMIT)?,
            mid: text("mid", ID_LIMIT)?,
            ts: payload.get("ts").and_then(Json::as_i64).unwrap_or(0),
        };
        let complete = !signal.from.is_empty()
            && match signal.kind {
                SignalType::Hello => true,
                _ if signal.to.is_empty() || signal.session.is_empty() => false,
                SignalType::Offer | SignalType::Answer => !signal.sdp.is_empty(),
                SignalType::Ice => !signal.candidate.is_empty(),
                SignalType::Bye => true,
            };
        complete.then_some(signal)
    }

    /// The payload, all ten keys.
    pub fn to_json(&self) -> Json {
        json!({
            "v": 1,
            "type": self.kind.as_str(),
            "from": self.from,
            "to": self.to,
            "session": self.session,
            "role": self.role.as_str(),
            "sdp": self.sdp,
            "candidate": self.candidate,
            "mid": self.mid,
            "ts": self.ts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> Json {
        json!({"v":1,"type":"offer","from":"host-1","to":"me","session":"s1","role":"host",
            "sdp":"v=0\r\n","candidate":"","mid":"","ts":5})
    }

    #[test]
    fn a_payload_round_trips_with_all_ten_keys() {
        let signal = Signal::parse(&offer()).expect("parses");
        assert_eq!(signal.kind, SignalType::Offer);
        assert_eq!(signal.role, Role::Host);
        assert_eq!((signal.from.as_str(), signal.to.as_str()), ("host-1", "me"));
        let out = signal.to_json();
        assert_eq!(out, offer());
        assert_eq!(out.as_object().unwrap().len(), 10);
    }

    #[test]
    fn missing_strings_read_empty_and_hello_needs_no_address() {
        let hello = json!({"v":1,"type":"hello","from":"d","role":"viewer","ts":1});
        let signal = Signal::parse(&hello).expect("a bare hello parses");
        assert_eq!((signal.to.as_str(), signal.session.as_str()), ("", ""));
    }

    #[test]
    fn bad_payloads_are_refused() {
        let with = |key: &str, value: Json| {
            let mut p = offer();
            p[key] = value;
            Signal::parse(&p)
        };
        assert_eq!(with("v", json!(2)), None, "another version");
        assert_eq!(with("type", json!("dance")), None, "unknown type");
        assert_eq!(with("role", json!("peer")), None, "unknown role");
        assert_eq!(with("from", json!("")), None, "no sender");
        assert_eq!(with("to", json!("")), None, "an offer needs its device");
        assert_eq!(
            with("session", json!("")),
            None,
            "an offer needs its session"
        );
        assert_eq!(with("sdp", json!("")), None, "an offer needs its sdp");
        assert_eq!(with("sdp", json!(7)), None, "sdp not a string");
        assert_eq!(
            with("sdp", json!("x".repeat(SDP_LIMIT + 1))),
            None,
            "sdp over 16 KiB"
        );
        assert!(
            with("sdp", json!("x".repeat(SDP_LIMIT))).is_some(),
            "16 KiB is fine"
        );
        assert_eq!(
            with("from", json!("x".repeat(ID_LIMIT + 1))),
            None,
            "long id"
        );
        let mut ice = offer();
        ice["type"] = json!("ice");
        assert_eq!(Signal::parse(&ice), None, "an ice needs its candidate");
        ice["candidate"] = json!("x".repeat(CANDIDATE_LIMIT + 1));
        assert_eq!(Signal::parse(&ice), None, "candidate over 2 KiB");
    }
}
