//! A NIP-01 / NIP-77 relay client (Relay.swift's `WebSocketRelay`, the parts
//! of nostr-tools' `SimplePool` sync.ts uses).
//!
//! One [`Relay`] owns at most one socket, dialed on first use. A failed
//! socket is dropped and every request waiting on it fails, so the next call
//! dials again; reconnecting a live subscription is [`live`]'s job. Native
//! sockets are pinged every [`RelayOptions::keepalive`] and failed when no
//! pong arrives within [`RelayOptions::pong_timeout`]: a socket that outlived
//! a suspension or a network change can otherwise wait forever.
//!
//! Received events are signature-checked and matched against their filters
//! before delivery (nostr-tools does the same), unless
//! [`RelayOptions::verify_events`] is off.
//!
//! The hosts never answer NIP-42 `AUTH` challenges and neither does this
//! client: Roostr's relay serves its kinds without authentication.

mod live;
mod pool;
mod transport;

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use futures::channel::{mpsc, oneshot};
use serde_json::Value;

use crate::event::Event;
use crate::filter::Filter;
use crate::negentropy::{Negentropy, NegentropyStorage};
use crate::shared::{Locked, Ptr, WeakPtr, downgrade};
use crate::{random, time};

pub use live::{BACKOFF_CEILING, BACKOFF_FLOOR, LiveItem, live};
pub use pool::{PUBLISH_SPACING, Pacer, publish_any, query_many};

/// Largest outgoing negentropy message in bytes: 500k hex characters, well
/// under the relay's 1 MiB message cap (sync.ts `NEG_FRAME_LIMIT`).
pub const NEG_FRAME_LIMIT: usize = 250_000;
/// No NEG-MSG within this window: before the first answer the relay is
/// taken not to speak NIP-77 (sync.ts `NEG_TIMEOUT_MS`).
pub const NEG_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a relay request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayError {
    /// No EOSE / OK / NEG-MSG within the caller's deadline.
    Timeout,
    /// The relay ended the subscription (`CLOSED`).
    Closed(String),
    /// `OK false` for a published event.
    Rejected(String),
    /// No pong within the keepalive deadline: the socket was presumed dead.
    Unresponsive,
    /// `NEG-ERR` for a reconciliation, with the relay's reason.
    Negentropy(String),
    /// The relay answered `NEG-OPEN` with a `NOTICE`: it does not speak NIP-77.
    Unsupported(String),
    /// The socket could not be opened, or failed.
    Transport(String),
    /// [`Relay::disconnect`] dropped the socket.
    Disconnected,
    /// A malformed negentropy message (from the relay, or our own set).
    Protocol(String),
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("relay did not answer in time"),
            Self::Closed(m) => write!(f, "relay closed the subscription: {m}"),
            Self::Rejected(m) => write!(f, "relay rejected the event: {m}"),
            Self::Unresponsive => f.write_str("relay stopped answering pings"),
            Self::Negentropy(m) => write!(f, "NEG-ERR {m}"),
            Self::Unsupported(m) => write!(f, "relay does not speak NIP-77: {m}"),
            Self::Transport(m) => write!(f, "relay connection failed: {m}"),
            Self::Disconnected => f.write_str("relay disconnected"),
            Self::Protocol(m) => write!(f, "negentropy: {m}"),
        }
    }
}

impl std::error::Error for RelayError {}

/// A reconciliation that did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileError {
    pub error: RelayError,
    /// Whether the relay had answered NEG-OPEN with a NEG-MSG.
    pub answered: bool,
}

impl ReconcileError {
    /// The relay never answered NEG-OPEN with a NEG-MSG and refused (NEG-ERR,
    /// NOTICE) or stayed silent: it does not speak NIP-77, so walk its
    /// history page by page instead and don't ask it again this session
    /// (sync.ts `NegentropyFallback(…, unsupported = true)`).
    pub fn relay_refused(&self) -> bool {
        !self.answered
            && matches!(
                self.error,
                RelayError::Timeout | RelayError::Negentropy(_) | RelayError::Unsupported(_)
            )
    }
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ReconcileError {}

/// A completed reconciliation (hex ids).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Held by the relay, missing here: fetch them with `REQ {ids}`.
    pub need: Vec<String>,
    /// Held here, missing on the relay.
    pub have: Vec<String>,
}

