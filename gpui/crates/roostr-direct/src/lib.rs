//! Direct lines: an agent's live progress (`roostr_core::live`) over a
//! WebRTC data channel from the computer running the agent (the harness,
//! the *host*) to each open app device (a *viewer*).
//!
//! Nostr only introduces the two: ephemeral kind-25079 signals ([`signal`]),
//! sealed and routed like the kind-25078 live events. A viewer says hello on
//! start and every minute ([`viewer::HELLO_EVERY_MS`]); a host without a
//! line to it offers one, the viewer answers, both trickle ICE candidates,
//! and the host sends every live event on the `roostr-live` channel - one
//! payload per message - instead of through the relay. Without a line the
//! relay copy still arrives; the app drops duplicates by (run, seq).
//!
//! [`Direct`] runs a [`viewer::Viewer`] with this platform's peers
//! ([`platform_peers`]: str0m natively, the browser's `RTCPeerConnection`
//! in wasm). The backend carries the signals both ways and gets the channel
//! messages back as [`Delivery`]s.

pub mod signal;
pub mod viewer;

#[cfg(not(target_family = "wasm"))]
pub mod native;
#[cfg(target_family = "wasm")]
pub mod web;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::{FutureExt, StreamExt, pin_mut, select};
use roostr_rt::{BoxFuture, MaybeSend, MaybeSync};
use serde_json::Value as Json;

pub use signal::{Role, Signal, SignalType};
pub use viewer::{Delivery, Effect, PeerEvent, Scope, Viewer};

/// The data channel's label.
pub const CHANNEL_LABEL: &str = "roostr-live";
/// The only ICE server: no TURN.
pub const STUN_SERVER: &str = "stun:stun.l.google.com:19302";

/// Where a peer's events go, tagged with its session.
#[derive(Clone)]
pub struct PeerEvents {
    session: String,
    tx: UnboundedSender<(String, PeerEvent)>,
}

impl PeerEvents {
    pub fn new(session: impl Into<String>, tx: UnboundedSender<(String, PeerEvent)>) -> PeerEvents {
        PeerEvents {
            session: session.into(),
            tx,
        }
    }

    pub fn send(&self, event: PeerEvent) {
        let _ = self.tx.unbounded_send((self.session.clone(), event));
    }
}

/// One answering peer; dropping it closes the line.
pub trait Peer: MaybeSend {
    /// A candidate the host trickled.
    fn remote_ice(&self, candidate: &str, mid: &str);
}

/// Makes this platform's answering peers.
pub trait PeerFactory: MaybeSend + MaybeSync + 'static {
    /// A peer answering `offer`: it reports its answer, candidates, channel
    /// messages and end to `events` (a failure is a [`PeerEvent::Closed`]).
    fn answer(&self, offer: &str, events: PeerEvents) -> Box<dyn Peer>;
}

/// This platform's peers.
pub fn platform_peers() -> Arc<dyn PeerFactory> {
    #[cfg(not(target_family = "wasm"))]
    {
        Arc::new(native::NativePeers::new())
    }
    #[cfg(target_family = "wasm")]
    {
        Arc::new(web::WebPeers)
    }
}

/// Sends one signal payload in a scope (`None` = personal; a hello goes out
/// in every scope whatever it says). Called one at a time, in order.
#[cfg(not(target_family = "wasm"))]
pub type Publish = Arc<dyn Fn(Json, Scope) -> BoxFuture<Result<(), String>> + Send + Sync>;
#[cfg(target_family = "wasm")]
pub type Publish = Arc<dyn Fn(Json, Scope) -> BoxFuture<Result<(), String>>>;
/// Takes one channel message.
#[cfg(not(target_family = "wasm"))]
pub type Deliver = Arc<dyn Fn(Delivery) + Send + Sync>;
#[cfg(target_family = "wasm")]
pub type Deliver = Arc<dyn Fn(Delivery)>;

/// A running viewer. Dropping it says bye to every host and closes the lines.
pub struct Direct {
    tx: UnboundedSender<(Json, Scope)>,
}

