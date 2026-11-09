//! Headless tests of the graph page: a fake vault behind the store, the page in a Rune
//! [`Runtime`], driven with pointer, wheel, pinch and key events. Waits only on the
//! executor's wakes (worker threads answering), never sleeps; the eased zoom and the label
//! delay run on the executor's virtual clock.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::nav::{self, Route};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_rt::{BoxFuture, BoxStream};
use rune::{Cx, El, Env, Key, KeyEvent, MagnifyEvent, Mods, Runtime, WheelEvent, col};
use serde_json::Value as Json;

use crate::scene::Sel;
use crate::view::{Graphs, Page};

const SPACE: &str = "sp-1";
const SIZE: (f32, f32) = (1200., 800.);

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    fields
        .entry("channel".into())
        .or_insert_with(|| Value::string(SPACE));
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        updated_at: 1_000,
        ..Object::default()
    }
}

fn relation(key: &str, format: &str, name: &str) -> RelationDef {
    RelationDef {
        id: format!("rel-{key}"),
        key: key.into(),
        format: format.into(),
        name: name.into(),
        space: Some(SPACE.into()),
        ..RelationDef::default()
    }
}

/// Two types (Task, Person) defined in the space, three tasks using three properties (one an
/// object property limited to people), two people, and a property nothing uses.
fn vault() -> Vault {
    let s = Value::string;
    let objects = vec![
        object(
            "t-task",
            "type",
            &[
                ("key", s("task")),
                ("name", s("Task")),
                ("iconEmoji", s("✅")),
            ],
        ),
        object(
            "t-person",
            "type",
            &[("key", s("person")), ("name", s("Person"))],
        ),
        object(
            "task-1",
            "task",
            &[
                ("name", s("Write docs")),
                ("status", s("open")),
                ("owner", s("person-1")),
                ("due", Value::int(1_700_000_000_000)),
            ],
        ),
        object(
            "task-2",
            "task",
            &[("name", s("Ship it")), ("status", s("done"))],
        ),
        object("task-3", "task", &[("name", s("Plan"))]),
        object("person-1", "person", &[("name", s("Ada"))]),
        object("person-2", "person", &[("name", s("Grace"))]),
    ];
    let mut owner = relation("owner", "object", "Owner");
    owner.object_types = Some(vec!["t-person".into()]);
    let relations = vec![
        relation("status", "text", "Status"),
        owner,
        relation("due", "date", "Due"),
        relation("budget", "number", "Budget"),
    ];
    Vault { objects, relations }
}

struct Vault {
    objects: Vec<Object>,
    relations: Vec<RelationDef>,
}

impl Vault {
    fn row(o: &Object) -> QueryRow {
        QueryRow {
            id: o.id.clone(),
            type_key: o.type_key.clone(),
            name: Some(o.field_str("name").into()),
            fields: o.fields.clone(),
            ..QueryRow::default()
        }
    }
}

struct Shared(Arc<Vault>);

impl Backend for Shared {
    fn describe(&self) -> String {
        "the test vault".into()
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
                channel_id: o.field_str("channel").into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            })
            .collect()))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let o = self.0.objects.iter().find(|o| o.id == id).cloned();
        ready(o.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
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
        ready(Ok(self.0.relations.clone()))
    }
    /// Every object (`{}`), the type and relation definitions (`typeKey in [..]`), or one
    /// type (`{"type": ..}`).
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let keys: Option<Vec<&str>> = body["filters"].as_array().and_then(|f| {
            f.iter()
                .find(|f| f["key"] == "typeKey")
                .and_then(|f| f["value"].as_array())
                .map(|v| v.iter().filter_map(Json::as_str).collect())
        });
        let records: Vec<QueryRow> = self
            .0
            .objects
            .iter()
            .filter(|o| match (&keys, body["type"].as_str()) {
                (Some(keys), _) => keys.contains(&o.type_key.as_str()),
                (None, Some(t)) => o.type_key == t,
                (None, None) => true,
            })
            .map(Vault::row)
            .collect();
        let total = records.len() as u64;
        let offset = body["offset"].as_u64().unwrap_or(0) as usize;
        let limit = body["limit"].as_u64().map_or(usize::MAX, |l| l as usize);
        ready(Ok(QueryResult {
            total,
            records: records.into_iter().skip(offset).take(limit).collect(),
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
        Box::pin(futures::stream::pending())
    }
}

fn window(s: &State, cx: &mut Cx<State>) -> El {
    col().w_full().h_full().child(crate::view(s, cx))
}

/// The graph page in a headless window over the test vault.
struct App {
    rt: Runtime<State>,
    woke: Receiver<()>,
    /// The window's own UI state file, removed with it.
    ui_state: PathBuf,
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.ui_state);
    }
}

