//! Exercise every public view of roostr-properties against the daemon named by the environment.
//!
//! `cargo run -p roostr-properties --example properties -- [object-id] [type-id] [relation-id] [section]`
//! (`-- --snap out.png --size 1400x900 <object-id> - - object` renders headless; `-` keeps a default.)
//! Defaults: seed-project-launch, its space's Task type, seed-rel-priority.
//! `section` (optional) shows one part alone: `object` (title, badges and the Properties pane),
//! `pane` (the pane alone, as the drawer's column), `values`, `pickers`, `type`, `property`.

use std::rc::Rc;

use roostr_core::objects::object_icon;
use roostr_core::relations::space_relations;
use roostr_core::{QueryRow, State, api, field_str};
use roostr_properties::{
    CredentialCell, CredentialTarget, EmojiPicker, FlowEvent, FlowItem, KeyPicker, PropertyFlow,
    PropertyFlowState, PropertyPick, PropertySuggest, PropertySuggestState, PropertyValue,
    SuggestEvent, TypeSuggest, TypeSuggestState, credential_cell_badge, object_meta, prop_icon,
    properties_pane, property_panel, reconnect_target, type_panel,
};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Placement, Theme, glyph, outline_button, section_label, ty};
use rune::{Cx, El, ScrollSpec, col, el, row, text};
use serde_json::json;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pop {
    Emoji,
    Property,
    Type,
    Flow,
}

/// The example's own state (the views' state lives in roostr-properties' slots).
#[derive(Default)]
struct Example {
    object_id: String,
    /// Explicit type / relation ids; empty = the defaults, found once the store loads.
    type_id: String,
    relation_id: String,
    section: String,
    pop: Option<Pop>,
    property_suggest: Option<PropertySuggestState>,
    type_suggest: Option<TypeSuggestState>,
    flow: Option<PropertyFlowState>,
    flow_items: Vec<FlowItem>,
    sort_key: String,
    last: String,
    cells: Vec<QueryRow>,
}

impl Example {
    fn section(&self, name: &str) -> bool {
        self.section.is_empty() || self.section == name
    }
}

fn arg(args: &[String], i: usize) -> Option<String> {
    args.get(i).filter(|a| a.as_str() != "-").cloned()
}

fn setup(s: &mut State) {
    roostr_properties::register(s);
    let args = rune::extra_args();
    let ex = Example {
        object_id: arg(args, 0).unwrap_or_else(|| "seed-project-launch".into()),
        type_id: arg(args, 1).unwrap_or_default(),
        relation_id: arg(args, 2).unwrap_or_else(|| "seed-rel-priority".into()),
        section: arg(args, 3).unwrap_or_default(),
        flow_items: vec![
            FlowItem {
                key: "name".into(),
                name: "Name".into(),
                format: "shorttext".into(),
                on: true,
                emoji: None,
            },
            FlowItem {
                key: "status".into(),
                name: "Status".into(),
                format: "status".into(),
                on: true,
                emoji: None,
            },
            FlowItem {
                key: "dueDate".into(),
                name: "Due date".into(),
                format: "date".into(),
                on: false,
                emoji: Some("📅".into()),
            },
        ],
        sort_key: "name".into(),
        ..Example::default()
    };
    s.store.ensure_object(&ex.object_id);
    s.put(ex);
    // Rows for the credential cells: the test credential and an object whose Error names it.
    let task = api::query_all(
        json!({ "filters": [{ "key": "id", "condition": "in", "value": ["props-test-cred", "props-test-task"] }] }),
        s,
    );
    s.rt.spawn(async move |cx| {
        if let Ok(rows) = task.await {
            cx.update(|s| s.slot::<Example>().cells = rows);
        }
    })
    .detach();
}

fn ex(s: &mut State) -> &mut Example {
    s.slot::<Example>()
}

fn first_space(s: &State) -> String {
    s.store
        .channels
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default()
}

