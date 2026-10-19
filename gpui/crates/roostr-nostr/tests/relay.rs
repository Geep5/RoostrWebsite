//! The relay client against an in-process NIP-01/NIP-77 relay (whose
//! negentropy side is the reference-checked responder), and once against
//! Roostr's real relay (`--ignored`).
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use async_net::TcpListener;
use async_tungstenite::tungstenite::Message;
use futures::channel::mpsc;
use futures::{StreamExt, future};
use roostr_nostr::negentropy::{Negentropy, NegentropyStorage};
use roostr_nostr::relay::{
    BACKOFF_FLOOR, LiveItem, NEG_TIMEOUT, Pacer, RelayOptions, SubscriptionItem, live, publish_any,
    query_many,
};
use roostr_nostr::{Event, EventTemplate, Filter, Keys, Relay, RelayError, nip11};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(5);

/// What the test relay knows and how it misbehaves.
#[derive(Default)]
struct World {
    events: Vec<Event>,
    /// Open connections' outgoing queues; `None` closes the socket.
    conns: Vec<mpsc::UnboundedSender<Option<String>>>,
    /// Live subscriptions: (connection, subscription id, filters).
    subs: Vec<(mpsc::UnboundedSender<Option<String>>, String, Vec<Filter>)>,
    /// Answer NEG-OPEN with a NOTICE, like a relay without NIP-77.
    no_nip77: bool,
    /// Bytes per responder message (forces multi-round reconciliations).
    neg_frame_limit: usize,
    neg_rounds: usize,
}

struct TestRelay {
    url: String,
    world: Arc<Mutex<World>>,
}

impl TestRelay {
    fn start() -> TestRelay {
        let listener = async_io::block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let world = Arc::new(Mutex::new(World {
            neg_frame_limit: 4096,
            ..World::default()
        }));
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

    fn world(&self) -> parking_lot::MutexGuard<'_, World> {
        self.world.lock()
    }

    /// Drop every open socket, as a relay restart would.
    fn kill_connections(&self) {
        for conn in self.world().conns.drain(..) {
            let _ = conn.unbounded_send(None);
        }
    }
}

fn send(tx: &mpsc::UnboundedSender<Option<String>>, frame: Value) {
    let _ = tx.unbounded_send(Some(frame.to_string()));
}

async fn serve(stream: async_net::TcpStream, world: Arc<Mutex<World>>) {
    let Ok(socket) = async_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::unbounded::<Option<String>>();
    world.lock().conns.push(tx.clone());
    let mut negs: std::collections::HashMap<String, Negentropy> = Default::default();
    loop {
        match future::select(source.next(), rx.next()).await {
            future::Either::Left((Some(Ok(Message::Text(text))), _)) => {
                let frame: Vec<Value> = serde_json::from_str(text.as_str()).unwrap();
                handle(&frame, &tx, &world, &mut negs);
            }
            future::Either::Left((Some(Ok(_)), _)) => {}
            future::Either::Left(_) => return,
            future::Either::Right((Some(Some(text)), _)) => {
                if sink.send(Message::text(text)).await.is_err() {
                    return;
                }
            }
            future::Either::Right(_) => {
                drop(sink.close(None).await);
                return;
            }
        }
    }
}

fn handle(
    frame: &[Value],
    tx: &mpsc::UnboundedSender<Option<String>>,
    world: &Arc<Mutex<World>>,
    negs: &mut std::collections::HashMap<String, Negentropy>,
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
            } else if event.kind == 666 {
                send(tx, json!(["OK", event.id, false, "blocked: test"]));
            } else {
                for (conn, sub, filters) in &w.subs {
                    if filters.iter().any(|f| f.matches(&event)) {
                        send(conn, json!(["EVENT", sub, event]));
                    }
                }
                send(tx, json!(["OK", event.id, true, ""]));
                w.events.push(event);
            }
        }
        "REQ" => {
            let filters: Vec<Filter> = frame[2..]
                .iter()
                .map(|f| serde_json::from_value(f.clone()).unwrap())
                .collect();
            // Kind 999 is never answered: the client's deadline is tested.
            if filters
                .iter()
                .any(|f| f.kinds.as_ref().is_some_and(|k| k.contains(&999)))
            {
                return;
            }
            for event in w
                .events
                .iter()
                .filter(|e| filters.iter().any(|f| f.matches(e)))
            {
                send(tx, json!(["EVENT", id, event]));
            }
            send(tx, json!(["EOSE", id]));
            w.subs.push((tx.clone(), id, filters));
        }
        "CLOSE" => w.subs.retain(|(_, sub, _)| *sub != id),
        "NEG-OPEN" if w.no_nip77 => send(tx, json!(["NOTICE", "ERROR: bad msg: unknown cmd"])),
        "NEG-OPEN" => {
            let filter: Filter = serde_json::from_value(frame[2].clone()).unwrap();
            let held = w
                .events
                .iter()
                .filter(|e| filter.matches(e))
                .map(|e| (e.created_at, e.id.as_str()));
            let mut server = Negentropy::new(
                NegentropyStorage::from_hex(held).unwrap(),
                w.neg_frame_limit,
            )
            .unwrap();
            let reply = server
                .reconcile(&hex::decode(frame[3].as_str().unwrap()).unwrap())
                .unwrap();
            w.neg_rounds += 1;
            send(
                tx,
                json!(["NEG-MSG", id, hex::encode(reply.output.unwrap())]),
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
            w.neg_rounds += 1;
            send(
                tx,
                json!(["NEG-MSG", id, hex::encode(reply.output.unwrap())]),
            );
        }
        "NEG-CLOSE" => {
            negs.remove(&id);
        }
        _ => {}
    }
}

