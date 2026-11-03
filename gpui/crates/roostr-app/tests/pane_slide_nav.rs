//! A pane column sliding open leaves the nav alone: each frame of the slide
//! lays out what changes size (the page, the column), not the nav. Field
//! report: opening Properties again was laggy - every frame of the slide
//! laid out the whole nav (its pinned lists) again, because the slide asked
//! for its frames by notifying the workspace the nav redraws with.
#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::time::Duration;

use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{px, size};
use roostr_core::io::{BoxFuture, BoxStream};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap, PaneState, PaneTab};
use roostr_shell::Workspace;
use serde_json::Value as Json;

/// One space: a page, and a pinned query listing a few agents.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn object(id: &str, type_key: &str, name: &str) -> Object {
    Object {
        id: id.into(),
        type_key: type_key.into(),
        fields: [
            ("name".to_string(), Value::string(name)),
            ("channel".to_string(), Value::string("sp-1")),
        ]
        .into(),
        ..Object::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "a page and a pinned query".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        let summary = |id: &str, type_key: &str, name: &str| ObjectSummary {
            id: id.into(),
            type_key: type_key.into(),
            name: name.into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        };
        ready(Ok(vec![
            summary("page-1", "page", "Launch plan"),
            summary("q-1", "query", "Agents"),
        ]))
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        ready(match id {
            "page-1" => Ok(object(id, "page", "Launch plan")),
            "q-1" => Ok(object(id, "query", "Agents")),
            _ => Err(anyhow::anyhow!("unknown object {id}")),
        })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: vec!["q-1".into()],
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        if body["setId"] != "q-1" {
            return ready(Ok(QueryResult::default()));
        }
        let records: Vec<QueryRow> = (0..20)
            .map(|n| QueryRow {
                id: format!("agent-{n}"),
                type_key: "agent".into(),
                name: Some(format!("Agent {n}")),
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
        Box::pin(futures::stream::pending())
    }
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn props_width(cx: &mut VisualTestContext) -> f32 {
    cx.debug_bounds("pane-col-Properties")
        .map_or(0., |b| f32::from(b.size.width))
}

#[gpui::test]
fn a_pane_column_sliding_open_does_not_lay_the_nav_out_again(cx: &mut TestAppContext) {
    let ui_state = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only test, before the app starts.
    unsafe { std::env::set_var("ROOSTR_UI_STATE", ui_state.path().join("ui-state.json")) };
    cx.update(|cx| {
        gpui_kit::init(cx);
        roostr_core::init(Arc::new(Vault), None, cx);
        roostr_app::init(cx);
    });
    let (_workspace, cx) =
        cx.add_window_view(|window, cx| Workspace::new(roostr_app::pages(), window, cx));
    cx.simulate_resize(size(px(1600.), px(800.)));
    cx.update(|_, cx| navigate(Route::Object("page-1".into()), cx));
    settle(cx);
    assert!(
        cx.debug_bounds("w-row-agent-0").is_some(),
        "the nav shows the pinned list"
    );
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |p, cx| p.set_open(PaneTab::Properties, false, cx))
    });
    settle(cx);
    assert_eq!(props_width(cx), 0., "Properties is collapsed");
    // Open it and let the slide run frame by frame, as the display asks for
    // them (no refresh: a refresh redraws everything, as a click does). The
    // frames the window asked for while it set up (tests have no frame loop)
    // are delivered first.
    for _ in 0..3 {
        cx.simulate_frame();
    }
    let before = roostr_shell::nav_layouts();
    cx.update(|_, cx| {
        PaneState::global(cx).update(cx, |p, cx| p.set_open(PaneTab::Properties, true, cx))
    });
    let mut widths = Vec::new();
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.simulate_frame();
        widths.push(props_width(cx));
    }
    let mut steps = widths.clone();
    steps.dedup();
    assert!(
        steps.len() >= 4,
        "the column slid open over several frames: {widths:?}"
    );
    assert!(
        widths.last().is_some_and(|w| *w > 200.),
        "and ended open: {widths:?}"
    );
    // Neither the opening (a pane change: the columns and the page header's
    // button) nor the slide's frames touch the nav.
    let nav = roostr_shell::nav_layouts() - before;
    assert_eq!(
        nav,
        0,
        "the nav was laid out {nav} times over the slide's {} frames",
        steps.len()
    );
}