/// Connection behavior.
#[derive(Clone, Debug)]
pub struct RelayOptions {
    /// Ping interval of an open native socket.
    pub keepalive: Duration,
    /// How long a ping may wait for its pong.
    pub pong_timeout: Duration,
    /// How long dialing (TCP, TLS, websocket handshake) may take.
    pub connect_timeout: Duration,
    /// Drop received events whose id or signature does not verify, or that
    /// match none of their subscription's filters.
    pub verify_events: bool,
}

impl Default for RelayOptions {
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(25),
            pong_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(15),
            verify_events: true,
        }
    }
}

/// What a [`Subscription`] delivers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscriptionItem {
    Event(Event),
    /// The relay sent everything it stored; what follows is live.
    Eose,
    /// An event of a subscription with an `after` filter, with its stream
    /// seq (RoostrRelay `["EVENT", sub, event, {"seq": n}]`).
    Sequenced(Event, u64),
    /// [`SubscriptionItem::Eose`] carrying the stream heads
    /// (`{"head", "oldest"}`, or one such object per stream).
    Cursor(Value),
}

struct QuerySlot {
    filters: Vec<Filter>,
    events: Vec<Event>,
    tx: oneshot::Sender<Result<Vec<Event>, RelayError>>,
}

struct SubSlot {
    filters: Vec<Filter>,
    tx: mpsc::UnboundedSender<Result<SubscriptionItem, RelayError>>,
}

struct NegSlot {
    /// NEG-OPEN is still unanswered: a NOTICE now means the verb is unknown.
    opening: bool,
    tx: Option<oneshot::Sender<Result<String, RelayError>>>,
}

#[derive(Default)]
struct State {
    conn: Option<transport::Conn>,
    /// Bumped per dial; frames and failures of older sockets are ignored.
    generation: u64,
    queries: HashMap<String, QuerySlot>,
    subs: HashMap<String, SubSlot>,
    publishes: HashMap<String, oneshot::Sender<Result<(), RelayError>>>,
    negs: HashMap<String, NegSlot>,
}

pub(crate) struct Inner {
    url: String,
    options: RelayOptions,
    state: Locked<State>,
}

/// A client for one relay; cheap to clone (clones share the socket).
#[derive(Clone)]
pub struct Relay {
    inner: Ptr<Inner>,
}

impl fmt::Debug for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Relay")
            .field("url", &self.inner.url)
            .finish_non_exhaustive()
    }
}

fn subscription_id() -> String {
    hex::encode(random::bytes::<8>())
}

fn frame(items: impl IntoIterator<Item = Value>) -> String {
    Value::Array(items.into_iter().collect()).to_string()
}

fn req_frame(id: &str, filters: &[Filter]) -> String {
    frame(
        [Value::from("REQ"), Value::from(id)].into_iter().chain(
            filters
                .iter()
                .map(|f| serde_json::to_value(f).expect("a filter always serializes")),
        ),
    )
}

fn close_frame(verb: &str, id: &str) -> String {
    frame([Value::from(verb), Value::from(id)])
}

impl Relay {
    /// A client for `url` (`wss://…` or `ws://…`) with default options. No
    /// socket is opened until the first request.
    pub fn new(url: impl Into<String>) -> Self {
        Self::with_options(url, RelayOptions::default())
    }

    pub fn with_options(url: impl Into<String>, options: RelayOptions) -> Self {
        Self {
            inner: Ptr::new(Inner {
                url: url.into(),
                options,
                state: Locked::new(State::default()),
            }),
        }
    }

    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Whether a socket is open or opening.
    pub fn is_connected(&self) -> bool {
        self.inner.state.with(|st| st.conn.is_some())
    }

    /// Drop the socket; everything waiting on it fails with
    /// [`RelayError::Disconnected`]. The next request dials again.
    pub fn disconnect(&self) {
        let generation = self
            .inner
            .state
            .with(|st| st.conn.as_ref().map(|_| st.generation));
        if let Some(generation) = generation {
            self.inner.fail(generation, RelayError::Disconnected);
        }
    }

    /// `REQ` until `EOSE`: everything the relay stores for `filters`. A
    /// `CLOSED` or a missed deadline is an error, never a short answer (only
    /// a real EOSE proves the relay has nothing more).
    pub async fn query(
        &self,
        filters: Vec<Filter>,
        timeout: Duration,
    ) -> Result<Vec<Event>, RelayError> {
        let id = subscription_id();
        let (tx, rx) = oneshot::channel();
        let text = req_frame(&id, &filters);
        let guard = Guard {
            inner: &self.inner,
            id: &id,
            kind: SlotKind::Query,
        };
        self.inner.state.with(|st| {
            st.queries.insert(
                id.clone(),
                QuerySlot {
                    filters,
                    events: Vec::new(),
                    tx,
                },
            );
            Inner::send_locked(&self.inner, st, text)
        })?;
        match time::timeout(timeout, rx).await {
            Some(Ok(result)) => {
                guard.disarm();
                result
            }
            Some(Err(oneshot::Canceled)) => Err(RelayError::Disconnected),
            None => Err(RelayError::Timeout),
        }
    }