fn note(keys: &Keys, kind: u32, created_at: u64, content: &str) -> Event {
    keys.sign(EventTemplate {
        kind,
        created_at,
        tags: vec![vec!["h".into(), "stream".into()]],
        content: content.into(),
    })
}

#[test]
fn publish_query_and_rejections() {
    let server = TestRelay::start();
    let relay = Relay::new(&server.url);
    let keys = Keys::generate();
    async_io::block_on(async {
        let first = note(&keys, 1078, 100, "a");
        relay.publish(&first, T).await.unwrap();
        let blocked = note(&keys, 666, 100, "b");
        assert_eq!(
            relay.publish(&blocked, T).await,
            Err(RelayError::Rejected("blocked: test".into()))
        );

        // A forged event the relay serves is dropped, never delivered.
        let mut forged = note(&keys, 1078, 101, "c");
        forged.content = "tampered".into();
        server.world().events.push(forged);
        let found = relay
            .query(
                vec![Filter::new().kinds([1078]).authors([keys.pubkey()])],
                T,
            )
            .await
            .unwrap();
        assert_eq!(found, vec![first.clone()]);
        // So is one the relay sends for a filter it does not match.
        server.world().events.push(note(&keys, 1079, 102, "d"));
        let found = relay
            .query(vec![Filter::new().kinds([1078]).tag('h', ["stream"])], T)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);

        // No EOSE: a timeout, not a short answer.
        assert_eq!(
            relay
                .query(vec![Filter::new().kinds([999])], Duration::from_millis(300))
                .await,
            Err(RelayError::Timeout)
        );

        // Several relays: the first acceptance wins; a dead relay does not fail the call.
        let dead = Relay::with_options(
            "ws://127.0.0.1:9",
            RelayOptions {
                connect_timeout: Duration::from_secs(2),
                ..RelayOptions::default()
            },
        );
        let second = note(&keys, 1078, 103, "e");
        publish_any(&[dead.clone(), relay.clone()], &second, T)
            .await
            .unwrap();
        let all = query_many(
            &[dead.clone(), relay.clone()],
            &[Filter::new().ids([first.id.clone(), second.id.clone()])],
            T,
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 2);
        assert!(matches!(
            query_many(&[dead], &[Filter::new()], T).await,
            Err(RelayError::Transport(_))
        ));
    });
}