impl App {
    fn new(focus: Option<&str>) -> App {
        // Each window remembers its tabs in a file of its own: tests run side by side, and
        // one's saved tabs must not become another's route. The state reads the variable
        // while it is built, under the lock.
        static UI_STATE: Mutex<usize> = Mutex::new(0);
        let (mut s, ui_state) = {
            let mut n = UI_STATE.lock();
            *n += 1;
            let path = std::env::temp_dir().join(format!(
                "roostr-graph-test-{}-{}.json",
                std::process::id(),
                *n
            ));
            let _ = std::fs::remove_file(&path);
            // SAFETY: every test thread builds its state under this lock, where the variable
            // is read.
            unsafe { std::env::set_var("ROOSTR_UI_STATE", &path) };
            let s = State::new(Arc::new(Shared(Arc::new(vault()))), None);
            (s, path)
        };
        let (tx, woke) = channel();
        let tx = Mutex::new(tx);
        s.rt.set_waker(Arc::new(move || {
            let _ = tx.lock().send(());
        }));
        crate::register(&mut s);
        s.nav.use_space(SPACE, &mut s.ui);
        nav::navigate(
            Route::Graph {
                focus: focus.map(str::to_string),
            },
            &mut s,
        );
        let mut rt = Runtime::new(s, window, SIZE, Env::new(true, 2.0));
        // As the app's host runs it: the executor and the event listeners before every frame.
        rt.poll = Some(|s: &mut State, _, _| s.poll().updated);
        let mut app = App { rt, woke, ui_state };
        app.until("the graph", |s| {
            page_of(s).is_some_and(|p| p.status.is_empty() && p.built.is_some() && p.stage[2] > 0.)
        });
        app
    }

    /// One frame (its poll runs the executor and the listeners).
    fn step(&mut self) {
        self.rt.frame(1.0 / 60.0);
    }

    fn until(&mut self, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.step();
            if done(&self.rt.state) {
                self.step();
                self.step();
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            let _ = self.woke.recv_timeout(left.min(Duration::from_millis(20)));
        }
    }

    /// Moves the executor's clock (the eased zoom, the label delay) and draws.
    fn advance(&mut self, by: Duration) {
        let frames = (by.as_millis() / 16).max(1);
        for _ in 0..frames {
            self.rt.state.rt.advance(Duration::from_millis(16));
            self.step();
        }
        self.step();
    }

    fn page(&self) -> &Page {
        page_of(&self.rt.state).expect("a graph page")
    }

    /// Where world point (x, y) shows in the window.
    fn screen(&self, x: f64, y: f64) -> (f32, f32) {
        let p = self.page();
        let (sx, sy) = p.v.project(x, y);
        (p.stage[0] + sx as f32, p.stage[1] + sy as f32)
    }

    fn type_centre(&self, key: &str) -> (f64, f64) {
        let b = self.page().built.clone().expect("built");
        let t = b.g.types.iter().find(|t| t.key == key).expect("the type");
        (t.x, t.y)
    }

    fn object_at(&self, id: &str) -> (f64, f64) {
        let b = self.page().built.clone().expect("built");
        let &(t, j) = b.obj_index.get(id).expect("the object");
        let o = b.object(t, j);
        (o.x, o.y)
    }

    fn texts(&self) -> Vec<String> {
        self.rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.text)
            .collect()
    }

    fn shows(&self, text: &str) -> bool {
        self.texts().iter().any(|t| t == text)
    }

    /// The world element as drawn: its window origin is the camera's pan, its scale the zoom.
    fn world(&self) -> ([f32; 4], f32) {
        let e = self
            .rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("graph-world"))
            .expect("the world");
        (e.rect, e.scale)
    }

    fn click(&mut self, (x, y): (f32, f32)) {
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.step();
    }

    fn hover(&mut self, (x, y): (f32, f32)) {
        self.rt.pointer_move(x, y);
        self.step();
    }

    fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        self.step();
    }

    fn pin(&self) -> Option<Sel> {
        self.page().pin.clone()
    }
}

