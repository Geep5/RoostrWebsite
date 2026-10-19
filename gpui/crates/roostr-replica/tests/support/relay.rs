//! An in-process NIP-01 / NIP-77 relay (roostr-nostr's test relay, grown
//! the parts RoostrRelay has that sync relies on): `limit` answered newest
//! first per filter, addressable kinds replaced per (pubkey, kind, d), and a
//! switch that refuses NEG-OPEN with a NOTICE like a relay without NIP-77.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_net::TcpListener;
use async_tungstenite::tungstenite::Message;
use futures::channel::mpsc;
use futures::{StreamExt, future};
use parking_lot::Mutex;
use roostr_nostr::negentropy::{Negentropy, NegentropyStorage};
use roostr_nostr::{Event, Filter};
use serde_json::{Value, json};

type Tx = mpsc::UnboundedSender<Option<String>>;

#[derive(Default)]
pub struct World {
    pub events: Vec<Event>,
    conns: Vec<Tx>,
    subs: Vec<(Tx, String, Vec<Filter>)>,
    /// Answer NEG-OPEN with a NOTICE (a relay without NIP-77).
    pub no_nip77: bool,
    pub neg_opens: usize,
    pub reqs: usize,
    /// `REQ`s that carried an `until` (paged walks).
    pub paged_reqs: usize,
    /// Every NEG-OPEN filter, in arrival order.
    pub neg_filters: Vec<Filter>,
    /// Every `REQ {ids}` fetch's ids.
    pub id_fetches: Vec<Vec<String>>,
    /// Every other one-filter `REQ` (walks, `#c` part fetches).
    pub walks: Vec<Filter>,
}

pub struct TestRelay {
    pub url: String,
    world: Arc<Mutex<World>>,
}

impl TestRelay {
    pub fn start() -> TestRelay {
        let listener = async_io::block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let world = Arc::new(Mutex::new(World::default()));
        let shared = world.clone();
        std::thread::spawn(move || {
            async_io::block_on(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let world = shared.clone();
                    std::thread::spawn(move || async_io::block_on(serve(stream, world)));
                }
            })
        });
        TestRelay { url, world }
    }

    pub fn world(&self) -> parking_lot::MutexGuard<'_, World> {
        self.world.lock()
    }

    /// Store an event as if a client had published it (live subscribers hear it).
    pub fn insert(&self, event: Event) {
        let mut w = self.world();
        for (conn, sub, filters) in &w.subs {
            if filters.iter().any(|f| f.matches(&event)) {
                send(conn, json!(["EVENT", sub, event]));
            }
        }
        store(&mut w, event);
    }
}

fn send(tx: &Tx, frame: Value) {
    let _ = tx.unbounded_send(Some(frame.to_string()));
}

fn addressable(kind: u32) -> bool {
    (30000..40000).contains(&kind)
}

fn store(w: &mut World, event: Event) {
    if w.events.iter().any(|e| e.id == event.id) {
        return;
    }
    if addressable(event.kind) {
        let d = event.tag("d").unwrap_or_default().to_string();
        w.events.retain(|e| {
            !(e.kind == event.kind
                && e.pubkey == event.pubkey
                && e.tag("d").unwrap_or_default() == d)
        });
    }
    w.events.push(event);
}

/// NIP-01 query: per filter, newest first, `limit` applied; union by id.
fn query<'a>(events: &'a [Event], filters: &[Filter]) -> Vec<&'a Event> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for filter in filters {
        let mut hits: Vec<&Event> = events.iter().filter(|e| filter.matches(e)).collect();
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        if let Some(limit) = filter.limit {
            hits.truncate(limit as usize);
        }
        for e in hits {
            if seen.insert(e.id.as_str()) {
                out.push(e);
            }
        }
    }
    out
}

