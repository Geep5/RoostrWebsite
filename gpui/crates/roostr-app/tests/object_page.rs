//! The object page in the assembled app: its title saves when left (even
//! with a store refresh landing between the leave and the save), a tap on a
//! query's title edits it on a phone and on the desktop, a chat object
//! opened on a phone shows as the full-screen chat, and an object opened
//! from an icon has that icon fly to its header.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::nav::{self, Route};
use roostr_core::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use roostr_ui::motion::{self, IconFlight};
use rune::{Color, Cx, El, Key, el};
use serde_json::Value as Json;
use support::App;

const PAGE: &str = "page-1";
const QUERY: &str = "query-1";
const CHAT: &str = "chat-1";

/// A vault of an untitled page, an untitled query and a chat that keeps
/// every mutation it is sent.
#[derive(Default)]
struct Vault {
    /// Saved names by object id.
    names: Mutex<HashMap<String, String>>,
    mutations: Mutex<Vec<(String, JsonMap)>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    boxed(std::future::ready(value))
}

struct Handle(Arc<Vault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "a page, a query and a chat".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        let summary = |id: &str, type_key: &str| ObjectSummary {
            id: id.into(),
            type_key: type_key.into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        };
        ready(Ok(vec![
            summary(PAGE, "page"),
            summary(QUERY, "query"),
            summary(CHAT, "chat"),
        ]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let type_key = match id {
            PAGE => "page",
            QUERY => "query",
            CHAT => "chat",
            _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
        };
        let name = self.0.names.lock().get(id).cloned().unwrap_or_default();
        ready(Ok(Object {
            id: id.into(),
            type_key: type_key.into(),
            fields: [
                ("name".to_string(), Value::string(name)),
                ("channel".to_string(), Value::string("sp-1")),
            ]
            .into(),
            ..Object::default()
        }))
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
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        ready(Ok(QueryResult::default()))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        if action == "set_field" && params["key"] == "name" {
            let id = params["object_id"].as_str().unwrap_or("");
            let name = params["value"]["stringValue"].as_str().unwrap_or("");
            self.0.names.lock().insert(id.to_string(), name.to_string());
        }
        self.0.mutations.lock().push((action.to_string(), params));
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

/// Whether `object`'s name was ever saved as `name`.
fn saved(vault: &Vault, object: &str, name: &str) -> bool {
    vault.mutations.lock().iter().any(|(action, params)| {
        action == "set_field"
            && params["object_id"] == object
            && params["key"] == "name"
            && params["value"]["stringValue"] == name
    })
}

fn open(vault: &Arc<Vault>, id: &str, width: f32, height: f32) -> App {
    let mut app = App::open(Arc::new(Handle(vault.clone())), width, height);
    app.navigate(Route::Object(id.into()));
    app
}

/// The title field's text and whether it holds the focus.
fn title(app: &App) -> (String, bool) {
    let field = app
        .rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("object-title-field"))
        .expect("the title field");
    (field.text.clone().unwrap_or_default(), field.focused)
}

/// A title typed into a new page and left at once is saved. A store refresh
/// landing between the leave and the save (a fresh vault's first sync brings
/// many) must not put the stored, still empty, name back over the draft.
#[test]
fn a_title_left_mid_refresh_is_saved() {
    let vault = Arc::new(Vault::default());
    let mut app = open(&vault, PAGE, 1280., 800.);
    app.tap("object-title");
    assert!(title(&app).1, "the click focused the title");
    app.type_text("E2E Page");
    assert_eq!(title(&app).0, "E2E Page");

    // Leave it (a click on empty page space), with a refresh queued.
    app.state().store.refresh_all();
    app.click_at(640., 760.);
    assert!(!title(&app).1, "the title kept the focus");
    assert_eq!(title(&app).0, "E2E Page", "the refresh took the draft");
    assert!(
        saved(&vault, PAGE, "E2E Page"),
        "the title was never saved: {:?}",
        vault.mutations.lock()
    );
    assert_eq!(title(&app).0, "E2E Page");
}

/// A tap on a new query's title edits it, on a phone and on the desktop: a
/// press on a query's page arms its table's rubber band, and must leave a
/// press the title took to the title.
#[test]
fn a_querys_title_takes_a_tap() {
    let vault = Arc::new(Vault::default());
    // iPhone 13 Pro Max portrait: the mobile layout.
    let mut app = open(&vault, QUERY, 428., 926.);
    app.tap("object-title");
    assert!(title(&app).1, "the tap left the title unfocused");
    app.type_text("Reading list");
    assert_eq!(title(&app).0, "Reading list");
    // Enter leaves the title and saves it.
    app.key(Key::Enter);
    assert!(
        saved(&vault, QUERY, "Reading list"),
        "the title was never saved: {:?}",
        vault.mutations.lock()
    );

    let mut app = open(&vault, QUERY, 1280., 800.);
    app.tap("object-title");
    assert!(title(&app).1, "the click left the title unfocused");
}

/// On a phone a chat object IS its discussion: opening it shows the
/// full-screen chat page in its place (not stacked: Back goes home), and
/// every object opens with the pane's sheet closed.
#[test]
fn a_chat_object_on_a_phone_opens_as_its_chat() {
    let vault = Arc::new(Vault::default());
    let mut app = open(&vault, CHAT, 390., 844.);
    assert_eq!(app.state().nav.route(), &Route::Chat(CHAT.into()));
    let s = app.state();
    s.nav.back(&mut s.ui);
    app.settle();
    assert_eq!(
        app.state().nav.route(),
        &Route::Home,
        "the chat replaced the object"
    );

    // An object opens without the sheet a previous one left up.
    app.state().pane.set_sheet(true);
    app.navigate(Route::Object(PAGE.into()));
    assert!(!app.state().pane.sheet, "the sheet stayed up");

    // On the desktop the chat object has its page.
    let mut app = open(&vault, CHAT, 1280., 800.);
    assert_eq!(app.state().nav.route(), &Route::Object(CHAT.into()));
    assert!(app.shown("object-page"));
}

/// Where the stand-in source icon sits: `[x, y, w, h]`.
const SOURCE: [f32; 4] = [600., 700., 20., 20.];

/// The stand-in source icon's matched id (a row icon showing `PAGE`).
fn source_id() -> String {
    motion::icon_id(&format!("test:{PAGE}"))
}

/// The app's window with a source icon of `PAGE` over it, as a sidebar
/// row or a card would show one.
fn with_source(s: &State, cx: &mut Cx<State>) -> El {
    let flight = s.get::<IconFlight>().unwrap_or(&IconFlight::IDLE);
    let icon = el()
        .absolute(SOURCE[0], SOURCE[1])
        .size(SOURCE[2], SOURCE[3])
        .fill(Color::WHITE);
    roostr_shell::view(s, cx).child(flight.source(icon, &source_id()))
}

/// An object opened from an icon (launched, then navigated to in one
/// handler) has its page header's icon fly in from that icon's rect to its
/// place in the header, carrying on as the page finishes opening; under
/// Reduce Motion it is in place at once.
#[test]
fn an_opened_objects_icon_flies_to_the_page_header() {
    for reduced in [false, true] {
        let vault = Arc::new(Vault::default());
        let mut app = App::open_view(
            Arc::new(Handle(vault.clone())),
            1280.,
            800.,
            |_| {},
            with_source,
        );
        app.navigate(Route::Object(PAGE.into()));
        let header = app.rect("obj-icon-face").expect("the header icon");
        app.navigate(Route::Home);
        app.rt.set_system_reduced_motion(reduced);
        app.rt.frame(1.0 / 60.0);
        let source = source_id();
        assert_eq!(app.rect(&source), Some(SOURCE));

        // The press: launch, then navigate. The page's state opens at the
        // shell's next poll, so this commit draws the opening page.
        let from = source.clone();
        app.rt.update(move |s, tx| {
            s.slot::<IconFlight>().launch(PAGE, from, tx);
            nav::navigate(Route::Object(PAGE.into()), s);
        });
        app.rt.frame(1.0 / 120.0);
        let target = format!("obj-icon-face<{source}");
        let first = app
            .rt
            .presented(&target)
            .expect("the launched header icon")
            .0;
        let near = |a: [f32; 4], b: [f32; 4]| (0..4).all(|i| (a[i] - b[i]).abs() < 8.0);
        if reduced {
            // In place: the opening page is short, so the column sits a few
            // points over until the loaded page's scroll bar narrows it.
            assert!(
                near(first, header),
                "reduced motion: in place at once: {first:?}"
            );
        } else {
            assert!(near(first, SOURCE), "starts at the source icon: {first:?}");
            // The source stays, keyed apart and without the id.
            assert!(app.rt.presented(&format!("{source}#launched")).is_some());
        }

        // The page opens (the poll the window runs each frame) as it flies.
        for _ in 0..3 {
            step(&mut app);
        }
        assert!(app.rt.find("object-title").is_some(), "the page opened");
        let now = app.rt.presented(&target).expect("the header icon").0;
        if reduced {
            assert_eq!(now, header, "reduced motion: stays in place");
            continue;
        }
        assert!(
            now[1] < first[1] - 1.0 && now[1] > header[1] + 1.0,
            "still flying from the source to the header {header:?}: {first:?} → {now:?}"
        );
        for _ in 0..240 {
            step(&mut app);
        }
        assert_eq!(app.rt.presented(&target).expect("landed").0, header);
    }
}

/// One 120 Hz frame after the poll the window runs each frame.
fn step(app: &mut App) {
    app.rt.update_if(|s, tx| roostr_host::poll(s, 0., tx));
    app.rt.frame(1.0 / 120.0);
}
