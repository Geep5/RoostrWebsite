//! Page covers: Add cover (hovering the icon and title) adds a pasted image
//! the usual way and sets `cover` + `cover_y`; the banner sits above the
//! title with the icon overlapping its bottom edge; Reposition drags the
//! focus and saves `cover_y`; Remove deletes both. Generate opens a prompt
//! box: its words (or none, "Choose for me") are asked for once and the
//! banner spins until the serving computer's image replaces the cover;
//! a failure shows its reason, and closing it deletes the run. A phone has
//! the same actions in its ⋯ menu.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;

use futures::channel::mpsc;
use parking_lot::Mutex;
use roostr_core::blossom;
use roostr_core::create::AddFile;
use roostr_core::nav::Route;
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap, State};
use roostr_rt::{BoxFuture, BoxStream, Task, boxed, boxed_stream};
use rune::Key;
use rune::clipboard::{ClipboardContent, ClipboardImage};
use serde_json::{Value as Json, json};
use support::App;

const PAGE: &str = "page-1";
const CREDENTIAL: &str = "openai-sp-1";
const NEEDS_KEY: &str = "Add an OpenAI key to this space's OpenAI credential to generate covers";
const PNG: &[u8] = include_bytes!("../../../../static/logo.png");

/// One space with a page, an OpenAI credential (blank key) and image Files
/// whose bytes sit on a local Blossom server.
struct Vault {
    objects: Mutex<HashMap<String, Object>>,
    /// `(action, params)` of every field write and vanish.
    log: Mutex<Vec<(String, Json)>>,
    events: Mutex<Option<mpsc::UnboundedReceiver<anyhow::Result<BackendEvent>>>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

fn object(id: &str, type_key: &str, fields: Fields) -> Object {
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

fn fields(json: Json) -> Fields {
    serde_json::from_value(json).unwrap()
}

const BLOB_KEY: [u8; 32] = [9u8; 32];

impl Vault {
    /// `blob`: the PNG encrypted under [`BLOB_KEY`], served at `server`.
    fn new(server: &str, blob: &[u8]) -> Self {
        let blob_sha = blossom::sha256_hex(blob);
        let blob_key: String = BLOB_KEY.iter().map(|b| format!("{b:02x}")).collect();
        let file = |id: &str| {
            object(
                id,
                "file",
                fields(json!({
                    "name": { "stringValue": format!("{id}.png") },
                    "channel": { "stringValue": "sp-1" },
                    "file_mime": { "stringValue": "image/png" },
                    "file_hash": { "stringValue": blossom::sha256_hex(PNG) },
                    "blob_sha256": { "stringValue": blob_sha },
                    "blob_key": { "stringValue": blob_key },
                    "blob_url": { "stringValue": format!("{server}/{blob_sha}") },
                })),
            )
        };
        let mut objects: HashMap<String, Object> = ["file-1", "try-1", "try-2", "try-3"]
            .into_iter()
            .map(|id| (id.to_string(), file(id)))
            .collect();
        objects.insert(
            PAGE.into(),
            object(
                PAGE,
                "page",
                fields(json!({
                    "name": { "stringValue": "Launch plan" },
                    "channel": { "stringValue": "sp-1" },
                })),
            ),
        );
        objects.insert(
            CREDENTIAL.into(),
            object(
                CREDENTIAL,
                "credential",
                fields(json!({
                    "name": { "stringValue": "OpenAI" },
                    "channel": { "stringValue": "sp-1" },
                    "service": { "stringValue": "openai" },
                    "key_api_key": { "stringValue": "" },
                })),
            ),
        );
        Self {
            objects: Mutex::new(objects),
            log: Mutex::new(Vec::new()),
            events: Mutex::new(None),
        }
    }

    /// The `(action, key-or-ids, value)` written since the last call.
    fn take_log(&self) -> Vec<(String, Json, Json)> {
        self.log
            .lock()
            .drain(..)
            .map(|(action, p)| {
                let what = if action == "vanish" {
                    p["object_ids"].clone()
                } else {
                    p["key"].clone()
                };
                (action, what, p.get("value").cloned().unwrap_or(Json::Null))
            })
            .collect()
    }

    /// Another computer (the credential's harness) changes `id`'s fields.
    fn edit(&self, id: &str, change: impl FnOnce(&mut Fields)) {
        change(&mut self.objects.lock().get_mut(id).unwrap().fields);
    }
}

/// The store's handle on a shared [`Vault`].
struct Handle(Arc<Vault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "a page with covers".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: PAGE.into(),
            type_key: "page".into(),
            name: "Launch plan".into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(
            self.0
                .objects
                .lock()
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown object {id}")),
        )
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let records: Vec<QueryRow> = self
            .0
            .objects
            .lock()
            .values()
            .filter(|o| body["type"] == json!(o.type_key) && o.type_key == "credential")
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                created_at: 1,
                fields: o.fields.clone(),
                ..QueryRow::default()
            })
            .collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string()
        };
        let (id, key) = (text("object_id"), text("key"));
        if action != "vanish" && !self.0.objects.lock().contains_key(&id) {
            return ready(Ok(JsonMap::new()));
        }
        match action {
            "set_field" => {
                let value: Value = serde_json::from_value(params["value"].clone()).unwrap();
                self.0.edit(&id, |f| {
                    f.insert(key, value);
                });
            }
            "delete_field" => self.0.edit(&id, |f| {
                f.remove(&key);
            }),
            "vanish" => {}
            _ => return ready(Ok(JsonMap::new())),
        }
        self.0
            .log
            .lock()
            .push((action.into(), Json::Object(params)));
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        match self.0.events.lock().take() {
            Some(rx) => boxed_stream(rx),
            None => boxed_stream(futures::stream::pending()),
        }
    }
}