#[test]
fn reconcile_finds_both_differences_over_several_rounds() {
    let server = TestRelay::start();
    let relay = Relay::new(&server.url);
    let keys = Keys::generate();
    let events: Vec<Event> = (0..3000)
        .map(|i| note(&keys, 1078, 1_700_000_000 + i / 5, &i.to_string()))
        .collect();
    // The relay holds 0..2500, this device holds 500..3000.
    server.world().events.extend(events[..2500].iter().cloned());
    let held =
        NegentropyStorage::from_hex(events[500..].iter().map(|e| (e.created_at, e.id.as_str())))
            .unwrap();
    let filter = Filter::new().kinds([1078]).authors([keys.pubkey()]);
    let result = async_io::block_on(relay.reconcile(&filter, held, NEG_TIMEOUT)).unwrap();
    let ids = |range: std::ops::Range<usize>| {
        events[range]
            .iter()
            .map(|e| e.id.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        result
            .need
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        ids(0..500)
    );
    assert_eq!(
        result
            .have
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        ids(2500..3000)
    );
    assert!(
        server.world().neg_rounds > 1,
        "the 4 KiB responder frames force several NEG-MSG rounds"
    );

    // Same sets: done in one round, nothing to fetch.
    let all =
        NegentropyStorage::from_hex(events[..2500].iter().map(|e| (e.created_at, e.id.as_str())))
            .unwrap();
    let same = async_io::block_on(relay.reconcile(&filter, all, NEG_TIMEOUT)).unwrap();
    assert!(same.need.is_empty() && same.have.is_empty());
}

#[test]
fn a_relay_without_nip77_is_told_apart() {
    let server = TestRelay::start();
    server.world().no_nip77 = true;
    let relay = Relay::new(&server.url);
    let error = async_io::block_on(relay.reconcile(
        &Filter::new().kinds([1078]),
        NegentropyStorage::default(),
        NEG_TIMEOUT,
    ))
    .unwrap_err();
    assert!(matches!(error.error, RelayError::Unsupported(_)), "{error}");
    assert!(error.relay_refused());

    // Nobody listening: not a refusal (walk this time, ask again later).
    let dead = Relay::with_options(
        "ws://127.0.0.1:9",
        RelayOptions {
            connect_timeout: Duration::from_secs(2),
            ..RelayOptions::default()
        },
    );
    let error = async_io::block_on(dead.reconcile(
        &Filter::new(),
        NegentropyStorage::default(),
        NEG_TIMEOUT,
    ))
    .unwrap_err();
    assert!(!error.relay_refused(), "{error}");
}

#[test]
fn subscriptions_stream_and_survive_a_dropped_socket() {
    let server = TestRelay::start();
    let relay = Relay::new(&server.url);
    let writer = Relay::new(&server.url);
    let keys = Keys::generate();
    async_io::block_on(async {
        let stored = note(&keys, 1078, 10, "stored");
        writer.publish(&stored, T).await.unwrap();

        // A plain subscription: stored events, EOSE, then live ones.
        let mut sub = relay.subscribe(vec![Filter::new().kinds([1078])]);
        assert_eq!(
            sub.next().await,
            Some(Ok(SubscriptionItem::Event(stored.clone())))
        );
        assert_eq!(sub.next().await, Some(Ok(SubscriptionItem::Eose)));
        let fresh = note(&keys, 1078, 11, "fresh");
        writer.publish(&fresh, T).await.unwrap();
        assert_eq!(
            sub.next().await,
            Some(Ok(SubscriptionItem::Event(fresh.clone())))
        );
        drop(sub);

        // A live stream: filters rebuilt on every (re)subscription.
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut stream = Box::pin(live(relay.clone(), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            vec![Filter::new().kinds([1078]).since(11)]
        }));
        assert_eq!(stream.next().await, Some(LiveItem::Event(fresh.clone())));
        assert_eq!(
            stream.next().await,
            Some(LiveItem::Up { reconnected: false })
        );

        server.kill_connections();
        assert!(matches!(
            stream.next().await,
            Some(LiveItem::Down(RelayError::Transport(_)))
        ));
        let down_at = Instant::now();
        // The backoff floor, then a new socket and subscription.
        assert_eq!(stream.next().await, Some(LiveItem::Event(fresh.clone())));
        assert!(down_at.elapsed() >= BACKOFF_FLOOR - Duration::from_millis(50));
        assert_eq!(
            stream.next().await,
            Some(LiveItem::Up { reconnected: true })
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let after = note(&keys, 1078, 12, "after reconnect");
        writer.publish(&after, T).await.unwrap();
        assert_eq!(stream.next().await, Some(LiveItem::Event(after)));
    });
}

#[test]
fn disconnect_fails_waiters_and_the_next_call_redials() {
    let server = TestRelay::start();
    let relay = Relay::new(&server.url);
    async_io::block_on(async {
        let pending = relay.query(vec![Filter::new().kinds([999])], T);
        let disconnect = async {
            roostr_nostr::sleep(Duration::from_millis(200)).await;
            assert!(relay.is_connected());
            relay.disconnect();
        };
        let (result, ()) = future::join(pending, disconnect).await;
        assert_eq!(result, Err(RelayError::Disconnected));
        assert!(!relay.is_connected());
        assert!(
            relay
                .query(vec![Filter::new().kinds([1])], T)
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn pacer_spaces_calls() {
    async_io::block_on(async {
        let pacer = Pacer::default();
        let start = Instant::now();
        future::join3(pacer.ready(), pacer.ready(), pacer.ready()).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(235) && elapsed < Duration::from_millis(600),
            "{elapsed:?}"
        );
    });
}

/// The acceptance check against Roostr's relay: no publishing, one REQ for
/// a kind-1 note, one NEG-OPEN round from an empty set, the NIP-11 document.
#[test]
#[ignore = "talks to wss://roostr-relay.fly.dev"]
fn live_roostr_relay() {
    let url = roostr_nostr::DEFAULT_RELAYS[0];
    let relay = Relay::new(url);
    async_io::block_on(async {
        let started = Instant::now();
        let notes = relay
            .query(
                vec![Filter::new().kinds([1]).limit(1)],
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        println!(
            "REQ kinds:[1] limit:1 -> {} event(s) in {:?}",
            notes.len(),
            started.elapsed()
        );
        assert!(notes.len() <= 1);
        assert!(notes.iter().all(Event::verify));

        let since = roostr_nostr::now_s() - 7 * 86_400;
        let started = Instant::now();
        let reconciled = relay
            .reconcile(
                &Filter::new().kinds([1]).since(since),
                NegentropyStorage::default(),
                NEG_TIMEOUT,
            )
            .await
            .unwrap();
        println!(
            "NEG-OPEN kinds:[1] since:{since} from an empty set -> need {} / have {} in {:?}",
            reconciled.need.len(),
            reconciled.have.len(),
            started.elapsed()
        );
        assert!(reconciled.have.is_empty());

        let info = nip11::fetch(url, Duration::from_secs(10)).await.unwrap();
        println!("NIP-11: {info:?}");
        println!(
            "blossom server: {}",
            roostr_nostr::blossom::server(&[url.to_string()]).await
        );
    });
}
