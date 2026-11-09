//! PropertyValue headless: text commits on Enter and on blur, tag/status
//! picks, a date picked on the calendar, and the object link picker (auto
//! open, filter, Enter, create-from-query: roostr-app's link_create).

mod support;

use chrono::{Local, TimeZone as _};
use roostr_core::types::RelationOption;
use roostr_core::{RelationDef, State, Value};
use roostr_properties::PropertyValue;
use roostr_ui::ThemePref;
use rune::{Cx, El, Key, col};
use serde_json::json;
use support::{App, SPACE, Vault, object, relation};

/// What the test view draws: editor `v` over `rel`/`value`, plus a second
/// text editor `w` to move focus to.
#[derive(Default)]
struct Config {
    rel: RelationDef,
    value: Option<Value>,
    auto_open: bool,
    saves: Vec<Value>,
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let mut body = col().padding(20.0).gap(10.0);
    if let Some(c) = s.get::<Config>() {
        body = body
            .child(
                PropertyValue::new("v", c.rel.clone(), c.value.clone())
                    .space(SPACE)
                    .auto_open(c.auto_open)
                    .on_save(|s: &mut State, v: Value| {
                        let c = s.slot::<Config>();
                        c.value = Some(v.clone());
                        c.saves.push(v);
                    })
                    .build(s, cx, t, &mut layer),
            )
            .child(
                PropertyValue::new("w", relation("rel-w", "other", "shorttext", "Other"), None)
                    .build(s, cx, t, &mut layer),
            );
    }
    roostr_ui::root(body, layer)
}

fn app(vault: Vault, rel: RelationDef, value: Option<Value>, auto_open: bool) -> App {
    let mut app = App::new(vault, view);
    app.s().put(Config {
        rel,
        value,
        auto_open,
        saves: vec![],
    });
    app.rt.invalidate();
    app.step();
    app.step();
    app
}

fn saves(app: &mut App) -> Vec<Value> {
    app.s().slot::<Config>().saves.clone()
}

fn strings(v: &Value) -> Vec<String> {
    v.items()
        .iter()
        .filter_map(|i| i.string_value.clone())
        .collect()
}

