//! A WebSocket proxy in front of the in-process `TestRelay` that makes it
//! misbehave the ways real relays do: it withholds events the relay holds,
//! or refuses NIP-77 with a `NEG-ERR`. It also records every `REQ`'s
//! filters (live subscriptions carry several) and counts `NEG-OPEN`s and
//! `NEG-CLOSE`s. Test files include it with `#[path]`.

use std::collections::HashSet;
use std::sync::Arc;

use async_net::{TcpListener, TcpStream};
use async_tungstenite::tungstenite::Message;
use futures::{StreamExt, future};
use parking_lot::{Mutex, MutexGuard};
use roostr_nostr::Filter;
use serde_json::{Value, json};

#[derive(Default)]
pub struct Rules {
    /// Event ids the relay holds but this proxy never delivers.
    pub withhold: HashSet<String>,
    /// Answer every `NEG-OPEN` with `["NEG-ERR", id, reason]`.
    pub neg_err: Option<String>,
    /// Every `REQ`'s filters, in arrival order.
    pub reqs: Vec<Vec<Filter>>,
    pub neg_opens: usize,
    pub neg_closes: usize,
}

pub struct RelayProxy {
    pub url: String,
    rules: Arc<Mutex<Rules>>,
}

impl RelayProxy {
    pub fn start(upstream: &str) -> RelayProxy {
        let listener = async_io::block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let rules = Arc::new(Mutex::new(Rules::default()));
        let shared = rules.clone();
        let upstream = upstream.to_string();
        std::thread::spawn(move || {
            async_io::block_on(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let rules = shared.clone();
                    let upstream = upstream.clone();
                    std::thread::spawn(move || async_io::block_on(pipe(stream, upstream, rules)));
                }
            })
        });
        RelayProxy { url, rules }
    }

    pub fn rules(&self) -> MutexGuard<'_, Rules> {
        self.rules.lock()
    }
}

async fn pipe(stream: TcpStream, upstream: String, rules: Arc<Mutex<Rules>>) {
    let Ok(client) = async_tungstenite::accept_async(stream).await else {
        return;
    };
    let Ok(tcp) = TcpStream::connect(upstream.trim_start_matches("ws://")).await else {
        return;
    };
    let Ok((server, _)) = async_tungstenite::client_async(upstream.as_str(), tcp).await else {
        return;
    };
    let (mut to_client, mut from_client) = client.split();
    let (mut to_server, mut from_server) = server.split();
    loop {
        match future::select(from_client.next(), from_server.next()).await {
            future::Either::Left((Some(Ok(message)), _)) => match message {
                Message::Text(text) => {
                    let sent = match outbound(text.as_str(), &rules) {
                        Some(reply) => to_client.send(Message::text(reply.to_string())).await,
                        None => to_server.send(Message::Text(text)).await,
                    };
                    if sent.is_err() {
                        break;
                    }
                }
                Message::Ping(data) => {
                    let _ = to_client.send(Message::Pong(data)).await;
                }
                Message::Close(_) => break,
                _ => {}
            },
            future::Either::Right((Some(Ok(message)), _)) => match message {
                Message::Text(text) => {
                    if withheld(text.as_str(), &rules) {
                        continue;
                    }
                    if to_client.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            },
            _ => break,
        }
    }
    let _ = to_server.close(None).await;
    let _ = to_client.close(None).await;
}

/// A client frame: recorded, and answered here instead of forwarded when
/// the rules say so.
fn outbound(text: &str, rules: &Mutex<Rules>) -> Option<Value> {
    let frame: Vec<Value> = serde_json::from_str(text).ok()?;
    let id = frame.get(1).and_then(Value::as_str).unwrap_or_default();
    let mut rules = rules.lock();
    match frame.first().and_then(Value::as_str)? {
        "REQ" => {
            let filters = frame[2..]
                .iter()
                .filter_map(|f| serde_json::from_value(f.clone()).ok())
                .collect();
            rules.reqs.push(filters);
            None
        }
        "NEG-OPEN" => {
            rules.neg_opens += 1;
            rules
                .neg_err
                .as_ref()
                .map(|reason| json!(["NEG-ERR", id, reason]))
        }
        "NEG-CLOSE" => {
            rules.neg_closes += 1;
            None
        }
        _ => None,
    }
}

/// A relay `EVENT` frame carrying a withheld event.
fn withheld(text: &str, rules: &Mutex<Rules>) -> bool {
    let Ok(frame) = serde_json::from_str::<Vec<Value>>(text) else {
        return false;
    };
    frame.first().and_then(Value::as_str) == Some("EVENT")
        && frame
            .get(2)
            .and_then(|e| e["id"].as_str())
            .is_some_and(|id| rules.lock().withhold.contains(id))
}