fn open(s: &mut State, pop: Pop) {
    if ex(s).pop == Some(pop) {
        ex(s).pop = None;
        return;
    }
    let space = first_space(s);
    let e = ex(s);
    e.pop = Some(pop);
    match pop {
        Pop::Property => {
            e.property_suggest = Some(PropertySuggestState::new(
                space,
                vec!["status".into()],
                vec![
                    ("type".into(), "Type".into()),
                    ("createdAt".into(), "Created".into()),
                ],
            ))
        }
        Pop::Type => {
            e.type_suggest = Some(TypeSuggestState::new(
                space,
                vec!["page".into()],
                "Filter types…".into(),
            ))
        }
        Pop::Flow => e.flow = Some(PropertyFlowState::new(e.flow_items.clone(), vec![], space)),
        Pop::Emoji => {}
    }
}

fn heading(t: &Theme, label: &str) -> El {
    section_label(t, label).mt(28.0).mb(10.0)
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(roostr_ui::ThemePref::Dark, cx);
    let mut layer = Vec::new();
    let Some(e) = s.get::<Example>() else {
        return el();
    };
    let space = first_space(s);
    let mut main = col().px(32.0).pt(24.0).pb(80.0);
    let object = s.store.object(&e.object_id);
    if let Some(object) = &object {
        let object_space = object.channel().to_string();
        if e.section("object") {
            let icon = object_icon(
                &s.store,
                object.field_str("iconEmoji"),
                &object.type_key,
                &object_space,
            );
            main = main
                .child(
                    row()
                        .items_center()
                        .gap(12.0)
                        .child(el().w(36.0).child(text(icon).font(ty::ui(30.0))))
                        .child(
                            text(object.name().to_string())
                                .font(ty::ui(34.0).weight(750.0).lh(34.0 * 1.25))
                                .color(t.fg),
                        ),
                )
                .child(object_meta(s, cx, t, &mut layer, &e.object_id, false));
        }
        if e.section("values") {
            main = main.child(heading(t, "PropertyValue gallery"));
            let rels: Vec<_> = space_relations(&s.store.relations, &object_space)
                .into_iter()
                .cloned()
                .collect();
            let mut seen = Vec::new();
            let mut wanted: Vec<_> = rels
                .iter()
                .filter(|r| {
                    object.fields.contains_key(&r.key)
                        || r.key == "chat_color"
                        || r.key == "description"
                })
                .filter(|r| {
                    !matches!(
                        r.key.as_str(),
                        "name" | "channel" | "iconEmoji" | "featuredRelations" | "repeat"
                    )
                })
                .cloned()
                .collect();
            wanted.sort_by(|a, b| a.format.cmp(&b.format));
            for rel in wanted {
                let kind = (rel.format.clone(), rel.key == "chat_color");
                if seen.contains(&kind) && rel.format != "object" {
                    continue;
                }
                seen.push(kind);
                let key = rel.key.clone();
                let oid = object.id.clone();
                main = main.child(
                    row()
                        .items_start()
                        .gap(16.0)
                        .py(6.0)
                        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
                        .child(
                            row()
                                .w(200.0)
                                .items_center()
                                .gap(7.0)
                                .child(glyph(prop_icon(&rel.format), 14.0).color(t.muted))
                                .child(
                                    text(format!("{} · {}", rel.name, rel.format))
                                        .font(ty::ui(13.0))
                                        .color(t.muted),
                                ),
                        )
                        .child(
                            col().grow(1.0).max_w(420.0).child(
                                PropertyValue::new(
                                    format!("gallery-{key}"),
                                    rel.clone(),
                                    object.field(&key).cloned(),
                                )
                                .space(object_space.clone())
                                .on_save(move |s: &mut State, v| {
                                    api::note::set_field(&oid, &key, v, s).detach_and_log_err(&s.rt)
                                })
                                .build(s, cx, t, &mut layer),
                            ),
                        ),
                );
            }
            if let Some(rel) = rels.iter().find(|r| r.format == "tag") {
                main = main.child(
                    row()
                        .gap(16.0)
                        .py(6.0)
                        .child(
                            text("read only · tag")
                                .font(ty::ui(13.0))
                                .color(t.muted)
                                .w(200.0),
                        )
                        .child(
                            PropertyValue::new(
                                "gallery-ro",
                                rel.clone(),
                                object.field(&rel.key).cloned(),
                            )
                            .read_only(true)
                            .build(s, cx, t, &mut layer),
                        ),
                );
            }
        }
    }
    if e.section("pickers") {
        main = main
            .child(heading(t, "Pickers"))
            .child(pickers(s, cx, t, &mut layer, e, &space));
        if !e.last.is_empty() {
            main = main.child(
                text(e.last.clone())
                    .font(ty::ui(12.0))
                    .color(t.muted)
                    .mt(8.0),
            );
        }
        main = main
            .child(heading(t, "Credential cells"))
            .child(credential_cells(s, cx, t, &mut layer, e));
    }
    if e.section("type") {
        let type_id = if e.type_id.is_empty() {
            s.store
                .types
                .iter()
                .find(|ty| ty.key == "task" && ty.space == space)
                .map(|ty| ty.id.clone())
                .unwrap_or_default()
        } else {
            e.type_id.clone()
        };
        main = main
            .child(heading(t, "Type panel"))
            .child(type_panel(s, cx, t, &mut layer, &type_id));
    }
    if e.section("property") {
        main = main
            .child(heading(t, "Property panel"))
            .child(property_panel(s, cx, t, &mut layer, &e.relation_id));
    }
    let mut pane = || {
        col()
            .key("pane")
            .w(380.0)
            .h_full()
            .min_h(0.0)
            .border_sides([0.0, 0.0, 0.0, 1.0], t.border)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .padding(12.0)
                    .child(properties_pane(s, cx, t, &mut layer, &e.object_id)),
            )
    };
    let body = if e.section == "pane" {
        row().w_full().h_full().fill(t.bg).child(pane())
    } else {
        let mut body = row().w_full().h_full().fill(t.bg).child(
            col()
                .key("main")
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(main),
        );
        if e.section.is_empty() || e.section == "object" {
            body = body.child(pane());
        }
        body
    };
    roostr_ui::root(body, layer)
}