    /// A live `REQ`: stored events, [`SubscriptionItem::Eose`], then new
    /// events as they arrive. The stream ends after one `Err` (CLOSED or a
    /// failed socket). Dropping it sends `CLOSE`.
    pub fn subscribe(&self, filters: Vec<Filter>) -> Subscription {
        let id = subscription_id();
        let (tx, rx) = mpsc::unbounded();
        let text = req_frame(&id, &filters);
        let sent = self.inner.state.with(|st| {
            st.subs.insert(
                id.clone(),
                SubSlot {
                    filters,
                    tx: tx.clone(),
                },
            );
            let sent = Inner::send_locked(&self.inner, st, text);
            if sent.is_err() {
                st.subs.remove(&id);
            }
            sent
        });
        if let Err(error) = sent {
            let _ = tx.unbounded_send(Err(error));
        }
        Subscription {
            id,
            inner: downgrade(&self.inner),
            rx,
        }
    }

    /// `EVENT` and wait for the relay's `OK`.
    pub async fn publish(&self, event: &Event, timeout: Duration) -> Result<(), RelayError> {
        let (tx, rx) = oneshot::channel();
        let text = frame([
            Value::from("EVENT"),
            serde_json::to_value(event).expect("an event always serializes"),
        ]);
        let guard = Guard {
            inner: &self.inner,
            id: &event.id,
            kind: SlotKind::Publish,
        };
        self.inner.state.with(|st| {
            st.publishes.insert(event.id.clone(), tx);
            Inner::send_locked(&self.inner, st, text)
        })?;
        match time::timeout(timeout, rx).await {
            Some(Ok(result)) => {
                guard.disarm();
                result
            }
            Some(Err(oneshot::Canceled)) => Err(RelayError::Disconnected),
            None => Err(RelayError::Timeout),
        }
    }

    /// NIP-77: reconcile `storage` (this device's events for `filter`)
    /// against the relay's set, as the initiator, to completion. `timeout`
    /// bounds each wait for a NEG-MSG ([`NEG_TIMEOUT`] in the hosts). The
    /// session is closed on every exit the relay has not closed itself.
    pub async fn reconcile(
        &self,
        filter: &Filter,
        storage: NegentropyStorage,
        timeout: Duration,
    ) -> Result<Reconciled, ReconcileError> {
        let protocol = |e: crate::negentropy::NegentropyError| ReconcileError {
            error: RelayError::Protocol(e.to_string()),
            answered: false,
        };
        let mut negentropy = Negentropy::new(storage, NEG_FRAME_LIMIT).map_err(protocol)?;
        let mut message = negentropy.initiate().map_err(protocol)?;
        let id = subscription_id();
        let filter_value = serde_json::to_value(filter).expect("a filter always serializes");
        let guard = Guard {
            inner: &self.inner,
            id: &id,
            kind: SlotKind::Neg,
        };
        let mut out = Reconciled::default();
        let mut answered = false;
        let fail = |error: RelayError, answered: bool| {
            // NEG-ERR and CLOSED end the session relay-side; anything else gets a NEG-CLOSE.
            if !matches!(error, RelayError::Negentropy(_) | RelayError::Closed(_)) {
                self.inner.send_if_connected(close_frame("NEG-CLOSE", &id));
            }
            ReconcileError { error, answered }
        };
        loop {
            let (tx, rx) = oneshot::channel();
            let hex = hex::encode(&message);
            let text = if answered {
                frame([
                    Value::from("NEG-MSG"),
                    Value::from(id.as_str()),
                    Value::from(hex),
                ])
            } else {
                frame([
                    Value::from("NEG-OPEN"),
                    Value::from(id.as_str()),
                    filter_value.clone(),
                    Value::from(hex),
                ])
            };
            let sent = self.inner.state.with(|st| {
                st.negs.insert(
                    id.clone(),
                    NegSlot {
                        opening: !answered,
                        tx: Some(tx),
                    },
                );
                Inner::send_locked(&self.inner, st, text)
            });
            if let Err(error) = sent {
                return Err(ReconcileError { error, answered });
            }
            let reply = match time::timeout(timeout, rx).await {
                Some(Ok(Ok(reply))) => reply,
                Some(Ok(Err(error))) => return Err(fail(error, answered)),
                Some(Err(oneshot::Canceled)) => {
                    return Err(fail(RelayError::Disconnected, answered));
                }
                None => return Err(fail(RelayError::Timeout, answered)),
            };
            answered = true;
            let bytes = hex::decode(&reply)
                .map_err(|_| fail(RelayError::Protocol("malformed NEG-MSG".into()), true))?;
            let step = negentropy
                .reconcile(&bytes)
                .map_err(|e| fail(RelayError::Protocol(e.to_string()), true))?;
            out.need.extend(step.need_ids);
            out.have.extend(step.have_ids);
            match step.output {
                Some(next) => message = next,
                None => break,
            }
        }
        drop(guard);
        self.inner.send_if_connected(close_frame("NEG-CLOSE", &id));
        Ok(out)
    }
}

