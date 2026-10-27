//! Page covers: Add cover (hovering the icon and title) adds a pasted image
//! the usual way and sets `cover` + `cover_y`; the banner sits above the
//! title with the icon overlapping its bottom edge; Reposition drags the
//! focus and saves `cover_y`; Remove deletes both. Generate asks once and
//! spins the banner until the serving computer's image replaces the cover;
//! a failure shows its reason, and closing it deletes the run. A phone has
//! the same actions in its ⋯ menu.
#![cfg(not(target_family = "wasm"))]

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use gpui::{
    ClipboardItem, Image, ImageFormat, Modifiers, MouseButton, Task, TestAppContext,
    VisualTestContext,
};
use gpui_kit::{point, px, size};
use parking_lot::Mutex;
use roostr_core::blossom;
use roostr_core::create::AddFile;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_shell::Workspace;
use serde_json::{Value as Json, json};

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
    Box::pin(std::future::ready(value))
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

impl Backend for Vault {
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
            self.objects
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
        if action != "vanish" && !self.objects.lock().contains_key(&id) {
            return ready(Ok(JsonMap::new()));
        }
        match action {
            "set_field" => {
                let value: Value = serde_json::from_value(params["value"].clone()).unwrap();
                self.edit(&id, |f| {
                    f.insert(key, value);
                });
            }
            "delete_field" => self.edit(&id, |f| {
                f.remove(&key);
            }),
            "vanish" => {}
            _ => return ready(Ok(JsonMap::new())),
        }
        self.log.lock().push((action.into(), Json::Object(params)));
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

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn tap(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
}

fn hover(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
        .center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    settle(cx);
}

/// The app on the page in a `width` x `height` window, its uploads recorded.
fn open<'a>(
    cx: &'a mut TestAppContext,
    backend: Arc<Vault>,
    width: f32,
    height: f32,
) -> (
    &'a mut VisualTestContext,
    Rc<RefCell<Vec<(String, String, String)>>>,
) {
    let uploads: Rc<RefCell<Vec<(String, String, String)>>> = Rc::default();
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend, None, cx);
        roostr_app::init(cx);
        // The upload itself (harness or Blossom) is the files' own business:
        // record what reaches it and answer with the File it made.
        let seen = uploads.clone();
        cx.set_global(AddFile(Rc::new(move |name, mime, _bytes, space, _cx| {
            seen.borrow_mut().push((name, mime, space));
            Task::ready(Ok("file-1".to_string()))
        })));
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(width), px(height)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object(PAGE.into()), cx));
    settle(cx);
    (cx, uploads)
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
    cx: &mut VisualTestContext,
    entries: Json,
) {
    backend.edit(PAGE, |f| {
        f.remove("cover_request");
        f.insert("cover_generation".into(), generation(entries));
    });
    tx.unbounded_send(Ok(BackendEvent::Committed(PAGE.into())))
        .unwrap();
    settle(cx);
}

/// The pending `cover_request`'s (action, prompt).
fn asked(backend: &Vault) -> Option<(String, Option<String>)> {
    let objects = backend.objects.lock();
    let entries = objects[PAGE]
        .fields
        .get("cover_request")?
        .entries()?
        .clone();
    Some((
        entries["action"].as_str().unwrap().to_string(),
        entries
            .get("prompt")
            .and_then(|p| p.as_str())
            .map(str::to_string),
    ))
}

#[gpui::test]
fn covers_on_desktop_and_phone(desktop: &mut TestAppContext, phone: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before either app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    on_a_desktop_page(desktop);
    in_a_phones_more_menu(phone);
}