fn pickers(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    e: &Example,
    space: &str,
) -> El {
    let button = |key: &str, label: &str, pop: Pop| {
        outline_button(key, label)
            .on_press(move |s: &mut State| open(s, pop))
            .build(t)
    };
    let mut bar = row().flex_wrap().gap(10.0).items_start();
    bar = bar.child(button("emoji", "Emoji picker", Pop::Emoji));
    if e.pop == Some(Pop::Emoji) {
        let panel = EmojiPicker::new("emoji-picker")
            .with_image(true)
            .on_pick(|s: &mut State, icon: &str| {
                let shown = if icon.len() > 40 {
                    format!("{}…", &icon[..icon.floor_char_boundary(40)])
                } else {
                    icon.to_string()
                };
                let e = ex(s);
                e.last = format!("EmojiPicker picked {shown}");
                e.pop = None;
            })
            .on_close(|s: &mut State| ex(s).pop = None)
            .build(s, cx, t, layer);
        layer.push(roostr_ui::overlay::anchored(
            panel,
            "emoji",
            Placement::BottomStart,
            8.0,
        ));
    }
    bar = bar.child(button("prop", "Property suggest", Pop::Property));
    if let (Some(Pop::Property), Some(state)) = (e.pop, &e.property_suggest) {
        let panel = PropertySuggest::new("prop-suggest", state)
            .on_state(|s: &mut State| ex(s).property_suggest.as_mut().expect("open"))
            .on_event(|s: &mut State, ev: SuggestEvent<PropertyPick>| {
                let e = ex(s);
                e.last = match ev {
                    SuggestEvent::Picked(PropertyPick::Relation(r)) => {
                        format!("PropertySuggest picked relation {} ({})", r.name, r.format)
                    }
                    SuggestEvent::Picked(PropertyPick::Extra(k)) => {
                        format!("PropertySuggest picked extra {k}")
                    }
                    SuggestEvent::Dismissed => "PropertySuggest dismissed".into(),
                };
                e.pop = None;
                e.property_suggest = None;
            })
            .build(s, cx, t, layer);
        layer.push(roostr_ui::overlay::anchored(
            panel,
            "prop",
            Placement::BottomStart,
            8.0,
        ));
    }
    bar = bar.child(button("type", "Type suggest", Pop::Type));
    if let (Some(Pop::Type), Some(state)) = (e.pop, &e.type_suggest) {
        let panel = TypeSuggest::new("type-suggest", state)
            .on_state(|s: &mut State| ex(s).type_suggest.as_mut().expect("open"))
            .on_event(|s: &mut State, ev: SuggestEvent<String>| {
                let e = ex(s);
                e.last = match ev {
                    SuggestEvent::Picked(k) => format!("TypeSuggest picked {k}"),
                    SuggestEvent::Dismissed => "TypeSuggest dismissed".into(),
                };
                e.pop = None;
                e.type_suggest = None;
            })
            .build(s, cx, t, layer);
        let framed = col()
            .w(280.0)
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(10.0)
            .padding(8.0)
            .child(panel);
        layer.push(roostr_ui::overlay::anchored(
            framed,
            "type",
            Placement::BottomStart,
            8.0,
        ));
    }
    bar = bar.child(button("flow", "Property flow", Pop::Flow));
    if let (Some(Pop::Flow), Some(state)) = (e.pop, &e.flow) {
        let panel = PropertyFlow::new("prop-flow", state)
            .on_state(|s: &mut State| ex(s).flow.as_mut().expect("open"))
            .on_event(|s: &mut State, ev: FlowEvent| {
                let e = ex(s);
                match ev {
                    FlowEvent::Toggle(key, on) => {
                        if let Some(i) = e.flow_items.iter_mut().find(|i| i.key == key) {
                            i.on = on;
                        }
                        let items = e.flow_items.clone();
                        if let Some(flow) = &mut e.flow {
                            flow.set_items(items);
                        }
                        e.last = format!("PropertyFlow toggled {key} → {on}");
                    }
                    FlowEvent::Add(rel) => e.last = format!("PropertyFlow added {}", rel.name),
                    FlowEvent::Close => {
                        e.pop = None;
                        e.flow = None;
                    }
                }
            })
            .build(s, cx, t, layer);
        layer.push(roostr_ui::overlay::anchored(
            panel,
            "flow",
            Placement::BottomStart,
            8.0,
        ));
    }
    let keys: Vec<String> = space_relations(&s.store.relations, space)
        .iter()
        .map(|r| r.key.clone())
        .collect();
    let label: Rc<dyn Fn(&str, &State) -> String> = Rc::new(|key: &str, s: &State| {
        let space = first_space(s);
        match s.store.relation(key, &space) {
            Some(r) => format!(
                "{}{}",
                r.icon_emoji
                    .as_ref()
                    .map(|e| format!("{e} "))
                    .unwrap_or_default(),
                if r.name.is_empty() { &r.key } else { &r.name }
            ),
            None => key.to_string(),
        }
    });
    bar.child(
        KeyPicker::new("key-picker", e.sort_key.clone(), keys, label)
            .on_pick(|s: &mut State, k: &str| {
                let e = ex(s);
                e.sort_key = k.to_string();
                e.last = format!("KeyPicker picked {k}");
            })
            .build(s, cx, t, layer),
    )
}

