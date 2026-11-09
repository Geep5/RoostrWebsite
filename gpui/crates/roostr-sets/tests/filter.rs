//! A query filters on an object property by which objects it holds (ported
//! from roostr-app's `filter_by_object`, its data view part). Field report:
//! filtering by Agent offered only "is empty" / "is not empty", so there was
//! no way to list what one agent is on. An object property now has "has any
//! of" (and all of / none of) with a picker of the objects it may hold;
//! picking one saves its id in the filter, which the engine matches.
#![cfg(not(target_family = "wasm"))]

mod support;

use roostr_core::types::{QueryRow, RelationDef, Value};
use roostr_sets::DataViewKind;
use serde_json::{Value as Json, json};
use support::{App, SPACE, Vault, object, value};

/// Two agents, a task on the designer, and a query filtering on Agent.
fn vault() -> Vault {
    let filter = value(json!({"valuesValue": {"items": [{"mapValue": {"entries": {
        "key": {"stringValue": "agent"}, "condition": {"stringValue": "in"},
        "value": {"valuesValue": {"items": []}}
    }}}]}}));
    let objects = vec![
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
            ],
        ),
        object(
            "agent-1",
            "agent",
            &[("name", Value::string("Mockup Designer"))],
        ),
        object(
            "agent-2",
            "agent",
            &[("name", Value::string("Inbox Triage"))],
        ),
        object(
            "t1",
            "task",
            &[
                ("name", Value::string("Landing mockup")),
                (
                    "agent",
                    Value::values(vec![Value::link("agent-1", "agent")]),
                ),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Uses designer")),
                ("viewFilters", filter),
            ],
        ),
    ];
    let relations = vec![RelationDef {
        id: "rel-agent".into(),
        key: "agent".into(),
        format: "object".into(),
        name: "Agent".into(),
        object_types: Some(vec!["type-agent".into()]),
        space: Some(SPACE.into()),
        ..RelationDef::default()
    }];
    let on_query = Box::new(|vault: &Vault, body: &Json| -> Vec<QueryRow> {
        let text = body.to_string();
        vault
            .objects
            .lock()
            .iter()
            .filter(|o| text.contains(&format!("\"{}\"", o.type_key)) || text.contains("setId"))
            .map(Vault::row)
            .collect()
    });
    Vault::new(objects, relations, on_query)
}

/// The saved `viewFilters` of `q1`, as JSON text.
fn saved(app: &App) -> String {
    app.vault
        .field("q1", "viewFilters")
        .map(|v| serde_json::to_string(&v).unwrap())
        .unwrap_or_default()
}

#[test]
fn a_query_filters_by_the_agent_an_object_has() {
    let mut app = App::new(vault(), "q1", DataViewKind::Query, (1400., 900.));
    app.until_shown("q1/dv-filter");
    app.click("q1/dv-filter");
    // "has any of" offers the property's objects: the agents, not the task.
    app.until_shown("fobj-add-agent-1");
    assert!(app.shown("fobj-add-agent-2"), "Inbox Triage is offered");
    assert!(!app.shown("fobj-add-t1"), "a task is not an agent");

    app.click("fobj-add-agent-1");
    let vault = app.vault.clone();
    app.until("the filter saved", move |_| {
        vault
            .field("q1", "viewFilters")
            .is_some_and(|v| serde_json::to_string(&v).unwrap().contains("agent-1"))
    });
    let filters = saved(&app);
    assert!(
        filters.contains("\"in\"") && filters.contains("agent-1"),
        "the filter holds the agent's id: {filters}"
    );
    // The view follows its object: the pick shows under Selected.
    app.s().store.reload_object("q1");
    app.until_shown("fobj-on-agent-1");
    assert!(!app.shown("fobj-add-agent-1"), "and is no longer offered");
}