fn links(v: &Value) -> Vec<String> {
    v.items()
        .iter()
        .filter_map(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
        .collect()
}

#[test]
fn text_commits_on_enter_and_on_blur() {
    let rel = relation("rel-t", "note", "shorttext", "Note");
    let mut app = app(Vault::default(), rel, None, false);
    app.click("v/text");
    app.type_text("hello");
    assert!(saves(&mut app).is_empty(), "typing alone saves nothing");
    app.key(Key::Enter);
    assert_eq!(saves(&mut app), [Value::string("hello")]);
    // Enter again with nothing new: no second save.
    app.key(Key::Enter);
    assert_eq!(saves(&mut app).len(), 1);

    app.type_text(" world");
    app.click("w/text");
    app.step();
    assert_eq!(
        saves(&mut app),
        [Value::string("hello"), Value::string("hello world")],
        "focus leaving the box commits it"
    );
}

#[test]
fn commit_pending_commits_typed_text() {
    let rel = relation("rel-n", "count", "number", "Count");
    let mut app = app(Vault::default(), rel, Some(Value::int(3)), false);
    app.click("v/text");
    app.type_text("4");
    roostr_properties::commit_pending(app.s());
    assert_eq!(saves(&mut app), [Value::int(34)]);
}

fn options_rel(format: &str) -> RelationDef {
    let mut rel = relation("rel-o", "stage", format, "Stage");
    rel.options = ["Red", "Blue"]
        .iter()
        .enumerate()
        .map(|(i, t)| RelationOption {
            id: format!("o{i}"),
            text: t.to_string(),
            color: "red".into(),
            order_id: format!("{i:06}"),
        })
        .collect();
    rel
}

#[test]
fn tags_toggle_and_status_picks_one() {
    let mut app = app(Vault::default(), options_rel("tag"), None, false);
    app.click("row-o0");
    app.click("row-o1");
    let s = saves(&mut app);
    assert_eq!(strings(&s[0]), ["Red"]);
    assert_eq!(strings(&s[1]), ["Red", "Blue"]);
    app.click("row-o0");
    assert_eq!(strings(saves(&mut app).last().unwrap()), ["Blue"]);

    let mut app = self::app(Vault::default(), options_rel("status"), None, false);
    app.click("row-o1");
    assert_eq!(strings(&saves(&mut app)[0]), ["Blue"]);
    app.click("row-o0");
    assert_eq!(strings(&saves(&mut app)[1]), ["Red"]);
    // Picking the selected one (or Clear) clears it.
    app.click("clear");
    assert!(strings(&saves(&mut app)[2]).is_empty());
}

#[test]
fn a_date_is_picked_on_the_calendar() {
    let rel = relation("rel-d", "due", "date", "Due");
    let mut app = app(Vault::default(), rel, None, false);
    assert!(!app.shown("date-calendar-surface"));
    app.click("date-btn");
    app.until("the calendar", |_| true);
    assert!(app.shown("date-calendar-surface"));
    let today = Local::now().date_naive();
    app.click(&format!("date-calendar-calendar-{today}"));
    let midnight = Local
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    assert_eq!(saves(&mut app), [Value::int(midnight)]);
    app.step();
    assert!(!app.shown("date-calendar-surface"), "picking closes it");
}

const TASK_TYPE: &str = "type-task";

fn task_vault() -> Vault {
    let vault = Vault::new(
        vec![
            object(
                TASK_TYPE,
                "type",
                &[
                    ("name", Value::string("Task")),
                    ("key", Value::string("task")),
                    ("layout", Value::string("task")),
                ],
            ),
            object("task-1", "task", &[("name", Value::string("Write docs"))]),
        ],
        vec![],
    );
    *vault.on_mutate.lock() = Some(Box::new(|vault, action, params| {
        if action != "create" {
            return None;
        }
        let mut objects = vault.objects.lock();
        let id = format!("task-new-{}", objects.len());
        let mut made = object(&id, params["type_key"].as_str().unwrap_or_default(), &[]);
        made.fields.insert(
            "name".into(),
            Value::string(params["name"].as_str().unwrap_or_default()),
        );
        objects.push(made);
        let mut out = roostr_core::JsonMap::new();
        out.insert("id".into(), json!(id));
        Some(out)
    }));
    vault
}

fn tasks_rel() -> RelationDef {
    let mut rel = relation("rel-tasks", "tasks", "object", "Tasks");
    rel.object_types = Some(vec![TASK_TYPE.into()]);
    rel
}

fn backspace(app: &mut App, n: usize) {
    for _ in 0..n {
        app.key(Key::Backspace);
    }
}

#[test]
fn the_link_picker_filters_picks_and_creates() {
    let mut app = app(task_vault(), tasks_rel(), None, true);
    app.until("the picker opens itself", |_| true);
    assert!(app.shown("obj-menu"), "auto_open opens the link picker");
    assert!(app.shown("cand-task-1"), "the space's task is offered");

    // An existing task's exact name (any case): offered, no create.
    app.type_text("write DOCS");
    assert!(app.shown("cand-task-1"));
    assert!(
        !app.shown("create-from-query"),
        "no create for an existing name"
    );
    backspace(&mut app, "write DOCS".len());
    app.type_text("zzz");
    assert!(!app.shown("cand-task-1"), "the query filters");
    backspace(&mut app, 3);

    // Enter picks the first candidate and closes the picker.
    app.type_text("write");
    app.key(Key::Enter);
    assert_eq!(links(&saves(&mut app)[0]), ["task-1"]);
    assert!(!app.shown("obj-menu"), "a pick closes the picker");

    // A name no task has: the create row; a click makes it and links it.
    app.click("link-object");
    assert!(app.shown("obj-menu"));
    app.type_text("test make");
    assert!(app.shown("create-from-query"), "the create row");
    app.click("create-from-query");
    let vault = app.vault.clone();
    app.until("the create lands", |s| {
        s.get::<Config>().is_some_and(|c| c.saves.len() == 2)
    });
    let creates = vault.calls("create");
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["name"], "test make");
    assert_eq!(creates[0]["type_key"], "task");
    let fields: roostr_core::Fields =
        serde_json::from_value(creates[0]["fields"].clone()).expect("create fields");
    assert_eq!(fields["channel"], Value::string(SPACE), "made in the space");
    assert_eq!(links(&saves(&mut app)[1]), ["task-1", "task-new-2"]);
    assert!(!app.shown("obj-menu"), "the picker closes");

    // Enter on a query no candidate has creates too.
    app.click("link-object");
    app.type_text("another one");
    assert!(app.shown("create-from-query"));
    app.key(Key::Enter);
    app.until("the second create", |s| {
        s.get::<Config>().is_some_and(|c| c.saves.len() == 3)
    });
    assert_eq!(vault.calls("create")[1]["name"], "another one");
    assert_eq!(
        links(&saves(&mut app)[2]),
        ["task-1", "task-new-2", "task-new-3"]
    );
}