fn page_of(s: &State) -> Option<&Page> {
    let Route::Graph { focus } = s.nav.route() else {
        return None;
    };
    s.get::<Graphs>()?.pages.get(focus)
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn builds_the_space_graph_and_fits_it() {
    let app = App::new(None);
    let b = app.page().built.clone().unwrap();
    let keys: Vec<&str> = b.g.types.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["person", "task", "type"], "types sorted by name");
    let props: Vec<&str> = b.g.props.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(props.len(), 3, "the used properties: {props:?}");
    assert_eq!(b.g.idle.len(), 1, "budget is unused");
    // The totals panel, the centre caption, and the type names on the ring.
    for text in [
        "TYPES",
        "OBJECTS",
        "PROPERTIES",
        "7",
        "3",
        "3 properties in use",
    ] {
        assert!(app.shows(text), "{text:?} in {:?}", app.texts());
    }
    assert!(app.shows("✅ TASK"), "far label: {:?}", app.texts());
    // Fitted: the world's origin left of centre (the panel takes the right), mid-height,
    // at the scale that frames the whole extent.
    let p = app.page();
    let fit = f64::min(p.stage[2] as f64 - 360., p.stage[3] as f64) / (2. * b.g.extent);
    assert!((p.v.s - fit).abs() < 1e-9, "{} vs {fit}", p.v.s);
    let (rect, scale) = app.world();
    assert!(near(scale, p.v.s as f32, 1e-4), "drawn at {scale}");
    assert!(
        near(rect[0], p.stage[0] + (p.stage[2] - 300.) / 2., 1.),
        "{rect:?}"
    );
    assert!(near(rect[1], p.stage[1] + p.stage[3] / 2., 1.), "{rect:?}");
    assert_eq!(p.level, 0);
    assert!(app.shows("Types as clusters, sized by object count"));
}

#[test]
fn the_wheel_zooms_about_the_pointer_without_rerunning_the_view() {
    let mut app = App::new(None);
    let at = app.screen(200., -150.);
    let before = app.page().v;
    let runs = app.rt.live_only_events();
    app.rt
        .wheel_event(WheelEvent::new(0., 40.).at(at.0, at.1).precise(true));
    app.step();
    let v = app.page().v;
    assert!(
        (v.s / before.s - (40f64 * 0.0016).exp()).abs() < 1e-6,
        "scrolling 40 points zooms by e^(40 × 0.0016): {} → {}",
        before.s,
        v.s
    );
    let after = app.screen(200., -150.);
    assert!(
        near(after.0, at.0, 0.01) && near(after.1, at.1, 0.01),
        "{at:?} → {after:?}"
    );
    let (rect, scale) = app.world();
    assert!(
        near(scale, v.s as f32, 1e-4),
        "the camera draws the new zoom"
    );
    assert!(near(rect[0], app.page().stage[0] + v.x as f32, 0.5));
    assert!(
        app.rt.live_only_events() > runs,
        "a zoom inside one level and level of detail only moves the camera"
    );
    // Scrolling the other way zooms out, and ctrl (a pinch off macOS) zooms faster.
    app.rt
        .wheel_event(WheelEvent::new(0., -40.).at(at.0, at.1).mods(Mods {
            ctrl: true,
            ..Mods::default()
        }));
    app.step();
    assert!((app.page().v.s / v.s - (-0.4f64).exp()).abs() < 1e-6);
}

#[test]
fn a_pinch_zooms_about_the_pointer_and_crosses_levels() {
    let mut app = App::new(None);
    let at = app.screen(0., 0.);
    let s0 = app.page().v.s;
    // Up past the schema threshold (0.44).
    let mut s = s0;
    while s < 0.6 {
        app.rt.magnify_event(MagnifyEvent::new(0.25).at(at.0, at.1));
        app.step();
        s *= 1.25;
    }
    let p = app.page();
    assert!((p.v.s - s).abs() < 1e-9, "{} vs {s}", p.v.s);
    let after = app.screen(0., 0.);
    assert!(near(after.0, at.0, 0.01) && near(after.1, at.1, 0.01));
    assert_eq!(p.level, 1, "Schema");
    assert!(app.shows("Properties appear; lines show which types use them"));
}

#[test]
fn a_drag_pans_and_a_click_pins() {
    let mut app = App::new(None);
    let before = app.page().v;
    let from = app.screen(-600., 600.);
    app.rt.pointer_move(from.0, from.1);
    app.rt.pointer_down(from.0, from.1);
    for k in 1..=10 {
        app.rt
            .pointer_move(from.0 + 12. * k as f32, from.1 - 5. * k as f32);
        app.step();
    }
    app.rt.pointer_up(from.0 + 120., from.1 - 50.);
    app.step();
    let v = app.page().v;
    assert!(near((v.x - before.x) as f32, 120., 0.01) && near((v.y - before.y) as f32, -50., 0.01));
    assert_eq!(app.pin(), None, "a drag pins nothing");
    let (rect, _) = app.world();
    assert!(
        near(rect[0], app.page().stage[0] + v.x as f32, 0.5),
        "the camera follows"
    );

    // A click on the Task cluster pins the type: the panel shows its usage.
    let task = app.type_centre("task");
    let t = app.page().built.clone().unwrap();
    let ti = t.g.types.iter().position(|t| t.key == "task").unwrap();
    app.click(app.screen(task.0 + 20., task.1));
    assert_eq!(app.pin(), Some(Sel::Type(ti)));
    assert!(app.shows("IN USE · 3 OF 4"), "{:?}", app.texts());
    assert!(app.shows("AVAILABLE, UNUSED"));
    assert!(app.shows("Budget"), "the unused property is listed");
    // A click on empty space unpins.
    app.click(app.screen(-1500., 1500.));
    assert_eq!(app.pin(), None);
    assert!(app.shows("TYPES"));
}

