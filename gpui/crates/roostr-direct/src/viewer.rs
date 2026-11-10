//! The viewer's side of the introductions, without I/O: which signals to
//! answer, which to send, when to say hello. [`crate::Direct`] runs it.

use std::collections::{HashMap, VecDeque};

use crate::signal::{Role, Signal, SignalType};

/// A hello goes out on start and this often after.
pub const HELLO_EVERY_MS: i64 = 60_000;
/// Candidates held for an offer not here yet (relays reorder).
const EARLY_ICE: usize = 64;

/// Where a signal travelled: `None` = the account's own (personal) scope,
/// `Some(space id)` = a shared space's.
pub type Scope = Option<String>;

/// What the viewer asks its runner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send a signal (kind 25079) in `space`; a hello goes out in every scope.
    Publish { signal: Signal, space: Scope },
    /// Make a peer answering `offer` for `session`.
    Answer { session: String, offer: String },
    /// Hand a trickled candidate to `session`'s peer.
    RemoteIce {
        session: String,
        candidate: String,
        mid: String,
    },
    /// Close and forget `session`'s peer.
    Close { session: String },
}

/// What a peer reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// The local answer to the offer.
    Answer(String),
    /// A local candidate to trickle.
    Ice { candidate: String, mid: String },
    /// The data channel opened.
    Open,
    /// One text message on the channel.
    Message(String),
    /// The line is gone (failed, timed out or closed by the host).
    Closed,
}

/// A data-channel message for the app, with the scope its line was
/// introduced in (a shared space's line may only speak of that space).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub text: String,
    pub space: Scope,
}

#[derive(Debug)]
struct Session {
    /// The host's device id.
    host: String,
    space: Scope,
    open: bool,
}

#[derive(Debug)]
struct EarlyIce {
    session: String,
    host: String,
    candidate: String,
    mid: String,
}

/// One device's viewer state.
#[derive(Debug)]
pub struct Viewer {
    device: String,
    next_hello: i64,
    sessions: HashMap<String, Session>,
    early: VecDeque<EarlyIce>,
}

