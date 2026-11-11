//! A query keeps a relative date rule: "Updated more than 30 days ago" is
//! stored as the engine's `numberOfDaysAgo` quick option with the day count,
//! so it keeps meaning that as days pass. The panel offers it beside the date
//! comparisons; the query sends it to the engine (here the fake vault, which
//! answers by it) and the table shows only the older records.
#![cfg(not(target_family = "wasm"))]

mod support;

use std::sync::Arc;

use parking_lot::Mutex;
use roostr_core::types::{QueryRow, Value};
use roostr_sets::DataViewKind;
use rune::Key;
use serde_json::{Value as Json, json};
use support::{App, Vault, object, value};

const DAY_MS: i64 = 86_400_000;
/// 2020-01-01 and 2099-01-01: far either side of any "30 days ago".
const OLD_MS: i64 = 1_577_836_800_000;
const NEW_MS: i64 = 4_070_908_800_000;

/// Does `row` pass the relative `updatedAt` rules of `filters` (as the engine would)?
fn passes(row: &QueryRow, filters: &[Json]) -> bool {
    let now = chrono::Utc::now().timestamp_millis();
    filters
        .iter()
        .filter(|f| f["key"] == "updatedAt" && f["quickOption"] == "numberOfDaysAgo")
        .all(|f| {
            let cutoff = now - f["value"].as_i64().expect("a numeric day count") * DAY_MS;
            match f["condition"].as_str() {
                Some("less") => row.updated_at < cutoff,
                Some("greaterOrEqual") => row.updated_at >= cutoff,
                _ => true,
            }
        })
}

/// Two tasks, one updated long ago and one far ahead, and a query on tasks
/// with an Updated rule not yet given a value. Every query body sent is kept.
fn vault(bodies: Arc<Mutex<Vec<Json>>>) -> Vault {
    let filter = value(json!({"valuesValue": {"items": [{"mapValue": {"entries": {
        "key": {"stringValue": "updatedAt"}, "condition": {"stringValue": "less"},
        "value": {"valuesValue": {"items": []}}
    }}}]}}));
    let task = |id: &str, name: &str, at: i64| {
        let mut o = object(id, "task", &[("name", Value::string(name))]);
        o.updated_at = at;
        o
    };
    let objects = vec![
        object(
            "type-task",
            "type",
            &[
                ("name", Value::string("Task")),
                ("key", Value::string("task")),
            ],
        ),
        task("t-old", "Stale task", OLD_MS),
        task("t-new", "Fresh task", NEW_MS),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Stale tasks")),
                ("setOf", Value::values(vec![Value::string("task")])),
                ("viewFilters", filter),
            ],
        ),
    ];
    let on_query = Box::new(move |vault: &Vault, body: &Json| -> Vec<QueryRow> {
        let text = body.to_string();
        if text.contains("\"setId\":\"q1\"") {
            bodies.lock().push(body.clone());
            let filters = body["filters"].as_array().cloned().unwrap_or_default();
            return vault
                .of_type("task")
                .into_iter()
                .filter(|r| passes(r, &filters))
                .collect();
        }
        vault
            .objects
            .lock()
            .iter()
            .filter(|o| o.type_key != "query" && text.contains(&format!("\"{}\"", o.type_key)))
            .map(Vault::row)
            .collect()
    });
    Vault::new(objects, Vec::new(), on_query)
}

/// The saved `viewFilters` of `q1`, as JSON.
fn saved(vault: &Vault) -> Json {
    vault
        .field("q1", "viewFilters")
        .map(|v| serde_json::to_value(&v).unwrap())
        .unwrap_or_default()
}

/// Presses the drawn text `label` (a menu row).
fn click_text(app: &mut App, label: &str) {
    let r = app
        .rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no text {label:?} drawn"))
        .rect;
    app.click_at(r[0] + r[2] / 2., r[1] + r[3] / 2.);
}

#[test]
fn updated_more_than_30_days_ago_is_stored_relative() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut app = App::new(
        vault(bodies.clone()),
        "q1",
        DataViewKind::Query,
        (1400., 900.),
    );
    app.until_shown("row-t-new");
    app.click("q1/dv-filter");
    app.until_shown("q1/fcond-0");
    app.click("q1/fcond-0");
    click_text(&mut app, "is more than … days ago");
    let vault = app.vault.clone();
    app.until("the relative condition saved", move |_| {
        saved(&vault).to_string().contains("numberOfDaysAgo")
    });
    app.s().store.reload_object("q1");
    let vault = app.vault.clone();
    app.until("the panel shows it", move |s| {
        s.store
            .object("q1")
            .and_then(|o| o.fields.get("viewFilters").cloned())
            == vault.field("q1", "viewFilters")
    });
    assert!(
        app.rt
            .elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("more than")),
        "the rule reads \"Updated more than … days ago\""
    );
    assert!(
        app.rt
            .elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("days ago"))
    );

    app.click("q1/fval-0");
    app.type_text("30");
    app.key(Key::Enter);
    let vault = app.vault.clone();
    app.until("the day count saved", move |_| {
        saved(&vault).to_string().contains("\"30\"")
    });
    let stored = saved(&app.vault);
    let entries = &stored["valuesValue"]["items"][0]["mapValue"]["entries"];
    assert_eq!(entries["key"]["stringValue"], "updatedAt", "{stored}");
    assert_eq!(entries["condition"]["stringValue"], "less", "{stored}");
    assert_eq!(
        entries["quickOption"]["stringValue"], "numberOfDaysAgo",
        "{stored}"
    );
    assert_eq!(
        entries["value"]["valuesValue"]["items"],
        json!([{"stringValue": "30"}]),
        "{stored}"
    );

    // The view follows its object: the query carries the rule, numeric.
    app.s().store.reload_object("q1");
    let sent = bodies.clone();
    app.until("the relative query sent", move |_| {
        sent.lock().last().is_some_and(|b| {
            b["filters"]
                .as_array()
                .is_some_and(|f| f.iter().any(|f| f["quickOption"] == "numberOfDaysAgo"))
        })
    });
    let body = bodies.lock().last().cloned().unwrap();
    let rule = body["filters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "updatedAt")
        .cloned()
        .unwrap();
    assert_eq!(
        rule,
        json!({"key": "updatedAt", "condition": "less", "quickOption": "numberOfDaysAgo", "value": 30})
    );
    app.until_shown("row-t-old");
    assert!(
        !app.shown("row-t-new"),
        "a record updated since is not shown"
    );
}
