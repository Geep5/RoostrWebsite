//! The store runs this install's direct-line viewer over a backend that
//! carries signals: its hello goes out through `publish_signal`, and the
//! backend's signal events reach it - an offer for another device is
//! ignored, one for this device is answered (here: refused, and said bye to)
//! in the scope it came in.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use serde_json::{Value as Json, json};

#[derive(Default)]
struct Fake {
    published: Mutex<Vec<(Json, Option<String>)>>,
    streams: Mutex<Vec<mpsc::UnboundedSender<Result<BackendEvent>>>>,
}

struct Handle(Arc<Fake>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "fake".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        boxed(async {
            Ok(Settings {
                has_key: true,
                ..Settings::default()
            })
        })
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        boxed(async { Ok(Vec::new()) })
    }
    fn object(&self, _id: &str) -> BoxFuture<Result<Object>> {
        boxed(async { Err(anyhow::anyhow!("none")) })
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        boxed(async { Ok(Vec::new()) })
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        boxed(async { Ok(Vec::new()) })
    }
    fn query(&self, _body: &Json) -> BoxFuture<Result<QueryResult>> {
        boxed(async { Ok(QueryResult::default()) })
    }
    fn mutate(&self, _action: &str, _params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn serving(&self, _object_ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        let (tx, rx) = mpsc::unbounded();
        self.0.streams.lock().push(tx);
        boxed_stream(rx)
    }
    fn direct_lines(&self) -> bool {
        true
    }
    fn publish_signal(&self, signal: Json, space: Option<String>) -> BoxFuture<Result<()>> {
        self.0.published.lock().push((signal, space));
        boxed(async { Ok(()) })
    }
}

/// Poll the state until `check` holds (the viewer runs on its own thread).
fn until(s: &mut State, what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        s.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn offer(to: &str, session: &str) -> Json {
    json!({"v":1,"type":"offer","from":"host-1","to":to,"session":session,"role":"host",
        "sdp":"not an sdp","candidate":"","mid":"","ts":1})
}

#[test]
fn the_store_runs_the_viewer_over_its_backend() {
    let dir = std::env::temp_dir().join(format!("roostr-lines-{}", uuid::Uuid::new_v4()));
    // SAFETY: this test binary's only test, before the state reads it.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", dir.join("ui-state.json")) };
    let fake = Arc::new(Fake::default());
    let mut s = State::new(Arc::new(Handle(fake.clone())), None);
    let device: String =
        s.ui.get(roostr_core::direct::DEVICE_KEY)
            .expect("a device id");

    until(&mut s, "the hello", || !fake.published.lock().is_empty());
    let (hello, space) = fake.published.lock()[0].clone();
    assert_eq!(hello["type"], json!("hello"));
    assert_eq!(hello["role"], json!("viewer"));
    assert_eq!(hello["from"], json!(device));
    assert_eq!(space, None);

    until(&mut s, "the commit stream", || {
        !fake.streams.lock().is_empty()
    });
    let send = |event: BackendEvent| {
        fake.streams.lock()[0].unbounded_send(Ok(event)).unwrap();
    };
    send(BackendEvent::Signal {
        signal: offer("another-device", "s0"),
        space: None,
    });
    send(BackendEvent::Signal {
        signal: offer(&device, "s1"),
        space: Some("sp".into()),
    });
    until(&mut s, "the bye", || fake.published.lock().len() >= 2);
    let published = fake.published.lock().clone();
    assert_eq!(
        published.len(),
        2,
        "nothing for the other device: {published:?}"
    );
    let (bye, space) = &published[1];
    assert_eq!(bye["type"], json!("bye"));
    assert_eq!(
        (&bye["to"], &bye["session"]),
        (&json!("host-1"), &json!("s1"))
    );
    assert_eq!(
        space.as_deref(),
        Some("sp"),
        "answered in the offer's scope"
    );
    let _ = std::fs::remove_dir_all(dir);
}