impl Direct {
    /// Start the viewer of `device`: it says hello now and every minute
    /// through `publish`, answers offers with `peers` and hands each channel
    /// message to `deliver`.
    pub fn start(
        device: impl Into<String>,
        peers: Arc<dyn PeerFactory>,
        publish: Publish,
        deliver: Deliver,
    ) -> Direct {
        let (tx, rx) = unbounded();
        let (out_tx, out_rx) = unbounded();
        spawn(publisher(out_rx, publish));
        spawn(run(Viewer::new(device), rx, peers, out_tx, deliver));
        Direct { tx }
    }

    /// A signal payload heard in `space` (`None` = personal).
    pub fn signal(&self, payload: Json, space: Scope) {
        let _ = self.tx.unbounded_send((payload, space));
    }
}

/// Signals go out one at a time, in order (the answer before its candidates).
async fn publisher(mut rx: UnboundedReceiver<(Json, Scope)>, publish: Publish) {
    while let Some((payload, space)) = rx.next().await {
        if let Err(error) = publish(payload, space).await {
            log::debug!("[direct] signal not sent: {error}");
        }
    }
}

async fn run(
    mut viewer: Viewer,
    mut inputs: UnboundedReceiver<(Json, Scope)>,
    factory: Arc<dyn PeerFactory>,
    out: UnboundedSender<(Json, Scope)>,
    deliver: Deliver,
) {
    let (peer_tx, mut peer_rx) = unbounded();
    let mut peers: HashMap<String, Box<dyn Peer>> = HashMap::new();
    let apply = |effects: Vec<Effect>, peers: &mut HashMap<String, Box<dyn Peer>>| {
        for effect in effects {
            match effect {
                Effect::Publish { signal, space } => {
                    let _ = out.unbounded_send((signal.to_json(), space));
                }
                Effect::Answer { session, offer } => {
                    let events = PeerEvents::new(session.clone(), peer_tx.clone());
                    peers.insert(session, factory.answer(&offer, events));
                }
                Effect::RemoteIce {
                    session,
                    candidate,
                    mid,
                } => {
                    if let Some(peer) = peers.get(&session) {
                        peer.remote_ice(&candidate, &mid);
                    }
                }
                Effect::Close { session } => {
                    peers.remove(&session);
                }
            }
        }
    };
    loop {
        apply(viewer.tick(now_ms()), &mut peers);
        let wait = (viewer.next_wake() - now_ms()).max(0) as u64;
        let woke = {
            let input = inputs.next().fuse();
            let peer = peer_rx.next().fuse();
            let timer = sleep(Duration::from_millis(wait)).fuse();
            pin_mut!(input, peer, timer);
            select! {
                input = input => match input {
                    Some(input) => Woke::Signal(input),
                    None => break,
                },
                event = peer => Woke::Peer(event),
                _ = timer => Woke::Timer,
            }
        };
        match woke {
            Woke::Signal((payload, space)) => {
                if let Some(signal) = Signal::parse(&payload) {
                    apply(viewer.on_signal(signal, space), &mut peers);
                }
            }
            Woke::Peer(Some((session, event))) => {
                let (effects, delivery) = viewer.on_peer(&session, event, now_ms());
                apply(effects, &mut peers);
                if let Some(delivery) = delivery {
                    deliver(delivery);
                }
            }
            Woke::Peer(None) | Woke::Timer => {}
        }
    }
    apply(viewer.shutdown(now_ms()), &mut peers);
}

enum Woke {
    Signal((Json, Scope)),
    Peer(Option<(String, PeerEvent)>),
    Timer,
}

fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Run `future` in the background: a thread natively, the page's event loop
/// in the browser.
fn spawn(future: impl Future<Output = ()> + MaybeSend + 'static) {
    #[cfg(not(target_family = "wasm"))]
    {
        let spawned = std::thread::Builder::new()
            .name("roostr-direct".into())
            .spawn(move || async_io::block_on(future));
        if let Err(error) = spawned {
            log::error!("[direct] cannot start: {error}");
        }
    }
    #[cfg(target_family = "wasm")]
    wasm_bindgen_futures::spawn_local(future);
}

async fn sleep(duration: Duration) {
    #[cfg(not(target_family = "wasm"))]
    {
        async_io::Timer::after(duration).await;
    }
    #[cfg(target_family = "wasm")]
    gloo_timers::future::sleep(duration).await;
}