#[test]
fn hovering_a_property_shows_its_users() {
    let mut app = App::new(None);
    // Schema level, where properties take the pointer.
    app.rt
        .magnify_event(MagnifyEvent::new(1.).at(app.screen(0., 0.).0, app.screen(0., 0.).1));
    app.step();
    let b = app.page().built.clone().unwrap();
    let owner = b.g.props.iter().find(|p| p.key == "owner").unwrap();
    app.hover(app.screen(owner.x, owner.y));
    assert_eq!(app.page().hover, Some(Sel::Prop("owner".into())));
    assert!(app.shows("USED BY 1 OF 3 TYPES"), "{:?}", app.texts());
    assert!(app.shows("→ Person"), "limited to people");
}

#[test]
fn the_objects_level_pins_objects_and_a_double_click_opens_one() {
    let mut app = App::new(None);
    let (x, y) = app.rt.find("graph-level-2").expect("the Objects pill");
    app.click((x, y));
    app.advance(Duration::from_millis(800));
    let p = app.page();
    assert!((p.v.s - 2.3).abs() < 1e-9, "zoomed to 230%: {}", p.v.s);
    assert_eq!(p.level, 2);
    // The biggest type (Task) is centred.
    let task = app.type_centre("task");
    let c = app.screen(task.0, task.1);
    assert!(
        near(c.0, p.stage[0] + (p.stage[2] - 300.) / 2., 1.),
        "{c:?}"
    );
    // Its object names show once the view rests.
    app.advance(Duration::from_millis(200));
    assert!(!app.page().label_objs.is_empty());
    assert!(app.shows("Write docs"), "{:?}", app.texts());

    let dot = app.object_at("task-1");
    app.click(app.screen(dot.0, dot.1));
    let b = app.page().built.clone().unwrap();
    let &(t, j) = b.obj_index.get("task-1").unwrap();
    assert_eq!(app.pin(), Some(Sel::Obj(t, j)));
    assert!(app.shows("Open object"), "{:?}", app.texts());
    assert!(app.shows("Links to"), "task-1 points at Ada");
    assert!(app.shows("Ada"));

    let at = app.screen(dot.0, dot.1);
    app.rt.pointer_down(at.0, at.1);
    app.rt.pointer_up(at.0, at.1);
    app.step();
    assert_eq!(
        app.rt.state.nav.route(),
        &Route::Object("task-1".into()),
        "a double click opens the object"
    );
}

#[test]
fn the_finder_flies_to_what_it_finds() {
    let mut app = App::new(None);
    let field = app.rt.find("graph-find-field").expect("the finder");
    app.click(field);
    app.key(Key::Text("gra".into()));
    let found: Vec<String> = app.page().found.iter().map(|f| f.name.clone()).collect();
    assert_eq!(found, ["Grace"]);
    assert!(app.shows("Grace"));
    app.key(Key::Enter);
    app.advance(Duration::from_millis(800));
    let b = app.page().built.clone().unwrap();
    let &(t, j) = b.obj_index.get("person-2").unwrap();
    assert_eq!(app.pin(), Some(Sel::Obj(t, j)));
    assert!((app.page().v.s - 2.3).abs() < 1e-9);
    let grace = app.object_at("person-2");
    let at = app.screen(grace.0, grace.1);
    let p = app.page();
    assert!(near(at.0, p.stage[0] + (p.stage[2] - 300.) / 2., 1.));
    assert!(near(at.1, p.stage[1] + p.stage[3] / 2., 1.));
    assert_eq!(p.query.value(), "", "the finder clears");

    // No match says so; Escape clears.
    app.click(field);
    app.key(Key::Text("zzz".into()));
    assert!(app.shows("No objects or properties match."));
    app.key(Key::Escape);
    assert_eq!(app.page().query.value(), "");
}

#[test]
fn a_focus_route_pins_and_zooms_to_its_object() {
    let mut app = App::new(Some("person-1"));
    app.advance(Duration::from_millis(800));
    let b = app.page().built.clone().unwrap();
    let &(t, j) = b.obj_index.get("person-1").unwrap();
    assert_eq!(app.pin(), Some(Sel::Obj(t, j)));
    assert!((app.page().v.s - 2.3).abs() < 1e-9);
    assert!(app.shows("Linked from"), "Write docs points at Ada");
}