/// A local Blossom stand-in: every GET answers `blob`.
fn serve(blob: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                blob.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&blob);
        }
    });
    format!("http://{address}")
}

/// Uploads that reached the app's file adder: `(name, mime, space)`.
#[derive(Default)]
struct Uploads(Vec<(String, String, String)>);

/// The app on the page in a `width` x `height` window, its uploads recorded.
fn open(backend: &Arc<Vault>, width: f32, height: f32) -> (App, rune::clipboard::MemoryClipboard) {
    let mut app = App::open_with(Arc::new(Handle(backend.clone())), width, height, |s| {
        // The upload itself (harness or Blossom) is the files' own business:
        // record what reaches it and answer with the File it made.
        s.add_file = Some(AddFile(Rc::new(
            |name, mime, _bytes, space, s: &mut State| {
                s.slot::<Uploads>().0.push((name, mime, space));
                Task::ready(Ok("file-1".to_string()))
            },
        )));
    });
    let clipboard = rune::clipboard::memory();
    app.rt.use_clipboard(&clipboard);
    app.navigate(Route::Object(PAGE.into()));
    (app, clipboard)
}

fn set(key: &str, value: Json) -> (String, Json, Json) {
    ("set_field".into(), json!(key), value)
}

fn deleted(key: &str) -> (String, Json, Json) {
    ("delete_field".into(), json!(key), Json::Null)
}

fn generation(entries: Json) -> Value {
    serde_json::from_value(json!({ "mapValue": { "entries": entries } })).unwrap()
}

/// The serving computer takes the pending ask and answers with `entries`.
fn answer(
    backend: &Vault,
    tx: &mpsc::UnboundedSender<anyhow::Result<BackendEvent>>,
    app: &mut App,
    entries: Json,
) {
    backend.edit(PAGE, |f| {
        f.remove("cover_request");
        f.insert("cover_generation".into(), generation(entries));
    });
    tx.unbounded_send(Ok(BackendEvent::Committed(PAGE.into())))
        .unwrap();
    app.settle();
}

/// The pending `cover_request`'s prompt.
fn asked(backend: &Vault) -> Option<String> {
    let objects = backend.objects.lock();
    let entries = objects[PAGE]
        .fields
        .get("cover_request")?
        .entries()?
        .clone();
    Some(entries["prompt"].as_str().unwrap_or_default().to_string())
}

fn center(r: [f32; 4]) -> (f32, f32) {
    (r[0] + r[2] / 2., r[1] + r[3] / 2.)
}