fn credential_cells(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    e: &Example,
) -> El {
    let mut cells = col().mt(16.0).gap(8.0);
    for r in &e.cells {
        for key in ["status", "error"] {
            let Some(target) = reconnect_target(r, key) else {
                continue;
            };
            let (label, g, color) = credential_cell_badge(r, s);
            let colors = roostr_ui::tags::badge_colors(t, color);
            let content = match (&target, key) {
                (CredentialTarget::Credential(_), "status") => row()
                    .items_center()
                    .gap(5.0)
                    .child(glyph(g, 14.0).color(colors.icon))
                    .child(text(label).font(ty::ui(13.0)).color(colors.fg)),
                _ => text(field_str(&r.fields, "error").to_string())
                    .font(ty::ui(13.0))
                    .color(t.red),
            };
            cells = cells.child(
                row()
                    .gap(12.0)
                    .child(
                        text(format!("{} · {key}", field_str(&r.fields, "name")))
                            .font(ty::ui(13.0))
                            .color(t.muted)
                            .w(200.0),
                    )
                    .child(
                        CredentialCell::new(
                            format!("cell-{}-{key}", r.id),
                            r.clone(),
                            target,
                            "Reconnect",
                        )
                        .child(content)
                        .build(s, cx, t, layer),
                    ),
            );
        }
    }
    cells
}

fn main() {
    roostr_host::run_example("Properties", setup, view);
}
