//! An agent's progress in the chat: while it works, its thinking and tool
//! steps stream into a fixed-height box under its name; its reply keeps the
//! record folded into a "Thought for …" line that opens the same box.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::live::{LiveEvent, LiveKind};
use roostr_core::nav::Route;
use roostr_core::types::{
    Block, BlockContent, CustomContent, Fields, Object, ObjectSummary, QueryResult, QueryRow,
    RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream, boxed};
use serde_json::Value as Json;
use support::App;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";
const AGENT: &str = "agent-1";

type Events = mpsc::UnboundedSender<anyhow::Result<BackendEvent>>;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

fn object(id: &str, type_key: &str, name: &str) -> Object {
    let mut fields = Fields::default();
    fields.insert("name".into(), Value::string(name));
    fields.insert("channel".into(), Value::string(SPACE));
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

fn custom(content_type: &str, meta: BTreeMap<String, String>) -> BlockContent {
    BlockContent {
        custom: Some(CustomContent {
            content_type: content_type.into(),
            meta: Some(meta),
            data: None,
        }),
        ..BlockContent::default()
    }
}

/// A page whose chat holds the agent's lines, each with `meta` on top of author, time and text.
fn page(lines: Vec<(&str, BTreeMap<String, String>)>) -> Object {
    let mut p = object(PAGE, "page", "Report");
    let mut blocks = vec![Block {
        id: "__discussion__".into(),
        children_ids: lines.iter().map(|(id, _)| id.to_string()).collect(),
        content: custom("discussion", BTreeMap::new()),
        ..Block::default()
    }];
    for (i, (id, extra)) in lines.into_iter().enumerate() {
        let mut meta = BTreeMap::from([
            ("author".to_string(), AGENT.to_string()),
            ("ts".to_string(), (1000 + i).to_string()),
            ("text".to_string(), format!("Line {i}")),
        ]);
        meta.extend(extra);
        blocks.push(Block {
            id: id.into(),
            content: custom("chat", meta),
            ..Block::default()
        });
    }
    p.blocks = blocks;
    p
}

struct Vault {
    objects: Vec<Object>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a page with an agent at work".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .objects
            .iter()
            .map(|o| ObjectSummary {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: o.field_str("name").into(),
                channel_id: SPACE.into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let found = self.objects.iter().find(|o| o.id == id).cloned();
        ready(found.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: SPACE.into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .objects
            .iter()
            .filter(|o| o.type_key == "agent" && text.contains("\"agent\""))
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: Some(o.field_str("name").into()),
                fields: o.fields.clone(),
                ..QueryRow::default()
            })
            .collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        match self.events.lock().take() {
            Some(rx) => Box::pin(rx),
            None => Box::pin(futures::stream::pending()),
        }
    }
}

/// The app on the page, and the backend's event stream into it.
fn open(page: Object) -> (App, Events) {
    let (tx, rx) = mpsc::unbounded();
    let vault = Vault {
        objects: vec![page, object(AGENT, "agent", "Scout")],
        events: Mutex::new(Some(rx)),
    };
    let mut app = App::open(Arc::new(vault), 1568., 934.);
    app.navigate(Route::Object(PAGE.into()));
    app.settle();
    (app, tx)
}

fn live(tx: &Events, seq: u64, kind: LiveKind, text: &str) {
    let event = LiveEvent {
        run: "run-1".into(),
        agent: AGENT.into(),
        object: PAGE.into(),
        thread: "__discussion__".into(),
        seq,
        kind,
        text: text.into(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    };
    tx.unbounded_send(Ok(BackendEvent::Live(event))).unwrap();
}

/// Every text drawn inside the element keyed `key`.
fn texts_in(app: &App, key: &str) -> String {
    let all = app.rt.elements();
    let Some(root) = all.iter().position(|e| e.key.as_deref() == Some(key)) else {
        return String::new();
    };
    let inside = |mut i: usize| loop {
        if i == root {
            return true;
        }
        match all[i].parent {
            Some(p) => i = p,
            None => return false,
        }
    };
    (0..all.len())
        .filter(|&i| inside(i))
        .filter_map(|i| all[i].text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How much of the text containing `needle` shows through its clips.
fn visible_height(app: &App, needle: &str) -> f32 {
    let all = app.rt.elements();
    let found = all
        .iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.contains(needle)))
        .unwrap_or_else(|| panic!("no text {needle:?}"));
    found.visible[3]
}

#[test]
fn live_progress_streams_into_a_box_of_fixed_height() {
    let (mut app, tx) = open(page(vec![("m1", BTreeMap::new())]));
    assert!(!app.shown("live-box-run-1"), "no box before any progress");

    live(&tx, 0, LiveKind::Thinking, "Looking at ");
    live(&tx, 1, LiveKind::Thinking, "the repository first.");
    live(&tx, 2, LiveKind::Step, "shell_exec: git status");
    app.settle();
    let first = app
        .rect("live-box-run-1")
        .expect("the live box shows under the agent");
    let shown = texts_in(&app, "live-run-1");
    assert!(shown.contains("Scout"), "the agent's name: {shown}");
    assert!(shown.contains("Thinking… 0:"), "the elapsed clock: {shown}");
    assert!(
        shown.contains("Looking at the repository first."),
        "the thinking so far: {shown}"
    );
    assert!(
        shown.contains("shell_exec: git status"),
        "the step marker: {shown}"
    );

    for seq in 3..40 {
        if seq % 5 == 0 {
            live(
                &tx,
                seq,
                LiveKind::Step,
                &format!("read_file: src/{seq}.rs"),
            );
        } else {
            live(
                &tx,
                seq,
                LiveKind::Thinking,
                &format!("Thought number {seq} goes on.\n"),
            );
        }
    }
    app.settle();
    let later = app.rect("live-box-run-1").expect("the box still shows");
    assert_eq!(
        first[3], later[3],
        "the box keeps its height as progress arrives"
    );
    assert!(
        visible_height(&app, "read_file: src/35.rs") > 0.,
        "the box follows the newest step"
    );
    assert_eq!(
        visible_height(&app, "Looking at the repository first."),
        0.,
        "the oldest lines scrolled out of the box"
    );

    live(&tx, 40, LiveKind::Done, "");
    app.settle();
    assert!(!app.shown("live-box-run-1"), "done ends the run");
}

#[test]
fn a_reply_keeps_its_record_folded_and_opens_it_on_click() {
    let record = r#"[{"kind":"thinking","text":"Check the status first.","ts":1},
        {"kind":"step","text":"shell_exec: git status","ts":2},
        {"kind":"step","text":"read_file: README.md","ts":3}]"#;
    let meta = BTreeMap::from([
        ("thinking".to_string(), record.to_string()),
        ("thought_ms".to_string(), "80000".to_string()),
    ]);
    let (mut app, _tx) = open(page(vec![("m0", BTreeMap::new()), ("m1", meta)]));

    let line = texts_in(&app, "thought-m1");
    assert_eq!(line, "Thought for 1m 20s · 2 steps ▸");
    assert!(!app.shown("thought-m0"), "a line without a record has none");
    assert!(!app.shown("thought-box-m1"), "folded by default");

    app.tap("thought-m1");
    assert!(app.shown("thought-box-m1"), "a click opens the record");
    let record = texts_in(&app, "thought-box-m1");
    assert!(record.contains("Check the status first."), "{record}");
    assert!(record.contains("read_file: README.md"), "{record}");
    assert!(texts_in(&app, "thought-m1").ends_with('▾'));
}