#[test]
fn covers_on_a_desktop_page() {
    let blob = blossom::encrypt_blob(PNG, &BLOB_KEY).unwrap();
    let server = serve(blob.clone());
    let backend = Arc::new(Vault::new(&server, &blob));
    let (tx, rx) = mpsc::unbounded();
    *backend.events.lock() = Some(rx);
    let (mut app, clipboard) = open(&backend, 1280., 1000.);

    // No cover: hovering the icon and title offers Add cover.
    assert!(!app.shown("object-cover") && !app.shown("cover-add"));
    app.hover("object-title");
    assert!(app.shown("cover-add"));
    app.tap("cover-add");
    assert!(app.shown("cover-chooser") && app.shown("cover-pick"));

    // Paste an image: it is added the usual way, then becomes the cover.
    clipboard.set_content(
        ClipboardContent::new("").with_image(ClipboardImage::from_png(PNG).expect("a PNG")),
    );
    app.tap("cover-paste");
    let added = app.state().slot::<Uploads>().0.clone();
    assert_eq!(added.len(), 1);
    assert_eq!(
        (added[0].1.as_str(), added[0].2.as_str()),
        ("image/png", "sp-1")
    );
    assert_eq!(
        backend.take_log(),
        vec![
            set(
                "cover",
                json!({ "linkValue": { "targetId": "file-1", "relationKey": "cover" } })
            ),
            set("cover_y", json!({ "floatValue": 0.5 })),
        ]
    );
    assert!(!app.shown("cover-chooser"));

    // The banner spans the page above the title; the icon overlaps its bottom edge.
    let banner = app.rect("object-cover").expect("the cover");
    let title = app.rect("object-title").unwrap();
    let icon = app.rect("object-icon").unwrap();
    assert_eq!(banner[3], 200.);
    let bottom = banner[1] + banner[3];
    assert!(bottom <= title[1], "{banner:?} above {title:?}");
    assert!(icon[1] < bottom && bottom < icon[1] + icon[3]);
    assert!(banner[2] > title[2]);

    // Hovering the cover shows one Cover button (as Anytype); its menu holds
    // the rest. Reposition drags the focus.
    app.hover("object-cover");
    assert!(app.shown("cover-change"));
    assert!(!app.shown("cover-reposition") && !app.shown("cover-remove"));
    app.tap("cover-change");
    assert!(
        app.shown("cover-chooser") && app.shown("cover-reposition") && app.shown("cover-remove")
    );
    // The menu floats under the button, centred on it; pressing the button
    // again closes it (not close-then-reopen), and once more opens it.
    let button = app.rect("cover-change").unwrap();
    let menu = app.rect("cover-chooser").unwrap();
    let below = menu[1] - (button[1] + button[3]);
    assert!((0. ..12.).contains(&below), "{menu:?} under {button:?}");
    assert!(
        menu[0] <= button[0] && menu[0] + menu[2] >= button[0] + button[2],
        "{menu:?} spans {button:?}"
    );
    app.tap("cover-change");
    assert!(!app.shown("cover-chooser"), "a second press closes it");
    app.hover("object-cover");
    app.tap("cover-change");
    assert!(app.shown("cover-chooser"));
    app.tap("cover-reposition");
    assert!(!app.shown("cover-chooser"));
    assert!(app.shown("cover-save") && !app.shown("cover-remove"));
    let from = center(banner);
    app.drag(from, (from.0, from.1 - 60.));
    assert!(
        backend.take_log().is_empty(),
        "nothing saved while dragging"
    );
    app.tap("cover-save");
    let log = backend.take_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].1, json!("cover_y"));
    let y = log[0].2["floatValue"].as_f64().unwrap();
    // A square image under a wide banner: dragging up 60pt shows more of its bottom.
    let overflow = f64::from(banner[2]) - 200.;
    assert!((y - (0.5 + 60. / overflow)).abs() < 1e-3, "{y}");
    assert!(!app.shown("cover-save"));

    // Remove, from the Cover menu, deletes both fields.
    app.hover("object-cover");
    app.tap("cover-change");
    app.tap("cover-remove");
    assert_eq!(
        backend.take_log(),
        vec![deleted("cover"), deleted("cover_y")]
    );
    assert!(!app.shown("object-cover"));

    // Generate, from Add cover: a prompt box; nothing is asked until it has words.
    app.hover("object-title");
    app.tap("cover-add");
    // The cover menu's Generate tab.
    app.tap("cover-chooser-generate");
    assert!(app.shown("cover-chooser") && app.shown("cover-prompt"));
    app.tap("cover-prompt-generate");
    assert_eq!(asked(&backend), None, "an empty prompt asks nothing");
    app.type_text("A misty pine forest");
    // Enter sends it: the box closes and the banner spins while it paints.
    app.key(Key::Enter);
    assert_eq!(asked(&backend).as_deref(), Some("A misty pine forest"));
    assert!(!app.shown("cover-prompt"));
    assert!(app.shown("object-cover") && app.shown("cover-generating"));
    answer(
        &backend,
        &tx,
        &mut app,
        json!({
            "at": { "intValue": 1 },
            "status": { "stringValue": "painting" }, "credential": { "stringValue": CREDENTIAL },
        }),
    );
    assert!(app.shown("cover-generating"));

    // Done: the computer makes the image the cover and ends the run; the spinner goes.
    backend.edit(PAGE, |f| {
        f.remove("cover_generation");
        f.insert("cover".into(), Value::link("file-1", "cover"));
    });
    tx.unbounded_send(Ok(BackendEvent::Committed(PAGE.into())))
        .unwrap();
    app.settle();
    assert!(app.shown("object-cover") && !app.shown("cover-generating"));
    backend.take_log();

    // A failure says why, with the way to the credential when the key is the trouble.
    app.hover("object-cover");
    app.tap("cover-change");
    app.tap("cover-chooser-generate");
    app.type_text("A red barn");
    app.tap("cover-prompt-generate");
    assert_eq!(asked(&backend).as_deref(), Some("A red barn"));
    answer(
        &backend,
        &tx,
        &mut app,
        json!({
            "at": { "intValue": 2 },
            "status": { "stringValue": "failed" }, "credential": { "stringValue": CREDENTIAL },
            "error": { "stringValue": NEEDS_KEY }, "needs_key": { "boolValue": true },
        }),
    );
    assert!(!app.shown("cover-generating"));
    assert!(app.shown("cover-error") && app.shown("cover-open-credential"));
    backend.take_log();
    app.tap("cover-failed-close");
    assert_eq!(backend.take_log(), vec![deleted("cover_generation")]);
    assert!(!app.shown("cover-failed"));
}

