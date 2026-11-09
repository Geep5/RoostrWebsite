//! Headless tests of the suggest menus, the add-property flow and the key picker.

mod support;

use std::rc::Rc;

use roostr_core::State;
use roostr_core::relations::CREATABLE_FORMATS;
use roostr_core::store::TypeDef;
use roostr_properties::{
    FlowEvent, FlowItem, KeyPicker, PropertyFlow, PropertyFlowState, PropertyPick, PropertySuggest,
    PropertySuggestState, SuggestEvent, TypeSuggest, TypeSuggestState,
};
use roostr_ui::ThemePref;
use rune::{Cx, El, Key, col};
use support::{App, SPACE, Vault, relation};

/// The host: menus open while their state is here; what they reported, in order.
#[derive(Default)]
struct Host {
    ps: Option<PropertySuggestState>,
    ts: Option<TypeSuggestState>,
    flow: Option<PropertyFlowState>,
    key: String,
    events: Vec<String>,
}

fn host(s: &mut State) -> &mut Host {
    s.slot::<Host>()
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let h = s.get::<Host>();
    let key = h.map(|h| h.key.clone()).unwrap_or_default();
    let body = col().padding(20.0).child(
        KeyPicker::new(
            "kp",
            key,
            vec!["name".into(), "status".into()],
            Rc::new(|k: &str, _: &State| k.to_uppercase()),
        )
        .on_pick(|s: &mut State, k: &str| host(s).key = k.to_string())
        .build(s, cx, t, &mut layer),
    );
    if let Some(st) = h.and_then(|h| h.ps.as_ref()) {
        let panel = PropertySuggest::new("ps", st)
            .on_state(|s: &mut State| host(s).ps.as_mut().unwrap())
            .on_event(|s: &mut State, ev: SuggestEvent<PropertyPick>| {
                let h = host(s);
                h.events.push(match ev {
                    SuggestEvent::Picked(PropertyPick::Relation(r)) => {
                        format!("rel:{}:{}", r.key, r.id)
                    }
                    SuggestEvent::Picked(PropertyPick::Extra(k)) => format!("extra:{k}"),
                    SuggestEvent::Dismissed => "dismissed".into(),
                });
                h.ps = None;
            })
            .build(s, cx, t, &mut layer);
        layer.push(panel.absolute(40.0, 80.0));
    }
    if let Some(st) = h.and_then(|h| h.ts.as_ref()) {
        let panel = TypeSuggest::new("ts", st)
            .on_state(|s: &mut State| host(s).ts.as_mut().unwrap())
            .on_event(|s: &mut State, ev: SuggestEvent<String>| {
                let h = host(s);
                h.events.push(match ev {
                    SuggestEvent::Picked(k) => format!("type:{k}"),
                    SuggestEvent::Dismissed => "dismissed".into(),
                });
                h.ts = None;
            })
            .build(s, cx, t, &mut layer);
        layer.push(panel.absolute(40.0, 80.0));
    }
    if let Some(st) = h.and_then(|h| h.flow.as_ref()) {
        let panel = PropertyFlow::new("flow", st)
            .on_state(|s: &mut State| host(s).flow.as_mut().unwrap())
            .on_event(|s: &mut State, ev: FlowEvent| {
                let h = host(s);
                match ev {
                    FlowEvent::Toggle(k, on) => h.events.push(format!("toggle:{k}:{on}")),
                    FlowEvent::Add(r) => h.events.push(format!("add:{}", r.key)),
                    FlowEvent::Close => {
                        h.events.push("close".into());
                        h.flow = None;
                    }
                }
            })
            .build(s, cx, t, &mut layer);
        layer.push(panel.absolute(40.0, 80.0));
    }
    roostr_ui::root(body, layer)
}

fn vault() -> Vault {
    Vault::new(
        vec![],
        vec![
            relation("r-status", "status", "status", "Status"),
            relation("r-priority", "priority", "number", "Priority"),
            relation("r-due", "due", "date", "Due"),
        ],
    )
}

/// Changes the host's slot from outside a handler and redraws.
fn set(app: &mut App, f: impl FnOnce(&mut Host)) {
    app.rt.update(|s, _| f(host(s)));
    app.step();
    app.step();
}

fn events(app: &mut App) -> Vec<String> {
    host(app.s()).events.clone()
}