async fn serve(stream: async_net::TcpStream, world: Arc<Mutex<World>>) {
    let Ok(socket) = async_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::unbounded::<Option<String>>();
    world.lock().conns.push(tx.clone());
    let mut negs: HashMap<String, Negentropy> = HashMap::new();
    loop {
        match future::select(source.next(), rx.next()).await {
            future::Either::Left((Some(Ok(Message::Text(text))), _)) => {
                let frame: Vec<Value> = serde_json::from_str(text.as_str()).unwrap();
                handle(&frame, &tx, &world, &mut negs);
            }
            future::Either::Left((Some(Ok(Message::Ping(data))), _)) => {
                let _ = sink.send(Message::Pong(data)).await;
            }
            future::Either::Left((Some(Ok(_)), _)) => {}
            future::Either::Left(_) => break,
            future::Either::Right((Some(Some(text)), _)) => {
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            future::Either::Right(_) => {
                drop(sink.close(None).await);
                break;
            }
        }
    }
    let mut w = world.lock();
    w.subs.retain(|(conn, _, _)| !conn.same_receiver(&tx));
    w.conns.retain(|conn| !conn.same_receiver(&tx));
}

fn handle(
    frame: &[Value],
    tx: &Tx,
    world: &Arc<Mutex<World>>,
    negs: &mut HashMap<String, Negentropy>,
) {
    let mut w = world.lock();
    let id = frame
        .get(1)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match frame[0].as_str().unwrap() {
        "EVENT" => {
            let event: Event = serde_json::from_value(frame[1].clone()).unwrap();
            if !event.verify() {
                send(tx, json!(["OK", event.id, false, "invalid: bad signature"]));
                return;
            }
            for (conn, sub, filters) in &w.subs {
                if filters.iter().any(|f| f.matches(&event)) {
                    send(conn, json!(["EVENT", sub, event]));
                }
            }
            send(tx, json!(["OK", event.id, true, ""]));
            store(&mut w, event);
        }
        "REQ" => {
            let filters: Vec<Filter> = frame[2..]
                .iter()
                .map(|f| serde_json::from_value(f.clone()).unwrap())
                .collect();
            w.reqs += 1;
            if let [filter] = filters.as_slice() {
                match &filter.ids {
                    Some(ids) => w.id_fetches.push(ids.clone()),
                    None => w.walks.push(filter.clone()),
                }
            }
            if filters.iter().any(|f| f.until.is_some()) {
                w.paged_reqs += 1;
            }
            for event in query(&w.events, &filters) {
                send(tx, json!(["EVENT", id, event]));
            }
            send(tx, json!(["EOSE", id]));
            w.subs.push((tx.clone(), id, filters));
        }
        "CLOSE" => w
            .subs
            .retain(|(conn, sub, _)| !(*sub == id && conn.same_receiver(tx))),
        "NEG-OPEN" if w.no_nip77 => send(tx, json!(["NOTICE", "ERROR: bad msg: unknown cmd"])),
        "NEG-OPEN" => {
            w.neg_opens += 1;
            let filter: Filter = serde_json::from_value(frame[2].clone()).unwrap();
            w.neg_filters.push(filter.clone());
            let held = w
                .events
                .iter()
                .filter(|e| filter.matches(e))
                .map(|e| (e.created_at, e.id.as_str()));
            let mut server =
                Negentropy::new(NegentropyStorage::from_hex(held).unwrap(), 0).unwrap();
            let reply = server
                .reconcile(&hex::decode(frame[3].as_str().unwrap()).unwrap())
                .unwrap();
            send(
                tx,
                json!(["NEG-MSG", id, hex::encode(reply.output.unwrap_or_default())]),
            );
            negs.insert(id, server);
        }
        "NEG-MSG" => {
            let Some(server) = negs.get_mut(&id) else {
                send(tx, json!(["NEG-ERR", id, "closed: no session"]));
                return;
            };
            let reply = server
                .reconcile(&hex::decode(frame[2].as_str().unwrap()).unwrap())
                .unwrap();
            send(
                tx,
                json!(["NEG-MSG", id, hex::encode(reply.output.unwrap_or_default())]),
            );
        }
        "NEG-CLOSE" => {
            negs.remove(&id);
        }
        _ => {}
    }
}
