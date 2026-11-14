//! Images in an object's chat. A message whose meta lists `files` shows each
//! image File as a thumbnail in its bubble (its bytes loaded as an editor
//! image block loads them). An image pasted into the composer is added as a
//! File the usual way, waits above the field, and goes with the next line as
//! `chat_post`'s `files` - with no text needed.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use roostr_core::blossom;
use roostr_core::create::AddFile;
use roostr_core::nav::Route;
use roostr_core::types::{
    Block, BlockContent, CustomContent, Fields, Object, ObjectSummary, QueryResult, RelationDef,
    Settings, Space,
};
use roostr_core::{Backend, BackendEvent, JsonMap, State};
use roostr_rt::{BoxFuture, BoxStream, Task, boxed, boxed_stream};
use rune::Key;
use rune::clipboard::{ClipboardContent, ClipboardImage};
use serde_json::{Value as Json, json};
use support::App;

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";
const FILE: &str = "file-1";
/// 1024 x 1024.
const PNG: &[u8] = include_bytes!("../../../../static/logo.png");
const BLOB_KEY: [u8; 32] = [7u8; 32];

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

fn object(id: &str, type_key: &str, fields: Json) -> Object {
    let fields: Fields = serde_json::from_value(fields).unwrap();
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

/// A page whose chat holds `messages` (each a message's meta).
fn page(messages: Vec<BTreeMap<String, String>>) -> Object {
    let mut p = object(
        PAGE,
        "page",
        json!({ "name": { "stringValue": "Launch plan" }, "channel": { "stringValue": SPACE } }),
    );
    let ids: Vec<String> = (0..messages.len()).map(|i| format!("m{i}")).collect();
    let chat = |content_type: &str, meta| BlockContent {
        custom: Some(CustomContent {
            content_type: content_type.into(),
            meta: Some(meta),
            data: None,
        }),
        ..BlockContent::default()
    };
    p.blocks = vec![Block {
        id: "__discussion__".into(),
        children_ids: ids.clone(),
        content: chat("discussion", BTreeMap::new()),
        ..Block::default()
    }];
    p.blocks
        .extend(ids.into_iter().zip(messages).map(|(id, meta)| Block {
            id,
            content: chat("chat", meta),
            ..Block::default()
        }));
    p
}

/// An image File whose encrypted bytes sit on the Blossom server at `server`.
fn image_file(server: &str, blob: &[u8]) -> Object {
    let blob_sha = blossom::sha256_hex(blob);
    let blob_key: String = BLOB_KEY.iter().map(|b| format!("{b:02x}")).collect();
    object(
        FILE,
        "file",
        json!({
            "name": { "stringValue": "Screenshot.png" },
            "channel": { "stringValue": SPACE },
            "file_mime": { "stringValue": "image/png" },
            "file_size": { "intValue": PNG.len() },
            "file_hash": { "stringValue": blossom::sha256_hex(PNG) },
            "blob_sha256": { "stringValue": blob_sha },
            "blob_key": { "stringValue": blob_key },
            "blob_url": { "stringValue": format!("{server}/{blob_sha}") },
        }),
    )
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

/// The objects, and every mutation's `(action, params)`.
struct Vault {
    objects: Vec<Object>,
    mutations: Mutex<Vec<(String, JsonMap)>>,
}

impl Vault {
    fn new(objects: Vec<Object>) -> Arc<Vault> {
        Arc::new(Vault {
            objects,
            mutations: Mutex::new(Vec::new()),
        })
    }

    fn posts(&self) -> Vec<JsonMap> {
        self.mutations
            .lock()
            .iter()
            .filter(|(action, _)| action == "chat_post")
            .map(|(_, params)| params.clone())
            .collect()
    }
}

struct Handle(Arc<Vault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "a page whose chat shows images".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        ready(Ok(self
            .0
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
        let found = self.0.objects.iter().find(|o| o.id == id).cloned();
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
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        self.0.mutations.lock().push((action.into(), params));
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        boxed_stream(futures::stream::pending())
    }
}

fn keys(app: &App) -> Vec<String> {
    app.rt
        .elements()
        .into_iter()
        .filter_map(|e| e.key)
        .collect()
}

fn key_starting(app: &App, prefix: &str) -> Option<String> {
    keys(app).into_iter().find(|k| k.starts_with(prefix))
}

#[test]
fn a_message_with_an_image_file_shows_its_thumbnail() {
    let blob = blossom::encrypt_blob(PNG, &BLOB_KEY).unwrap();
    let server = serve(blob.clone());
    let meta = BTreeMap::from([
        ("author".to_string(), "someone".to_string()),
        ("ts".to_string(), "1000".to_string()),
        ("text".to_string(), "Here is the screen".to_string()),
        ("files".to_string(), FILE.to_string()),
    ]);
    let vault = Vault::new(vec![page(vec![meta]), image_file(&server, &blob)]);
    let mut app = App::open(Arc::new(Handle(vault)), 1568., 934.);
    app.navigate(Route::Object(PAGE.into()));
    app.settle();

    let thumb = key_starting(&app, "chat-image-")
        .unwrap_or_else(|| panic!("no image thumbnail in the message; keys: {:?}", keys(&app)));
    let rect = app.rect(&thumb).expect("the thumbnail is drawn");
    let bubble = app.rect("msg-text-m0").expect("the bubble is drawn");
    // The square image at the thumbnail width, inside its bubble.
    assert_eq!((rect[2], rect[3]), (240., 240.), "thumbnail {rect:?}");
    assert!(
        rect[0] >= bubble[0]
            && rect[1] >= bubble[1]
            && rect[0] + rect[2] <= bubble[0] + bubble[2]
            && rect[1] + rect[3] <= bubble[1] + bubble[3],
        "the thumbnail {rect:?} is outside its bubble {bubble:?}"
    );

    // Clicking it opens the File.
    app.tap(&thumb);
    assert_eq!(app.rt.state.nav.route(), &Route::Object(FILE.into()));
}

/// Uploads that reached the app's file adder: `(name, mime, space)`.
#[derive(Default)]
struct Uploads(Vec<(String, String, String)>);

#[test]
fn a_pasted_image_goes_with_the_next_message() {
    let vault = Vault::new(vec![page(Vec::new())]);
    let mut app = App::open_with(Arc::new(Handle(vault.clone())), 1568., 934., |s| {
        // The upload itself (harness or Blossom) is the files' own business:
        // record what reaches it and answer with the File it made.
        s.add_file = Some(AddFile(Rc::new(
            |name, mime, _bytes, space, s: &mut State| {
                s.slot::<Uploads>().0.push((name, mime, space));
                Task::ready(Ok("file-9".to_string()))
            },
        )));
    });
    app.navigate(Route::Object(PAGE.into()));
    app.tap("composer");

    // Text pastes as text.
    app.rt.paste("Look");
    app.settle();
    assert!(
        app.state().slot::<Uploads>().0.is_empty(),
        "text was uploaded"
    );

    // An image is added as a File in the page's space and waits above the field.
    app.rt.paste_content(
        ClipboardContent::new("").with_image(ClipboardImage::from_png(PNG).expect("a PNG")),
    );
    app.settle();
    let uploads = app.state().slot::<Uploads>().0.clone();
    assert_eq!(uploads.len(), 1, "the pasted image was not uploaded");
    assert_eq!(
        (uploads[0].1.as_str(), uploads[0].2.as_str()),
        ("image/png", SPACE)
    );
    let tile = key_starting(&app, "attachment-")
        .unwrap_or_else(|| panic!("no attachment above the field; keys: {:?}", keys(&app)));
    assert!(app.shown(&tile));

    app.key(Key::Enter);
    let posts = vault.posts();
    assert_eq!(posts.len(), 1, "nothing was sent");
    assert_eq!(posts[0]["text"], json!("Look"));
    assert_eq!(posts[0]["files"], json!(["file-9"]));
    // The composer is empty again; the sent line shows the image.
    assert_eq!(key_starting(&app, "attachment-"), None);
    assert!(
        key_starting(&app, "chat-image-").is_some(),
        "the sent line shows no image"
    );

    // An image alone is a message too.
    app.rt.paste_content(
        ClipboardContent::new("").with_image(ClipboardImage::from_png(PNG).expect("a PNG")),
    );
    app.settle();
    app.key(Key::Enter);
    let posts = vault.posts();
    assert_eq!(posts.len(), 2, "an image-only message was not sent");
    assert_eq!(posts[1]["text"], json!(""));
    assert_eq!(posts[1]["files"], json!(["file-9"]));
}

#[test]
fn an_image_dragged_onto_the_chat_is_attached_to_the_next_message() {
    let line = BTreeMap::from([
        ("author".to_string(), "someone".to_string()),
        ("ts".to_string(), "1000".to_string()),
        ("text".to_string(), "Send me the screenshot".to_string()),
    ]);
    let vault = Vault::new(vec![page(vec![line])]);
    let mut app = App::open_with(Arc::new(Handle(vault.clone())), 1568., 934., |s| {
        s.add_file = Some(AddFile(Rc::new(
            |name, mime, _bytes, space, s: &mut State| {
                s.slot::<Uploads>().0.push((name, mime, space));
                Task::ready(Ok("file-9".to_string()))
            },
        )));
    });
    app.navigate(Route::Object(PAGE.into()));
    let dir = std::env::temp_dir().join(format!("roostr-chat-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let photo = dir.join("photo.png");
    std::fs::write(&photo, PNG).expect("write the image");

    // Over a message, well above the composer: the whole chat outlines as the drop target.
    app.settle();
    let message = app.rect("msg-text-m0").expect("the message is drawn");
    let over = (message[0] + 20., message[1] + message[3] / 2.);
    app.rt.drag_files(vec![photo.clone()], Some(over));
    app.settle();
    let outline = keys(&app).into_iter().find(|k| k.ends_with("/drop-over"));
    assert!(outline.is_some(), "no drop outline over the chat");

    // Dropped there, it waits above the field and goes with the next message.
    app.rt.drop_files(vec![photo], Some(over));
    app.settle();
    assert!(
        !keys(&app).iter().any(|k| k.ends_with("/drop-over")),
        "the outline stays after the drop"
    );
    let uploads = app.state().slot::<Uploads>().0.clone();
    assert_eq!(uploads.len(), 1, "the dropped image was not uploaded");
    assert_eq!(
        (uploads[0].0.as_str(), uploads[0].1.as_str()),
        ("photo.png", "image/png")
    );
    assert!(
        key_starting(&app, "attachment-").is_some(),
        "no attachment above the field"
    );
    app.tap("composer");
    app.rt.paste("Here");
    app.key(Key::Enter);
    let posts = vault.posts();
    assert_eq!(posts.last().expect("a post")["files"], json!(["file-9"]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_paperclip_attaches_a_file_chosen_in_the_open_dialog() {
    let vault = Vault::new(vec![page(Vec::new())]);
    let dir = std::env::temp_dir().join(format!("roostr-chat-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let photo = dir.join("chosen.png");
    std::fs::write(&photo, PNG).expect("write the image");
    let chosen = photo.clone();
    let mut app = App::open_with(Arc::new(Handle(vault.clone())), 1568., 934., move |s| {
        s.add_file = Some(AddFile(Rc::new(
            |name, mime, _bytes, space, s: &mut State| {
                s.slot::<Uploads>().0.push((name, mime, space));
                Task::ready(Ok("file-9".to_string()))
            },
        )));
        // The Open dialog answers with the file the person chose.
        let chosen = chosen.clone();
        s.pick_file = Some(roostr_core::create::PickFile(Rc::new(move |_| {
            Task::ready(Some(chosen.clone()))
        })));
    });
    app.navigate(Route::Object(PAGE.into()));
    let clip = keys(&app)
        .into_iter()
        .find(|k| k.ends_with("/attach"))
        .unwrap_or_else(|| panic!("no attach button; keys: {:?}", keys(&app)));
    app.tap(&clip);
    app.settle();
    let uploads = app.state().slot::<Uploads>().0.clone();
    assert_eq!(uploads.len(), 1, "the chosen file was not uploaded");
    assert_eq!(uploads[0].0, "chosen.png");
    assert!(
        key_starting(&app, "attachment-").is_some(),
        "no attachment above the field"
    );
    std::fs::remove_dir_all(&dir).ok();
}