#[derive(Clone, Copy)]
enum SlotKind {
    Query,
    Publish,
    Neg,
}

/// Removes a request's slot when its future ends or is dropped early, so an
/// abandoned request never leaks its entry (a query also sends `CLOSE`).
struct Guard<'a> {
    inner: &'a Ptr<Inner>,
    id: &'a str,
    kind: SlotKind,
}

impl Guard<'_> {
    /// The slot was settled by the relay: nothing to remove.
    fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let close = self.inner.state.with(|st| match self.kind {
            SlotKind::Query => st.queries.remove(self.id).is_some(),
            SlotKind::Publish => {
                st.publishes.remove(self.id);
                false
            }
            SlotKind::Neg => {
                st.negs.remove(self.id);
                false
            }
        });
        if close {
            self.inner.send_if_connected(close_frame("CLOSE", self.id));
        }
    }
}

/// A live subscription; see [`Relay::subscribe`].
pub struct Subscription {
    id: String,
    inner: WeakPtr<Inner>,
    rx: mpsc::UnboundedReceiver<Result<SubscriptionItem, RelayError>>,
}

impl Subscription {
    /// The NIP-01 subscription id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl Stream for Subscription {
    type Item = Result<SubscriptionItem, RelayError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if inner.state.with(|st| st.subs.remove(&self.id).is_some()) {
            inner.send_if_connected(close_frame("CLOSE", &self.id));
        }
    }
}

impl Inner {
    /// Send `text`, dialing first when there is no socket. Called with the
    /// state locked so a request's slot and its frame land on the same socket.
    fn send_locked(this: &Ptr<Inner>, st: &mut State, text: String) -> Result<(), RelayError> {
        if st.conn.is_none() {
            st.generation += 1;
            st.conn = Some(transport::Conn::dial(this, st.generation)?);
        }
        let conn = st.conn.as_mut().expect("dialed above");
        conn.send(text)
    }

    /// Send on the open socket only (CLOSE, NEG-CLOSE: never worth a dial).
    fn send_if_connected(&self, text: String) {
        self.state.with(|st| {
            if let Some(conn) = st.conn.as_mut() {
                let _ = conn.send(text);
            }
        });
    }

    /// Drop socket `generation` if it is still current and fail everything
    /// waiting on it.
    pub(crate) fn fail(&self, generation: u64, error: RelayError) {
        let dropped = self.state.with(|st| {
            if st.generation != generation || st.conn.is_none() {
                return None;
            }
            let conn = st.conn.take();
            for (_, slot) in st.queries.drain() {
                let _ = slot.tx.send(Err(error.clone()));
            }
            for (_, slot) in st.subs.drain() {
                let _ = slot.tx.unbounded_send(Err(error.clone()));
            }
            for (_, tx) in st.publishes.drain() {
                let _ = tx.send(Err(error.clone()));
            }
            for slot in st.negs.values_mut() {
                if let Some(tx) = slot.tx.take() {
                    let _ = tx.send(Err(error.clone()));
                }
            }
            conn
        });
        if let Some(conn) = dropped {
            log::debug!("relay {}: {error}", self.url);
            conn.close();
        }
    }

