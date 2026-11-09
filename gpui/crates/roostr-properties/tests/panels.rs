//! Type and property object page bodies, headless over the fake vault.
mod support;

use roostr_core::State;
use roostr_core::types::{QueryResult, Value};
use roostr_ui::ThemePref;
use rune::{Cx, El};
use support::{App, Vault, object};

fn type_view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let body = roostr_properties::type_panel(s, cx, t, &mut layer, "ty-1");
    roostr_ui::root(body, layer)
}

fn property_view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let body = roostr_properties::property_panel(s, cx, t, &mut layer, "rel-1");
    roostr_ui::root(body, layer)
}

fn field_text(params: &roostr_core::backend::JsonMap, key: &str) -> Option<String> {
    (params["key"] == key).then(|| {
        params["value"]["stringValue"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    })
}

#[test]
fn type_panel_switches_layout_and_lists_templates() {
    let vault = Vault::new(
        vec![
            object("ty-1", "type", &[("name", Value::string("Book"))]),
            object(
                "tpl-1",
                "template",
                &[
                    ("name", Value::string("Reading notes")),
                    ("target_type", Value::string("ty-1")),
                ],
            ),
        ],
        vec![],
    );
    *vault.on_query.lock() = Some(Box::new(|v: &Vault, body| {
        if body["type"] != "template" {
            return None;
        }
        let records: Vec<_> = v
            .objects
            .lock()
            .iter()
            .filter(|o| o.type_key == "template")
            .map(Vault::row)
            .collect();
        Some(QueryResult {
            total: records.len() as u64,
            records,
        })
    }));
    let mut app = App::new(vault, type_view);
    app.until("the type object", |s| s.store.object("ty-1").is_some());
    assert!(app.shown("type-panel/page"));

    app.click("type-panel/task");
    let vault = app.vault.clone();
    app.until("layout written", |_| {
        vault
            .calls("set_field")
            .iter()
            .any(|p| field_text(p, "layout").as_deref() == Some("task"))
    });

    app.click("tpl-chip");
    for _ in 0..20 {
        if app.shown("tg-tpl-1") {
            break;
        }
        app.step();
    }
    assert!(app.shown("tg-tpl-1"), "template card from the vault query");
    assert!(app.shown("tg-new"));
}

#[test]
fn property_panel_saves_definition_on_blur_and_lists_holders() {
    let vault = Vault::new(
        vec![
            object(
                "rel-1",
                "relation",
                &[
                    ("name", Value::string("Mood")),
                    ("key", Value::string("mood")),
                    ("format", Value::string("shorttext")),
                ],
            ),
            object(
                "o-1",
                "note",
                &[
                    ("name", Value::string("Monday")),
                    ("mood", Value::string("happy")),
                ],
            ),
        ],
        vec![],
    );
    *vault.on_query.lock() = Some(Box::new(|v: &Vault, body| {
        if !body.to_string().contains("notEmpty") {
            return None;
        }
        let records: Vec<_> = v
            .objects
            .lock()
            .iter()
            .filter(|o| o.fields.contains_key("mood"))
            .map(Vault::row)
            .collect();
        Some(QueryResult {
            total: records.len() as u64,
            records,
        })
    }));
    let mut app = App::new(vault, property_view);
    app.until("the relation object", |s| s.store.object("rel-1").is_some());
    for _ in 0..50 {
        if app.shown("name-o-1") {
            break;
        }
        app.step();
    }
    assert!(app.shown("name-o-1"), "holder row listed");

    app.click("pp-definition");
    app.type_text("How the day felt");
    assert!(
        app.vault.calls("set_field").is_empty(),
        "no save while typing"
    );
    // Press outside: the area blurs and saves.
    app.rt.pointer_down(1300.0, 850.0);
    app.rt.pointer_up(1300.0, 850.0);
    let vault = app.vault.clone();
    app.until("definition saved", |_| {
        vault
            .calls("set_field")
            .iter()
            .any(|p| field_text(p, "description").as_deref() == Some("How the day felt"))
    });
}