fn on_a_desktop_page(cx: &mut TestAppContext) {
    let blob = blossom::encrypt_blob(PNG, &BLOB_KEY).unwrap();
    let server = serve(blob.clone());
    let backend = Arc::new(Vault::new(&server, &blob));
    let (tx, rx) = mpsc::unbounded();
    *backend.events.lock() = Some(rx);
    let (cx, uploads) = open(cx, backend.clone(), 1280., 1000.);

    // No cover: hovering the icon and title offers Add cover.
    assert!(!shown(cx, "object-cover") && !shown(cx, "cover-add"));
    hover(cx, "object-title");
    assert!(shown(cx, "cover-add"));
    tap(cx, "cover-add");
    assert!(shown(cx, "cover-chooser") && shown(cx, "cover-pick"));

    // Paste an image: it is added the usual way, then becomes the cover.
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            PNG.to_vec(),
        )))
    });
    tap(cx, "cover-paste");
    let added = uploads.borrow().clone();
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
    assert!(!shown(cx, "cover-chooser"));

    // The banner spans the page above the title; the icon overlaps its bottom edge.
    let banner = cx.debug_bounds("object-cover").expect("the cover");
    let title = cx.debug_bounds("object-title").unwrap();
    let icon = cx.debug_bounds("object-icon").unwrap();
    assert_eq!(banner.size.height, px(200.));
    assert!(banner.bottom() <= title.top(), "{banner:?} above {title:?}");
    assert!(icon.top() < banner.bottom() && banner.bottom() < icon.bottom());
    assert!(banner.size.width > title.size.width);

    // Hovering the cover shows its actions; Reposition drags the focus.
    hover(cx, "object-cover");
    for action in [
        "cover-change",
        "cover-reposition",
        "cover-remove",
        "cover-generate",
    ] {
        assert!(shown(cx, action), "{action}");
    }
    tap(cx, "cover-reposition");
    assert!(shown(cx, "cover-save") && !shown(cx, "cover-remove"));
    let from = banner.center();
    let to = point(from.x, from.y - px(60.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    settle(cx);
    assert!(
        backend.take_log().is_empty(),
        "nothing saved while dragging"
    );
    tap(cx, "cover-save");
    let log = backend.take_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].1, json!("cover_y"));
    let y = log[0].2["floatValue"].as_f64().unwrap();
    // A square image under a wide banner: dragging up 60px shows more of its bottom.
    let overflow = f64::from(f32::from(banner.size.width)) - 200.;
    assert!((y - (0.5 + 60. / overflow)).abs() < 1e-3, "{y}");
    assert!(!shown(cx, "cover-save"));

    // Remove deletes both fields.
    hover(cx, "object-cover");
    tap(cx, "cover-remove");
    assert_eq!(
        backend.take_log(),
        vec![deleted("cover"), deleted("cover_y")]
    );
    assert!(!shown(cx, "object-cover"));

    // Generate, from Add cover: one ask, no panel; the banner spins while it runs.
    hover(cx, "object-title");
    tap(cx, "cover-add");
    tap(cx, "cover-chooser-generate");
    assert!(!shown(cx, "cover-chooser"));
    assert_eq!(asked(&backend), Some(("generate".into(), None)));
    assert!(shown(cx, "object-cover") && shown(cx, "cover-generating"));
    answer(
        &backend,
        &tx,
        cx,
        json!({
            "at": { "intValue": 1 }, "action": { "stringValue": "generate" },
            "status": { "stringValue": "painting" }, "credential": { "stringValue": CREDENTIAL },
        }),
    );
    assert!(shown(cx, "cover-generating"));

    // Done: the computer makes the image the cover and ends the run; the spinner goes.
    backend.edit(PAGE, |f| {
        f.remove("cover_generation");
        f.insert("cover".into(), Value::link("file-1", "cover"));
    });
    tx.unbounded_send(Ok(BackendEvent::Committed(PAGE.into())))
        .unwrap();
    settle(cx);
    assert!(shown(cx, "object-cover") && !shown(cx, "cover-generating"));
    backend.take_log();

    // A failure says why, with the way to the credential when the key is the trouble.
    hover(cx, "object-cover");
    tap(cx, "cover-generate");
    answer(
        &backend,
        &tx,
        cx,
        json!({
            "at": { "intValue": 2 }, "action": { "stringValue": "generate" },
            "status": { "stringValue": "failed" }, "credential": { "stringValue": CREDENTIAL },
            "error": { "stringValue": NEEDS_KEY }, "needs_key": { "boolValue": true },
        }),
    );
    assert!(!shown(cx, "cover-generating"));
    assert!(shown(cx, "cover-error") && shown(cx, "cover-open-credential"));
    backend.take_log();
    tap(cx, "cover-failed-close");
    assert_eq!(backend.take_log(), vec![deleted("cover_generation")]);
    assert!(!shown(cx, "cover-failed"));
}

fn in_a_phones_more_menu(cx: &mut TestAppContext) {
    let backend = Arc::new(Vault::new("http://127.0.0.1:9", b"unserved"));
    let (cx, _) = open(cx, backend.clone(), 390., 844.);

    tap(cx, "m-more");
    assert!(shown(cx, "more-cover-add") && shown(cx, "more-cover-generate"));
    assert!(!shown(cx, "more-cover-remove"));
    tap(cx, "more-cover-add");
    assert!(shown(cx, "cover-chooser"));
    tap(cx, "cover-chooser-close");

    // With a cover: Change, Reposition, Remove, Generate; Reposition shows Save / Cancel.
    backend.edit(PAGE, |f| {
        f.insert("cover".into(), Value::link("file-1", "cover"));
    });
    cx.update(|_, cx| roostr_core::Store::global(cx).update(cx, |s, cx| s.reload_object(PAGE, cx)));
    settle(cx);
    let banner = cx.debug_bounds("object-cover").expect("the cover");
    assert_eq!(banner.size, size(px(390.), px(140.)));
    tap(cx, "m-more");
    for item in [
        "more-cover-change",
        "more-cover-reposition",
        "more-cover-remove",
        "more-cover-generate",
    ] {
        assert!(shown(cx, item), "{item}");
    }
    tap(cx, "more-cover-reposition");
    assert!(shown(cx, "cover-save") && shown(cx, "cover-cancel"));
    tap(cx, "cover-cancel");
    tap(cx, "m-more");
    tap(cx, "more-cover-remove");
    assert_eq!(
        backend.take_log(),
        vec![deleted("cover"), deleted("cover_y")]
    );
    tap(cx, "m-more");
    tap(cx, "more-cover-generate");
    assert_eq!(asked(&backend), Some(("generate".into(), None)));
    assert!(shown(cx, "cover-generating"));
}
