//! The Properties pane headless (ported from roostr-app's project_folder_row, filter_by_object
//! and link_create pane parts).

mod support;

use roostr_core::types::RelationDef;
use roostr_core::{State, Value};
use roostr_ui::ThemePref;
use rune::{Cx, El, Key, col, el, row};
use serde_json::json;
use support::{App, SPACE, Vault, object, relation};

/// The object the test pane shows.
struct Showing(String);

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let id = s.get::<Showing>().map(|x| x.0.clone()).unwrap_or_default();
    // A Properties column's width.
    let body = col()
        .w(320.0)
        .padding(12.0)
        .child(roostr_properties::properties_pane(
            s, cx, t, &mut layer, &id,
        ));
    roostr_ui::root(body, layer)
}

fn show(app: &mut App, id: &str) {
    app.s().put(Showing(id.into()));
    roostr_properties::open_pane(app.s(), id);
    let id = id.to_string();
    app.until("the object loads", move |s| s.store.object(&id).is_some());
    app.step();
}

fn agent_relation() -> RelationDef {
    RelationDef {
        object_types: Some(vec!["type-agent".into()]),
        ..relation("rel-agent", "agent", "object", "Agent")
    }
}

fn types() -> Vec<roostr_core::Object> {
    vec![
        object(
            "type-agent",
            "type",
            &[
                ("name", Value::string("Agent")),
                ("key", Value::string("agent")),
            ],
        ),
        object(
            "type-task",
            "type",
            &[
                ("name", Value::string("Task")),
                ("key", Value::string("task")),
                ("layout", Value::string("task")),
            ],
        ),
        object(
            "agent-1",
            "agent",
            &[("name", Value::string("Mockup Designer"))],
        ),
    ]
}

#[test]
fn an_object_with_an_agent_shows_its_project_folder() {
    let mut objects = types();
    objects.push(object(
        "t1",
        "task",
        &[
            ("name", Value::string("Landing mockup")),
            (
                "agent",
                Value::values(vec![Value::link("agent-1", "agent")]),
            ),
        ],
    ));
    objects.push(object(
        "t2",
        "task",
        &[("name", Value::string("Grocery list"))],
    ));
    let folder = RelationDef {
        hidden: true,
        ..relation("rel-repo-path", "repo_path", "shorttext", "Project folder")
    };
    let mut app = App::new(Vault::new(objects, vec![agent_relation(), folder]), view);
    show(&mut app, "t1");
    assert!(
        app.shown("prop-row-repo_path"),
        "a task with an agent shows its Project folder"
    );
    show(&mut app, "t2");
    assert!(
        !app.shown("prop-row-repo_path"),
        "a task with no agent doesn't"
    );
}

#[test]
fn repeat_and_its_instructions_on_a_query() {
    let mut objects = types();
    objects.push(object(
        "q1",
        "query",
        &[("name", Value::string("Uses designer"))],
    ));
    let instructions = relation(
        "rel-repeat-instructions",
        "repeat_instructions",
        "longtext",
        "Repeat instructions",
    );
    let mut app = App::new(
        Vault::new(objects, vec![agent_relation(), instructions]),
        view,
    );
    show(&mut app, "q1");

    // A query repeats like a task: its Properties offer Repeat.
    assert!(app.shown("prop-row-__repeat__"), "Repeat on a query");
    assert!(
        !app.shown("prop-row-repeat_instructions"),
        "not before it repeats"
    );
    // Picking a Repeat (its editor open) shows them already.
    app.click("prop-row-__repeat__");
    app.step();
    assert!(
        app.shown("prop-row-repeat_instructions"),
        "shown while Repeat is being set"
    );

    // Once it repeats: right under Repeat.
    let rule: Value = serde_json::from_value(json!({"mapValue": {"entries": {
        "freq": {"stringValue": "day"}, "interval": {"intValue": 1}, "anchor": {"intValue": 0},
        "next": {"intValue": 4_102_444_800_000_i64},
        "times": {"valuesValue": {"items": [{"intValue": 540}]}}
    }}}))
    .unwrap();
    if let Some(q) = app.vault.objects.lock().iter_mut().find(|o| o.id == "q1") {
        q.fields.insert("repeat".into(), rule);
        q.updated_at += 1;
    }
    app.s().store.reload_object("q1");
    app.until("the rule lands", |s| {
        s.store
            .object("q1")
            .is_some_and(|o| o.fields.contains_key("repeat"))
    });
    // Close the Repeat editor if it is still open.
    if app.shown("repeat-save") {
        app.key(Key::Escape);
    }
    let (repeat, _) = app.rt.presented("prop-row-__repeat__").expect("Repeat");
    let (below, _) = app
        .rt
        .presented("prop-row-repeat_instructions")
        .expect("Repeat instructions once it repeats");
    let gap = below[1] - (repeat[1] + repeat[3]);
    assert!((0.0..40.0).contains(&gap), "{below:?} under {repeat:?}");

    // Typed in their box, Done saves them and closes the box.
    app.click("prop-row-repeat_instructions");
    if !app.shown("prop-pop") {
        // That press went to the still-open Repeat editor's outside scrim.
        app.click("prop-row-repeat_instructions");
    }
    let (pop, _) = app.rt.presented("prop-pop").expect("the value box");
    let (x, y) = (pop[0] + pop[2] * 0.5, pop[1] + pop[3] - 20.0);
    app.rt.pointer_down(x, y);
    app.rt.pointer_up(x, y);
    app.step();
    app.type_text("Tell Grant the most important task");
    app.click("pop-done");
    app.until("the instructions save", |_| true);
    let saved = app.vault.field("q1", "repeat_instructions");
    assert_eq!(
        saved.and_then(|v| v.string_value).as_deref(),
        Some("Tell Grant the most important task")
    );
    assert!(!app.shown("pop-done"), "and closes the box");
}

