//! A table cell's link editor can be used. A saved query's table is wider
//! than the page column, which clips it before the chat pane: a cell near
//! that edge used to open its editor at the cell, mostly past the clip, and
//! picking from its link menu (which floated outside the editor) closed the
//! editor before the pick was saved - "it won't let me add another game".
//! The editor now opens inside the table's visible width with its menu in
//! its own flow, and a second game is added to the first.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::{point, px, size};
use parking_lot::Mutex;
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Fields, Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use serde_json::{Value as Json, json};

const SPACE: &str = "sp-1";
const PUBLISHER: &str = "pub-1";
const CELL: &str = "cell-pub-1-main_games";

/// One space: Game and Game Publisher types, two games, a publisher whose
/// Main games (no type limit, any number) holds TYR, and a query of publishers
/// showing Main games.
struct Vault {
    objects: Mutex<Vec<Object>>,
}

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, pairs: &[(&str, Value)]) -> Object {
    let mut fields: Fields = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    fields.insert("channel".into(), Value::string(SPACE));
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields,
        ..Object::default()
    }
}

impl Default for Vault {
    fn default() -> Self {
        // A game named like Tanks in another space: never offered here.
        let mut elsewhere = object("g9", "game", &[("name", Value::string("Tanks Elsewhere"))]);
        elsewhere
            .fields
            .insert("channel".into(), Value::string("sp-2"));
        let objects = vec![
            object("type-game", "type", &[("name", Value::string("Game")), ("key", Value::string("game"))]),
            object("type-pub", "type", &[("name", Value::string("Game Publisher")), ("key", Value::string("game_publisher"))]),
            object("g1", "game", &[("name", Value::string("TYR"))]),
            object("g2", "game", &[("name", Value::string("Tanks"))]),
            elsewhere,
            object(PUBLISHER, "game_publisher", &[("name", Value::string("Stoke")), ("main_games", Value::values(vec![Value::link("g1", "main_games")]))]),
            object("q1", "query", &[("name", Value::string("Publishers")), ("setOf", Value::values(vec![Value::string("game_publisher")])),
                ("viewRelations", serde_json::from_value(json!({"valuesValue":{"items":[{"mapValue":{"entries":{"key":{"stringValue":"main_games"},"width":{"intValue":150}}}}]}})).unwrap()),
                // Filtered on Priority without showing it as a column.
                ("viewFilters", serde_json::from_value(json!({"valuesValue":{"items":[{"mapValue":{"entries":{"key":{"stringValue":"priority"},"condition":{"stringValue":"equal"},"value":{"valuesValue":{"items":[{"stringValue":"High"}]}}}}}]}})).unwrap())]),
        ];
        Self {
            objects: Mutex::new(objects),
        }
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "publishers and their games".into()
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
            .lock()
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
        let object = self.objects.lock().iter().find(|o| o.id == id).cloned();
        ready(object.ok_or_else(|| anyhow::anyhow!("unknown object {id}")))
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
        ready(Ok(vec![
            RelationDef {
                id: "rel-tasks".into(),
                key: "main_games".into(),
                format: "object".into(),
                name: "Main Games".into(),
                object_types: None,
                space: Some(SPACE.into()),
                ..RelationDef::default()
            },
            RelationDef {
                id: "rel-priority".into(),
                key: "priority".into(),
                format: "shorttext".into(),
                name: "Priority".into(),
                space: Some(SPACE.into()),
                ..RelationDef::default()
            },
        ]))
    }
    /// The type definitions; nothing else is listed here.
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        let text = body.to_string();
        let records: Vec<QueryRow> = self
            .objects
            .lock()
            .iter()
            .filter(|o| {
                (text.contains("\"setId\":\"q1\"") && o.type_key == "game_publisher")
                    || (!text.contains("setId")
                        && text.contains(&format!("\"{}\"", o.type_key))
                        && o.type_key != "query")
                    // The property pane's "views this object is in" lookup.
                    || (text.contains("\"collection\"") && o.type_key == "query")
            })
            .map(|o| QueryRow {
                id: o.id.clone(),
                type_key: o.type_key.clone(),
                name: Some(o.field_str("name").into()),
                // Like the store: a query that names its fields gets only those.
                fields: match body["fields"].as_array() {
                    Some(keys) => o
                        .fields
                        .iter()
                        .filter(|(k, _)| keys.iter().any(|f| f == k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    None => o.fields.clone(),
                },
                ..QueryRow::default()
            })
            .collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        let text = |key: &str| params[key].as_str().unwrap_or_default().to_string();
        let result = JsonMap::new();
        match action {
            "set_field" => {
                let value: Value =
                    serde_json::from_value(params["value"].clone()).expect("set_field value");
                let id = text("object_id");
                if let Some(o) = self.objects.lock().iter_mut().find(|o| o.id == id) {
                    o.fields.insert(text("key"), value);
                }
            }
            _ => {}
        }
        ready(Ok(result))
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

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
}

/// Click a point near the left edge of `selector` (its visible part).
fn click_left(cx: &mut VisualTestContext, selector: &'static str) {
    let b = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"));
    cx.simulate_click(point(b.origin.x + px(8.), b.center().y), Modifiers::none());
    cx.run_until_parked();
}

/// The games the publisher's Main games links, as the backend holds them.
fn main_games(vault: &Vault) -> Vec<String> {
    vault
        .objects
        .lock()
        .iter()
        .find(|o| o.id == PUBLISHER)
        .and_then(|o| o.field("main_games"))
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
fn a_cell_editor_at_the_page_columns_edge_opens_where_it_can_be_used(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    let backend = Arc::new(Vault::default());
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(backend.clone(), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) = cx.add_window_view(|window, cx| {
        roostr_shell::Workspace::new(roostr_app::pages(), window, cx)
    });
    cx.simulate_resize(size(px(1400.), px(900.)));
    settle(cx);
    cx.update(|_, cx| navigate(Route::Object("q1".into()), cx));
    settle(cx);
    // A linked game shows as its own pill (emoji and name), not a comma list.
    assert!(
        cx.debug_bounds("pill-pub-1-main_games-g1").is_some(),
        "TYR is a pill in the Main games cell"
    );
    // Clicking the pill opens the game, not the cell's editor (the first
    // press on the page opens its side columns, as below).
    for _ in 0..2 {
        let Some(pill) = cx.debug_bounds("pill-pub-1-main_games-g1") else {
            break;
        };
        cx.simulate_click(pill.center(), Modifiers::none());
        settle(cx);
        assert!(
            cx.debug_bounds("link-object").is_none(),
            "no cell editor from a pill"
        );
    }
    assert_eq!(
        cx.update(|_, cx| roostr_core::nav::Navigator::global(cx)
            .read(cx)
            .route()
            .clone()),
        Route::Object("g1".into())
    );
    cx.update(|_, cx| navigate(Route::Object("q1".into()), cx));
    settle(cx);
    // The side columns are open: the table is wider than the page column. With
    // the pointer over it and no text field focused (as after a click outside
    // one), → scrolls it sideways, ← back.
    cx.update(|window, cx| window.blur(cx));
    let cell = cx.debug_bounds(CELL).expect("the Main games cell");
    let before = cell.left();
    // Its visible part: the cell runs past the table's clipped edge.
    cx.simulate_mouse_move(
        point(cell.origin.x + px(8.), cell.center().y),
        None,
        Modifiers::none(),
    );
    cx.simulate_keystrokes("right");
    settle(cx);
    let scrolled = cx.debug_bounds(CELL).unwrap().left();
    assert!(
        scrolled < before,
        "→ scrolled the table: {before:?} → {scrolled:?}"
    );
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(
        cx.debug_bounds(CELL).unwrap().left(),
        before,
        "← scrolled it back"
    );
    // The first press on the page opens its side columns (done above, by the
    // pill), which narrow the page column to where the user saw it: the table
    // clipped at Main games. A press on the cell beside its pill opens its editor.
    click_left(cx, CELL);
    settle(cx);
    if cx.debug_bounds("link-object").is_none() {
        click_left(cx, CELL);
    }
    // A cell that already links something opens on its chips: + adds another.
    let add = cx
        .debug_bounds("link-object")
        .expect("the cell's editor")
        .center();
    cx.simulate_click(add, Modifiers::none());
    cx.run_until_parked();
    let menu = cx.debug_bounds("obj-menu").expect("the cell's link picker");
    let cell = cx.debug_bounds(CELL).unwrap();
    let title = cx.debug_bounds("object-title").unwrap();
    // The cell sits at the clipped edge; its picker opens inside the page column.
    assert!(
        menu.right() > cell.left(),
        "{menu:?} reaches the cell {cell:?}"
    );
    assert!(
        menu.left() < cell.left(),
        "{menu:?} pulled left of {cell:?}"
    );
    assert!(
        menu.left() >= title.left(),
        "{menu:?} inside the column {title:?}"
    );

    // The second game is offered and added after TYR.
    cx.simulate_input("tan");
    cx.run_until_parked();
    // Typing in the link search: → at the end of the text belongs to the
    // field, never scrolls the table under the pointer.
    let cell = cx.debug_bounds(CELL).unwrap();
    cx.simulate_mouse_move(
        point(cell.origin.x + px(8.), cell.center().y),
        None,
        Modifiers::none(),
    );
    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(
        cx.debug_bounds(CELL).unwrap().left(),
        cell.left(),
        "→ while typing left the table where it was"
    );
    // The cell's picker offers the row's own space only: the same-named game
    // in another space is not a candidate (the row came without its channel).
    assert!(
        cx.debug_bounds("cand-g9").is_none(),
        "a game from another space is offered"
    );
    let at = cx
        .debug_bounds("cand-g2")
        .expect("Tanks is offered")
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(main_games(&backend), ["g1", "g2"]);

    // A publisher's own page: the query over its type filters on Priority,
    // so Priority shows among its properties though it is empty and no column.
    cx.update(|_, cx| navigate(Route::Object(PUBLISHER.into()), cx));
    settle(cx);
    assert!(
        cx.debug_bounds("prop-row-main_games").is_some(),
        "the property pane is open on the publisher"
    );
    assert!(
        cx.debug_bounds("prop-row-priority").is_some(),
        "a property the query filters on is offered on its objects"
    );
}