impl Viewer {
    /// The viewer of `device` (this install's id); its first hello is due now.
    pub fn new(device: impl Into<String>) -> Viewer {
        Viewer {
            device: device.into(),
            next_hello: i64::MIN,
            sessions: HashMap::new(),
            early: VecDeque::new(),
        }
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// When [`Self::tick`] next has something to do (epoch ms).
    pub fn next_wake(&self) -> i64 {
        self.next_hello
    }

    /// Lines whose channel is open.
    pub fn open_lines(&self) -> usize {
        self.sessions.values().filter(|s| s.open).count()
    }

    /// The hello, when due.
    pub fn tick(&mut self, now: i64) -> Vec<Effect> {
        if now < self.next_hello {
            return Vec::new();
        }
        self.next_hello = now + HELLO_EVERY_MS;
        vec![Effect::Publish {
            signal: Signal::new(SignalType::Hello, Role::Viewer, &self.device, now),
            space: None,
        }]
    }

    /// A signal heard in `space`. Only a host's offer, ice or bye addressed
    /// to this device counts; our own echoes, other devices' traffic and
    /// every hello are ignored.
    pub fn on_signal(&mut self, signal: Signal, space: Scope) -> Vec<Effect> {
        if signal.from == self.device || signal.role != Role::Host || signal.to != self.device {
            return Vec::new();
        }
        match signal.kind {
            SignalType::Hello | SignalType::Answer => Vec::new(),
            SignalType::Offer => self.offer(signal, space),
            SignalType::Ice => {
                match self.sessions.get(&signal.session) {
                    Some(s) if s.host == signal.from => {
                        return vec![Effect::RemoteIce {
                            session: signal.session,
                            candidate: signal.candidate,
                            mid: signal.mid,
                        }];
                    }
                    Some(_) => {}
                    None => {
                        if self.early.len() >= EARLY_ICE {
                            self.early.pop_front();
                        }
                        self.early.push_back(EarlyIce {
                            session: signal.session,
                            host: signal.from,
                            candidate: signal.candidate,
                            mid: signal.mid,
                        });
                    }
                }
                Vec::new()
            }
            SignalType::Bye => match self.sessions.get(&signal.session) {
                Some(s) if s.host == signal.from => {
                    self.sessions.remove(&signal.session);
                    vec![Effect::Close {
                        session: signal.session,
                    }]
                }
                _ => Vec::new(),
            },
        }
    }

    fn offer(&mut self, signal: Signal, space: Scope) -> Vec<Effect> {
        if self.sessions.contains_key(&signal.session) {
            return Vec::new(); // the same offer again (another relay)
        }
        // One line per host: its new offer replaces the old line.
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.host == signal.from)
            .map(|(id, _)| id.clone())
            .collect();
        let mut effects: Vec<Effect> = stale
            .into_iter()
            .map(|session| {
                self.sessions.remove(&session);
                Effect::Close { session }
            })
            .collect();
        self.sessions.insert(
            signal.session.clone(),
            Session {
                host: signal.from.clone(),
                space,
                open: false,
            },
        );
        effects.push(Effect::Answer {
            session: signal.session.clone(),
            offer: signal.sdp,
        });
        let (early, rest): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.early)
            .into_iter()
            .partition(|e| e.session == signal.session && e.host == signal.from);
        self.early = rest;
        effects.extend(early.into_iter().map(|e| Effect::RemoteIce {
            session: e.session,
            candidate: e.candidate,
            mid: e.mid,
        }));
        effects
    }

    /// What `session`'s peer reported: signals to send and, for a channel
    /// message, the delivery.
    pub fn on_peer(
        &mut self,
        session: &str,
        event: PeerEvent,
        now: i64,
    ) -> (Vec<Effect>, Option<Delivery>) {
        let Some(line) = self.sessions.get_mut(session) else {
            return (Vec::new(), None);
        };
        let reply = |kind: SignalType, line: &Session| {
            let mut signal = Signal::new(kind, Role::Viewer, &self.device, now);
            signal.to = line.host.clone();
            signal.session = session.to_string();
            signal
        };
        match event {
            PeerEvent::Answer(sdp) => {
                let mut signal = reply(SignalType::Answer, line);
                signal.sdp = sdp;
                let space = line.space.clone();
                (vec![Effect::Publish { signal, space }], None)
            }
            PeerEvent::Ice { candidate, mid } => {
                let mut signal = reply(SignalType::Ice, line);
                signal.candidate = candidate;
                signal.mid = mid;
                let space = line.space.clone();
                (vec![Effect::Publish { signal, space }], None)
            }
            PeerEvent::Open => {
                line.open = true;
                (Vec::new(), None)
            }
            PeerEvent::Message(text) => {
                let space = line.space.clone();
                (Vec::new(), Some(Delivery { text, space }))
            }
            PeerEvent::Closed => {
                let signal = reply(SignalType::Bye, line);
                let space = line.space.clone();
                self.sessions.remove(session);
                (
                    vec![
                        Effect::Publish { signal, space },
                        Effect::Close {
                            session: session.to_string(),
                        },
                    ],
                    None,
                )
            }
        }
    }

    /// The app is closing: a bye to every host, every line closed.
    pub fn shutdown(&mut self, now: i64) -> Vec<Effect> {
        let mut effects = Vec::new();
        for (session, line) in self.sessions.drain() {
            let mut signal = Signal::new(SignalType::Bye, Role::Viewer, &self.device, now);
            signal.to = line.host;
            signal.session = session.clone();
            effects.push(Effect::Publish {
                signal,
                space: line.space,
            });
            effects.push(Effect::Close { session });
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_host(kind: SignalType, to: &str, session: &str) -> Signal {
        let mut s = Signal::new(kind, Role::Host, "host-1", 1);
        s.to = to.into();
        s.session = session.into();
        s.sdp = "v=0".into();
        s.candidate = "candidate:1 1 udp 1 192.0.2.1 9 typ host".into();
        s.mid = "0".into();
        s
    }

    fn published(effects: &[Effect]) -> Vec<(SignalType, String, Scope)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Publish { signal, space } => {
                    Some((signal.kind, signal.to.clone(), space.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hello_on_start_then_every_minute() {
        let mut v = Viewer::new("me");
        let first = v.tick(1_000);
        assert_eq!(
            published(&first),
            [(SignalType::Hello, String::new(), None)]
        );
        let Effect::Publish { signal, .. } = &first[0] else {
            unreachable!()
        };
        assert_eq!((signal.from.as_str(), signal.role), ("me", Role::Viewer));
        assert!(v.tick(1_000 + HELLO_EVERY_MS - 1).is_empty(), "not yet");
        assert_eq!(v.next_wake(), 1_000 + HELLO_EVERY_MS);
        assert_eq!(published(&v.tick(1_000 + HELLO_EVERY_MS)).len(), 1);
        assert!(v.tick(1_000 + HELLO_EVERY_MS + 5).is_empty());
    }

    #[test]
    fn an_offer_to_this_device_is_answered_in_its_scope() {
        let mut v = Viewer::new("me");
        let space = Some("sp".to_string());
        let effects = v.on_signal(from_host(SignalType::Offer, "me", "s1"), space.clone());
        assert_eq!(
            effects,
            [Effect::Answer {
                session: "s1".into(),
                offer: "v=0".into()
            }]
        );
        let (effects, _) = v.on_peer("s1", PeerEvent::Answer("answer-sdp".into()), 2);
        let Effect::Publish {
            signal,
            space: sent_in,
        } = &effects[0]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(signal.kind, SignalType::Answer);
        assert_eq!(
            (signal.to.as_str(), signal.session.as_str()),
            ("host-1", "s1")
        );
        assert_eq!(
            (signal.sdp.as_str(), signal.role),
            ("answer-sdp", Role::Viewer)
        );
        assert_eq!(sent_in, &space);
        let (effects, _) = v.on_peer(
            "s1",
            PeerEvent::Ice {
                candidate: "c".into(),
                mid: "0".into(),
            },
            3,
        );
        assert_eq!(
            published(&effects),
            [(SignalType::Ice, "host-1".into(), space)]
        );
    }

    #[test]
    fn signals_not_for_this_device_or_from_itself_are_ignored() {
        let mut v = Viewer::new("me");
        assert!(
            v.on_signal(from_host(SignalType::Offer, "other", "s1"), None)
                .is_empty(),
            "addressed to another device"
        );
        let mut own = from_host(SignalType::Offer, "me", "s2");
        own.from = "me".into();
        assert!(v.on_signal(own, None).is_empty(), "our own echo");
        let mut viewer = from_host(SignalType::Offer, "me", "s3");
        viewer.role = Role::Viewer;
        assert!(
            v.on_signal(viewer, None).is_empty(),
            "a viewer never offers"
        );
        let mut hello = Signal::new(SignalType::Hello, Role::Viewer, "phone", 1);
        hello.to = "me".into();
        assert!(v.on_signal(hello, None).is_empty(), "hellos are for hosts");
        assert!(
            v.on_signal(from_host(SignalType::Ice, "other", "s1"), None)
                .is_empty()
        );
        assert!(
            v.on_signal(from_host(SignalType::Bye, "other", "s1"), None)
                .is_empty()
        );
        assert_eq!(v.sessions.len(), 0);
    }

    #[test]
    fn ice_before_its_offer_waits_and_bye_closes() {
        let mut v = Viewer::new("me");
        assert!(
            v.on_signal(from_host(SignalType::Ice, "me", "s1"), None)
                .is_empty()
        );
        let effects = v.on_signal(from_host(SignalType::Offer, "me", "s1"), None);
        assert!(matches!(&effects[1], Effect::RemoteIce { session, .. } if session == "s1"));
        assert_eq!(
            v.on_signal(from_host(SignalType::Ice, "me", "s1"), None)
                .len(),
            1,
            "ice for a known session goes straight through"
        );
        let mut stranger = from_host(SignalType::Bye, "me", "s1");
        stranger.from = "host-2".into();
        assert!(
            v.on_signal(stranger, None).is_empty(),
            "only its host ends it"
        );
        assert_eq!(
            v.on_signal(from_host(SignalType::Bye, "me", "s1"), None),
            [Effect::Close {
                session: "s1".into()
            }]
        );
        let (effects, delivery) = v.on_peer("s1", PeerEvent::Message("x".into()), 5);
        assert!(
            effects.is_empty() && delivery.is_none(),
            "a closed line delivers nothing"
        );
    }

    #[test]
    fn a_hosts_new_offer_replaces_its_line_and_messages_carry_the_scope() {
        let mut v = Viewer::new("me");
        v.on_signal(from_host(SignalType::Offer, "me", "s1"), None);
        assert!(
            v.on_signal(from_host(SignalType::Offer, "me", "s1"), None)
                .is_empty(),
            "the same offer twice"
        );
        let effects = v.on_signal(from_host(SignalType::Offer, "me", "s2"), Some("sp".into()));
        assert_eq!(
            effects[0],
            Effect::Close {
                session: "s1".into()
            }
        );
        v.on_peer("s2", PeerEvent::Open, 3);
        assert_eq!(v.open_lines(), 1);
        let (_, delivery) = v.on_peer("s2", PeerEvent::Message("{}".into()), 4);
        assert_eq!(
            delivery,
            Some(Delivery {
                text: "{}".into(),
                space: Some("sp".into())
            })
        );
        let (effects, _) = v.on_peer("s2", PeerEvent::Closed, 5);
        assert_eq!(
            published(&effects),
            [(SignalType::Bye, "host-1".into(), Some("sp".into()))]
        );
        assert_eq!(v.open_lines(), 0);
        v.on_signal(from_host(SignalType::Offer, "me", "s3"), None);
        let effects = v.shutdown(9);
        assert_eq!(
            published(&effects),
            [(SignalType::Bye, "host-1".into(), None)]
        );
    }
}