#[test]
fn a_type_has_agent_and_repeat_and_the_agent_row_edits() {
    let mut objects = types();
    objects.push(object(
        "q1",
        "query",
        &[
            ("name", Value::string("Uses designer")),
            (
                "agent",
                Value::values(vec![Value::link("agent-1", "agent")]),
            ),
        ],
    ));
    let mut app = App::new(Vault::new(objects, vec![agent_relation()]), view);
    show(&mut app, "type-task");
    assert!(app.shown("prop-row-agent"), "Agent on a type");
    assert!(app.shown("prop-row-__repeat__"), "Repeat on a type");

    // The row around the agent chip opens the editor; Escape closes it.
    show(&mut app, "q1");
    let (row, _) = app.rt.presented("prop-row-agent").expect("Agent row");
    let (x, y) = (row[0] + row[2] - 8.0, row[1] + row[3] * 0.5);
    app.rt.pointer_down(x, y);
    app.rt.pointer_up(x, y);
    app.step();
    assert!(app.shown("prop-pop"), "the row opens the editor");
    app.key(Key::Escape);
    if app.shown("prop-pop") {
        // The first Escape closed a picker inside the box.
        app.key(Key::Escape);
    }
    assert!(!app.shown("prop-pop"), "Escape closes it");

    // The chip opens its agent.
    app.click("agentchip-agent-1");
    assert_eq!(
        *app.s().nav.route(),
        roostr_core::Route::Object("agent-1".into())
    );
}

#[test]
fn an_empty_object_relation_opens_its_link_picker() {
    let mut objects = types();
    objects.push(object(
        "task-1",
        "task",
        &[("name", Value::string("Write docs"))],
    ));
    objects.push(object(
        "human-1",
        "human",
        &[
            ("name", Value::string("Ada")),
            ("tasks", Value::values(vec![])),
        ],
    ));
    let tasks = RelationDef {
        object_types: Some(vec!["type-task".into()]),
        space: Some(SPACE.into()),
        ..relation("rel-tasks", "tasks", "object", "Tasks")
    };
    let mut app = App::new(Vault::new(objects, vec![tasks]), view);
    show(&mut app, "human-1");
    app.click("prop-row-tasks");
    app.until("the link picker", |_| true);
    assert!(app.shown("obj-menu"), "the link picker");
    app.until("its candidates", |_| true);
    assert!(app.shown("cand-task-1"), "the space's task is offered");
}

