//! The table's motion, headless: a held row shrinks (not while a cell of it is
//! edited), its Name icon carries its flight id until the row opens, and the
//! selection count rolls.
#![cfg(not(target_family = "wasm"))]

mod support;

use roostr_core::types::Value;
use roostr_sets::DataViewKind;
use roostr_ui::motion::{IconFlight, icon_id};
use rune::{Mods, PhysicalInput};
use serde_json::json;
use support::{App, Vault, object, relation, value};

/// A query `q1` of three emails with a From column.
fn emails() -> App {
    let mut objects = vec![
        object(
            "type-email",
            "type",
            &[
                ("name", Value::string("Email")),
                ("key", Value::string("email")),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Emails")),
                ("setOf", Value::values(vec![Value::string("email")])),
                (
                    "viewRelations",
                    value(json!({"valuesValue": {"items": [{"mapValue": {"entries": {
                        "key": {"stringValue": "email_from"}, "width": {"intValue": 150}}}}]}})),
                ),
            ],
        ),
    ];
    objects.extend((0..3).map(|i| {
        object(
            &format!("e{i}"),
            "email",
            &[
                ("name", Value::string(format!("Email {i}"))),
                ("email_from", Value::string(format!("p{i}@example.com"))),
            ],
        )
    }));
    let vault = Vault::new(
        objects,
        vec![relation("rel-from", "email_from", "shorttext", "From")],
        Box::new(|vault, body| {
            if body.to_string().contains("\"setId\":\"q1\"") {
                vault.of_type("email")
            } else {
                Vec::new()
            }
        }),
    );
    let mut app = App::new(vault, "q1", DataViewKind::Query, (900., 600.));
    app.until_shown("cell-e0-email_from");
    app
}

fn scale(app: &App, key: &str) -> f32 {
    app.rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && !e.exiting)
        .unwrap_or_else(|| panic!("no element keyed {key}"))
        .scale
}

fn centre(app: &App, key: &str) -> (f32, f32) {
    let r = app.rect(key).unwrap_or_else(|| panic!("no {key}"));
    (r[0] + r[2] / 2., r[1] + r[3] / 2.)
}

/// Holds a press at `key`'s centre for a few frames; the row's scale then.
fn held_scale(app: &mut App, key: &str, row: &str) -> f32 {
    let (x, y) = centre(app, key);
    app.rt.pointer_move(x, y);
    app.rt.pointer_down(x, y);
    for _ in 0..30 {
        app.rt.frame(1.0 / 60.0);
    }
    scale(app, row)
}

#[test]
fn a_held_row_shrinks_but_not_while_its_cell_is_edited() {
    let mut app = emails();
    // The Name (which opens the row) and a cell each shrink while held.
    assert_eq!(scale(&app, "name-e1"), 1.0);
    let held = held_scale(&mut app, "name-e1", "name-e1");
    assert!(held < 0.995, "a held Name shrinks: {held}");
    let (x, y) = centre(&app, "name-e1");
    app.rt.pointer_up(x, y);
    app.step();
    assert_eq!(scale(&app, "name-e1"), 1.0, "back at rest");
    let held = held_scale(&mut app, "cell-e2-email_from", "cell-e2-email_from");
    assert!(held < 0.995, "a held cell shrinks: {held}");
    // Released in place, it opens the cell's editor.
    let (x, y) = centre(&app, "cell-e2-email_from");
    app.rt.pointer_up(x, y);
    app.until_shown("pop-e2-email_from");
    assert_eq!(scale(&app, "cell-e2-email_from"), 1.0);

    // Its cell's editor open, a press on that row leaves it at rest.
    let held = held_scale(&mut app, "name-e2", "name-e2");
    assert_eq!(held, 1.0, "the edited row stays put while held");
    assert_eq!(scale(&app, "row-e2"), 1.0);
    let (x, y) = centre(&app, "name-e2");
    app.rt.pointer_up(x, y);
    app.step();
}

#[test]
fn the_name_icon_carries_its_flight_id_until_its_row_opens() {
    let mut app = emails();
    let id = icon_id("table:q1:e0");
    assert!(app.shown(&id), "the Name icon is keyed by its flight id");
    app.click("name-e0");
    let flight = app.s().get::<IconFlight>().cloned().unwrap_or_default();
    assert_eq!(flight.launched("e0"), Some(id.as_str()), "launched from it");
    assert!(!app.shown(&id), "the row gives up the id to the header");
    assert!(app.shown(&format!("{id}#launched")), "and keeps its icon");
    assert!(app.shown(&icon_id("table:q1:e1")), "other rows keep theirs");
}

#[test]
fn the_selection_count_rolls() {
    let mut app = emails();
    app.rt.physical_input(PhysicalInput::Modifiers(Mods {
        cmd: true,
        ..Mods::default()
    }));
    app.click("name-e0");
    let label = |app: &App| {
        app.rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("q1/sel-count"))
            .and_then(|e| e.label)
    };
    assert_eq!(label(&app).as_deref(), Some("1 selected"));
    let (x, y) = centre(&app, "name-e1");
    app.rt.pointer_move(x, y);
    app.rt.pointer_down(x, y);
    app.rt.pointer_up(x, y);
    app.rt.update(|s, _| {
        s.poll();
    });
    app.rt.frame(1.0 / 60.0);
    app.rt.frame(1.0 / 60.0);
    assert_eq!(label(&app).as_deref(), Some("2 selected"));
    let rolling = app.rt.elements().into_iter().any(|e| {
        e.key
            .as_deref()
            .is_some_and(|k| k.starts_with("q1/sel-count."))
            && (e.exiting || e.moving || e.opacity < 1.0)
    });
    assert!(rolling, "the changed digit rolls");
}
