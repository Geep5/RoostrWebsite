//! Two native peers in one process: a host offering like the harness does
//! and a running [`Direct`] viewer, introduced through an in-memory signal
//! bus; one live event crosses the data channel.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::unbounded;
use parking_lot::Mutex;
use roostr_direct::native::NativePeers;
use roostr_direct::{Delivery, Direct, Peer, PeerEvent, PeerEvents, Role, Signal, SignalType};
use serde_json::{Value as Json, json};

const LIVE: &str = r#"{"v":1,"run":"r1","agent":"a1","object":"o1","thread":"__discussion__","seq":1,"kind":"text","text":"Hi","ts":1}"#;

/// A line that delivered one live event, kept open.
struct Line {
    _viewer: Direct,
    _host: roostr_direct::native::NativePeer,
    /// Every payload the viewer sent.
    outbox: Arc<Mutex<Vec<Json>>>,
}

impl Line {
    fn signals(&self) -> Vec<Signal> {
        self.outbox
            .lock()
            .iter()
            .filter_map(Signal::parse)
            .collect()
    }
}

/// Run a line between a host and a viewer both made by `peers`.
fn cross(peers: NativePeers) -> Line {
    // The bus: what the viewer publishes, in order.
    let outbox: Arc<Mutex<Vec<Json>>> = Arc::default();
    let delivered: Arc<Mutex<Vec<Delivery>>> = Arc::default();
    let sent = outbox.clone();
    let got = delivered.clone();
    let viewer = Direct::start(
        "viewer-1",
        Arc::new(peers.clone()),
        Arc::new(move |payload, _space| {
            sent.lock().push(payload);
            Box::pin(async { Ok(()) })
        }),
        Arc::new(move |delivery| got.lock().push(delivery)),
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let wait_for = |what: &str, check: &dyn Fn() -> bool| {
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    // The viewer says hello first.
    wait_for("the hello", &|| {
        outbox
            .lock()
            .iter()
            .any(|p| Signal::parse(p).is_some_and(|s| s.kind == SignalType::Hello))
    });

    // The host offers.
    let (host_tx, mut host_rx) = unbounded();
    let (host, offer_sdp) = peers
        .offer(PeerEvents::new("s1", host_tx))
        .expect("an offer");
    let from_host = |kind: SignalType| {
        let mut s = Signal::new(kind, Role::Host, "host-1", 1);
        s.to = "viewer-1".into();
        s.session = "s1".into();
        s
    };
    let mut offer = from_host(SignalType::Offer);
    offer.sdp = offer_sdp;
    viewer.signal(offer.to_json(), None);

    // Both trickle; the host takes the answer.
    let mut seen = 0;
    let mut open = false;
    while !open {
        assert!(Instant::now() < deadline, "the channel never opened");
        let signals: Vec<Signal> = outbox.lock()[seen..]
            .iter()
            .filter_map(Signal::parse)
            .collect();
        seen += signals.len();
        for s in signals {
            assert!(
                s.to.is_empty() || s.to == "host-1",
                "addressed to the host: {s:?}"
            );
            match s.kind {
                SignalType::Answer => {
                    assert_eq!((s.session.as_str(), s.role), ("s1", Role::Viewer));
                    host.accept_answer(&s.sdp);
                }
                SignalType::Ice => host.remote_ice(&s.candidate, &s.mid),
                _ => {}
            }
        }
        while let Ok((_, event)) = host_rx.try_recv() {
            match event {
                PeerEvent::Open => open = true,
                PeerEvent::Ice { candidate, mid } => {
                    let mut ice = from_host(SignalType::Ice);
                    ice.candidate = candidate;
                    ice.mid = mid;
                    viewer.signal(ice.to_json(), None);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    host.send(LIVE);
    wait_for("the delivery", &|| !delivered.lock().is_empty());
    assert_eq!(
        delivered.lock()[0],
        Delivery {
            text: LIVE.to_string(),
            space: None
        }
    );
    // The message is the live payload itself.
    let payload: Json = serde_json::from_str(&delivered.lock()[0].text).unwrap();
    assert_eq!(payload["run"], json!("r1"));
    Line {
        _viewer: viewer,
        _host: host,
        outbox,
    }
}

#[test]
fn a_live_event_crosses_a_native_loopback_line() {
    cross(NativePeers::loopback());
}

/// The app's own peers: the default route's address and a server-reflexive
/// candidate from the real STUN server, trickled. Needs the network.
#[test]
#[ignore]
fn a_live_event_crosses_a_line_on_the_network() {
    let line = cross(NativePeers::new());
    let deadline = Instant::now() + Duration::from_secs(5);
    let reflexive = || {
        line.signals()
            .iter()
            .any(|s| s.kind == SignalType::Ice && s.candidate.contains("typ srflx"))
    };
    while !reflexive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        reflexive(),
        "a server-reflexive candidate was trickled: {:?}",
        line.signals()
    );
}