/// Adding an agent from the Agent row: the pick links it and closes the link picker (GPUI's
/// `pick_candidate`), the box stays on the new chip, and a press outside closes the box (GPUI's
/// `pane-backdrop`; the box is modal, so a scrim under it never saw the press).
#[test]
fn adding_an_agent_closes_the_picker_and_a_press_outside_the_box() {
    let mut objects = types();
    objects.push(object(
        "t1",
        "task",
        &[("name", Value::string("Metalstorm Monthly Update"))],
    ));
    let mut app = App::new(Vault::new(objects, vec![agent_relation()]), view);
    show(&mut app, "t1");
    app.click("prop-row-agent");
    app.until("the link picker", |_| true);
    assert!(app.shown("obj-menu"), "an empty Agent opens its picker");
    app.until("its candidates", |_| true);
    app.click("cand-agent-1");
    app.until("the agent lands", |s| {
        s.store
            .object("t1")
            .is_some_and(|o| o.field("agent").is_some_and(|v| !v.items().is_empty()))
    });
    assert!(!app.shown("obj-menu"), "the pick closes the picker");
    assert!(app.shown("prop-pop"), "the box stays on the new chip");
    // A press away from the pane.
    app.rt.pointer_down(1000.0, 800.0);
    app.rt.pointer_up(1000.0, 800.0);
    app.step();
    assert!(!app.shown("prop-pop"), "a press outside closes the box");
}

/// The pane as the desktop shows it: a Properties column at the right of a 1440×850 window.
fn right_view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let id = s.get::<Showing>().map(|x| x.0.clone()).unwrap_or_default();
    let body = row().w_full().h_full().child(el().grow(1.0)).child(
        col()
            .w(320.0)
            .h_full()
            .px(14.0)
            .py(8.0)
            .child(roostr_properties::properties_pane(
                s, cx, t, &mut layer, &id,
            )),
    );
    roostr_ui::root(body, layer)
}

const WINDOW: (f32, f32) = (1440.0, 850.0);

/// A query with agent-1 on its Agent, in [`right_view`], its Repeat editor open.
fn repeat_editor() -> App {
    let mut objects = types();
    objects.push(object(
        "q1",
        "query",
        &[
            ("name", Value::string("Metalstorm Monthly Update")),
            (
                "agent",
                Value::values(vec![Value::link("agent-1", "agent")]),
            ),
        ],
    ));
    let mut app = App::sized(
        Vault::new(objects, vec![agent_relation()]),
        right_view,
        WINDOW,
    );
    show(&mut app, "q1");
    app.click("prop-row-__repeat__");
    assert!(app.shown("repeat-pop"), "the Repeat editor");
    app
}

/// Draws until the entrances settle.
fn settle(app: &mut App) {
    for _ in 0..40 {
        app.step();
    }
}

fn inside_window(r: [f32; 4]) -> bool {
    r[0] >= 0.0 && r[1] >= 0.0 && r[0] + r[2] <= WINDOW.0 && r[1] + r[3] <= WINDOW.1
}

/// The Repeat editor stays inside the window with the pane at its right edge.
#[test]
fn the_repeat_editor_stays_inside_the_window() {
    let mut app = repeat_editor();
    settle(&mut app);
    let (pop, _) = app.rt.presented("repeat-pop").expect("the editor");
    assert!(inside_window(pop), "{pop:?} in {WINDOW:?}");
}

/// The Every and Agent selects inside the Repeat editor open their lists under their
/// triggers, not at the window's corner (the lists went into the layer before the editor
/// they anchor into).
#[test]
fn the_repeat_selects_open_under_their_triggers() {
    let mut app = repeat_editor();
    for key in ["freq-select", "agent-select"] {
        app.click(key);
        settle(&mut app);
        let menu = format!("{key}-menu");
        let (list, _) = app.rt.presented(&menu).expect("the open list");
        let (trigger, _) = app.rt.presented(key).expect("the trigger");
        let bottom = trigger[1] + trigger[3];
        assert!(
            (list[1] - bottom - 6.0).abs() < 1.0,
            "{key}: list {list:?} 6pt under trigger {trigger:?}"
        );
        // Start-aligned with the trigger, or end-aligned when that runs off the window.
        let (start, end) = (trigger[0], trigger[0] + trigger[2]);
        assert!(
            (list[0] - start).abs() < 1.0 || (list[0] + list[2] - end).abs() < 1.0,
            "{key}: list {list:?} along trigger {trigger:?}"
        );
        assert!(inside_window(list), "{key}: {list:?} in {WINDOW:?}");
        app.key(Key::Escape);
        assert!(!app.shown(&menu), "{key}: Escape closes the list");
        assert!(app.shown("repeat-pop"), "{key}: and leaves the editor");
    }
}