#[test]
fn property_suggest_filters_excludes_and_picks() {
    let mut app = App::new(vault(), view);
    set(&mut app, |h| {
        h.ps = Some(PropertySuggestState::new(
            SPACE.into(),
            vec!["due".into()],
            vec![],
        ))
    });
    // My properties: Status and Priority; Due is excluded.
    assert!(app.shown("ps-command-item-0-0"));
    assert!(app.shown("ps-command-item-0-1"));
    assert!(!app.shown("ps-command-item-0-2"));
    // "prio": Create first, then Priority; Enter picks the first property.
    app.type_text("prio");
    assert!(app.shown("ps-command-item-1-0"));
    assert!(!app.shown("ps-command-item-1-1"));
    app.key(Key::Enter);
    assert_eq!(events(&mut app), ["rel:priority:r-priority"]);
    assert!(host(app.s()).ps.is_none());
}

#[test]
fn property_suggest_creates_an_object_property() {
    let mut app = App::new(vault(), view);
    app.vault.on_mutate.lock().replace(Box::new(|_, action, _| {
        (action == "create").then(|| {
            let mut m = roostr_core::backend::JsonMap::new();
            m.insert("id".into(), "r-owner".into());
            m
        })
    }));
    app.s().store.types.push(TypeDef {
        id: "t-person".into(),
        key: "person".into(),
        name: "Person".into(),
        icon: "🙂".into(),
        layout: "page".into(),
        default_template_id: String::new(),
        space: SPACE.into(),
        builtin: false,
    });
    set(&mut app, |h| {
        h.ps = Some(PropertySuggestState::new(SPACE.into(), vec![], vec![]))
    });
    app.type_text("Owner");
    // No property matches: Enter takes "Create property", then the format list.
    app.key(Key::Enter);
    // Object is the last format: Up from the first wraps to it.
    assert_eq!(CREATABLE_FORMATS.last(), Some(&"object"));
    app.key(Key::Up);
    app.key(Key::Enter);
    // Step 3: the Object's type limits, then Create.
    // Let the list leave and the panel settle at its new height.
    app.rt.frame(1.0);
    app.step();
    app.click("limit-t-person");
    app.click("create-it");
    app.until("the new property", |s| {
        s.get::<Host>().is_some_and(|h| !h.events.is_empty())
    });
    assert_eq!(events(&mut app), ["rel:owner:r-owner"]);
    let create = app.vault.calls("create");
    assert_eq!(create.len(), 1);
    let text = serde_json::to_string(&create[0]).unwrap();
    assert!(text.contains("t-person"), "{text}");
    assert!(text.contains("\"object\""), "{text}");
}

#[test]
fn property_suggest_escape_dismisses_at_once() {
    let mut app = App::new(vault(), view);
    set(&mut app, |h| {
        h.ps = Some(PropertySuggestState::new(SPACE.into(), vec![], vec![]))
    });
    app.type_text("zz");
    app.key(Key::Escape);
    assert_eq!(events(&mut app), ["dismissed"]);
    assert!(host(app.s()).ps.is_none());
}

#[test]
fn type_suggest_picks_a_type_key() {
    let mut app = App::new(vault(), view);
    set(&mut app, |h| {
        h.ts = Some(TypeSuggestState::new(
            SPACE.into(),
            vec![],
            "Filter types…".into(),
        ))
    });
    app.type_text("coll");
    app.key(Key::Enter);
    assert_eq!(events(&mut app), ["type:collection"]);
}

#[test]
fn flow_toggles_and_adds() {
    let mut app = App::new(vault(), view);
    set(&mut app, |h| {
        h.flow = Some(PropertyFlowState::new(
            vec![FlowItem {
                key: "status".into(),
                name: "Status".into(),
                format: "status".into(),
                on: true,
                emoji: None,
            }],
            vec![],
            SPACE.into(),
        ))
    });
    app.click("switch-status");
    assert_eq!(events(&mut app), ["toggle:status:false"]);
    app.click("add-row");
    // Status is a stage-1 row: only Priority and Due are offered.
    assert!(!app.shown("rel-status"));
    app.click("rel-priority");
    assert_eq!(
        events(&mut app),
        ["toggle:status:false", "add:priority", "close"]
    );
    assert!(host(app.s()).flow.is_none());
}

#[test]
fn key_picker_picks() {
    let mut app = App::new(vault(), view);
    set(&mut app, |h| h.key = "name".into());
    app.click("kp");
    app.click("kp-item-1");
    assert_eq!(host(app.s()).key, "status");
}