#[test]
fn covers_in_a_phones_more_menu() {
    let backend = Arc::new(Vault::new("http://127.0.0.1:9", b"unserved"));
    let (mut app, _) = open(&backend, 390., 844.);

    // The ⋯ menu has one cover entry: it opens the cover menu.
    app.tap("m-more");
    assert!(app.shown("more-cover"));
    app.tap("more-cover");
    assert!(app.shown("cover-chooser"));
    assert!(!app.shown("cover-remove"), "nothing to remove yet");
    // A press outside closes it, as a menu.
    app.tap("object-title");
    assert!(!app.shown("cover-chooser"));

    // With a cover: the menu adds Reposition and Remove; Reposition shows Save / Cancel.
    backend.edit(PAGE, |f| {
        f.insert("cover".into(), Value::link("file-1", "cover"));
    });
    app.state().store.reload_object(PAGE);
    app.settle();
    let banner = app.rect("object-cover").expect("the cover");
    assert_eq!([banner[2], banner[3]], [390., 140.]);
    // No hover on a phone: a tap on the cover opens its menu, another closes it.
    app.tap("object-cover");
    assert!(app.shown("cover-chooser") && app.shown("cover-remove"));
    app.tap("object-cover");
    assert!(!app.shown("cover-chooser"), "a second tap closes it");
    app.tap("m-more");
    app.tap("more-cover");
    app.tap("cover-reposition");
    assert!(app.shown("cover-save") && app.shown("cover-cancel"));
    app.tap("cover-cancel");
    app.tap("m-more");
    app.tap("more-cover");
    app.tap("cover-remove");
    assert_eq!(
        backend.take_log(),
        vec![deleted("cover"), deleted("cover_y")]
    );
    app.tap("m-more");
    app.tap("more-cover");
    app.tap("cover-chooser-generate");
    assert!(app.shown("cover-prompt"));
    // Choose for me asks with no prompt: the computer picks the idea.
    app.tap("cover-prompt-choose");
    assert_eq!(asked(&backend).as_deref(), Some(""));
    assert!(app.shown("cover-generating"));
}