    /// One text frame from socket `generation`.
    pub(crate) fn handle(&self, generation: u64, text: &str) {
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let mut items = items.into_iter();
        let Some(Value::String(verb)) = items.next() else {
            return;
        };
        let mut next_str = || match items.next() {
            Some(Value::String(s)) => Some(s),
            _ => None,
        };
        match verb.as_str() {
            "EVENT" => {
                let Some(id) = next_str() else { return };
                let Some(event) = items
                    .next()
                    .and_then(|v| serde_json::from_value::<Event>(v).ok())
                else {
                    return;
                };
                let seq = items
                    .next()
                    .and_then(|extra| extra.get("seq").and_then(Value::as_u64));
                if self.options.verify_events && !event.verify() {
                    log::debug!(
                        "relay {}: dropped event {} with a bad id or signature",
                        self.url,
                        event.id
                    );
                    return;
                }
                self.with_current(generation, |st| {
                    if let Some(slot) = st.queries.get_mut(&id) {
                        if self.accepts(&slot.filters, &event) {
                            slot.events.push(event);
                        }
                    } else if let Some(slot) = st.subs.get(&id)
                        && self.accepts(&slot.filters, &event)
                    {
                        let item = match seq {
                            Some(seq) => SubscriptionItem::Sequenced(event, seq),
                            None => SubscriptionItem::Event(event),
                        };
                        let _ = slot.tx.unbounded_send(Ok(item));
                    }
                });
            }
            "EOSE" => {
                let Some(id) = next_str() else { return };
                let cursor = items.next().filter(Value::is_object);
                let close = self.with_current(generation, |st| {
                    if let Some(slot) = st.queries.remove(&id) {
                        let _ = slot.tx.send(Ok(slot.events));
                        return true;
                    }
                    if let Some(slot) = st.subs.get(&id) {
                        let item = match cursor {
                            Some(cursor) => SubscriptionItem::Cursor(cursor),
                            None => SubscriptionItem::Eose,
                        };
                        let _ = slot.tx.unbounded_send(Ok(item));
                    }
                    false
                });
                if close == Some(true) {
                    self.send_if_connected(close_frame("CLOSE", &id));
                }
            }
            "CLOSED" => {
                let Some(id) = next_str() else { return };
                let message = next_str().unwrap_or_default();
                self.with_current(generation, |st| {
                    let error = RelayError::Closed(message);
                    if let Some(slot) = st.queries.remove(&id) {
                        let _ = slot.tx.send(Err(error.clone()));
                    }
                    if let Some(slot) = st.subs.remove(&id) {
                        let _ = slot.tx.unbounded_send(Err(error.clone()));
                    }
                    if let Some(tx) = st.negs.get_mut(&id).and_then(|slot| slot.tx.take()) {
                        let _ = tx.send(Err(error));
                    }
                });
            }
            "OK" => {
                let Some(id) = next_str() else { return };
                let accepted = matches!(items.next(), Some(Value::Bool(true)));
                let message = match items.next() {
                    Some(Value::String(s)) => s,
                    _ => String::new(),
                };
                self.with_current(generation, |st| {
                    if let Some(tx) = st.publishes.remove(&id) {
                        let _ = tx.send(if accepted {
                            Ok(())
                        } else {
                            Err(RelayError::Rejected(message))
                        });
                    }
                });
            }
            "NOTICE" => {
                let message = next_str().unwrap_or_default();
                log::debug!("relay {}: NOTICE {message}", self.url);
                // NOTICE names no subscription; it only answers an unanswered NEG-OPEN.
                self.with_current(generation, |st| {
                    for slot in st.negs.values_mut().filter(|slot| slot.opening) {
                        if let Some(tx) = slot.tx.take() {
                            let _ = tx.send(Err(RelayError::Unsupported(message.clone())));
                        }
                    }
                });
            }
            "NEG-MSG" => {
                let (Some(id), Some(hex)) = (next_str(), next_str()) else {
                    return;
                };
                self.with_current(generation, |st| {
                    if let Some(slot) = st.negs.get_mut(&id) {
                        slot.opening = false;
                        if let Some(tx) = slot.tx.take() {
                            let _ = tx.send(Ok(hex));
                        }
                    }
                });
            }
            "NEG-ERR" => {
                let Some(id) = next_str() else { return };
                let reason = next_str().unwrap_or_default();
                self.with_current(generation, |st| {
                    if let Some(tx) = st.negs.get_mut(&id).and_then(|slot| slot.tx.take()) {
                        let _ = tx.send(Err(RelayError::Negentropy(reason)));
                    }
                });
            }
            _ => {}
        }
    }

    /// Run `f` on the state if socket `generation` is still the current one.
    fn with_current<R>(&self, generation: u64, f: impl FnOnce(&mut State) -> R) -> Option<R> {
        self.state
            .with(|st| (st.generation == generation && st.conn.is_some()).then(|| f(st)))
    }

    fn accepts(&self, filters: &[Filter], event: &Event) -> bool {
        !self.options.verify_events || filters.iter().any(|f| f.matches(event))
    }
}
