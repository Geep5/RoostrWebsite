//! Exercise every public view of roostr-properties against the test daemon.
//!
//! `cargo run -p roostr-properties --example properties -- [object-id] [type-id] [relation-id] [section]`
//! Defaults: seed-project-launch, its space's Task type, seed-rel-priority.
//! `section` (optional) shows one part alone: `object`, `values`, `pickers`, `type`, `property`.

use std::rc::Rc;

/// How KeyPicker reads a key.
type KeyLabel = Rc<dyn Fn(&str, &App) -> SharedString>;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::objects::object_icon;
use roostr_core::relations::space_relations;
use roostr_core::{QueryRow, Store, api, theme};
use roostr_properties::{
    CredentialCell, CredentialTarget, EmojiPicker, FlowEvent, FlowItem, KeyPicker, ObjectMeta,
    PropertiesPane, PropertyFlow, PropertyPanel, PropertyPick, PropertySuggest, PropertyValue,
    SuggestEvent, TypePanel, TypeSuggest, credential_cell_badge, prop_icon, reconnect_target,
};
use serde_json::json;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pop {
    Emoji,
    Property,
    Type,
    Flow,
}

struct Example {
    object_id: String,
    section: String,
    meta: Entity<ObjectMeta>,
    pane: Entity<PropertiesPane>,
    type_panel: Entity<TypePanel>,
    property_panel: Entity<PropertyPanel>,
    pop: Option<Pop>,
    property_suggest: Option<Entity<PropertySuggest>>,
    type_suggest: Option<Entity<TypeSuggest>>,
    flow: Option<Entity<PropertyFlow>>,
    flow_items: Vec<FlowItem>,
    sort_key: String,
    last: String,
    cells: Vec<QueryRow>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Example {
    fn new(args: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let object_id = args
            .first()
            .cloned()
            .unwrap_or_else(|| "seed-project-launch".into());
        let store = Store::global(cx).read(cx);
        let space = store
            .channels
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let type_id = args.get(1).cloned().unwrap_or_else(|| {
            store
                .types
                .iter()
                .find(|t| t.key == "task" && t.space == space)
                .map(|t| t.id.clone())
                .unwrap_or_default()
        });
        let relation_id = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| "seed-rel-priority".into());
        let section = args.get(3).cloned().unwrap_or_default();
        for id in [&type_id, &relation_id] {
            Store::global(cx).update(cx, |s, cx| s.ensure_object(id, cx));
        }
        let meta = cx.new(|cx| ObjectMeta::new(object_id.clone(), window, cx));
        let pane = cx.new(|cx| PropertiesPane::new(object_id.clone(), window, cx));
        let type_panel = cx.new(|cx| TypePanel::new(type_id, window, cx));
        let property_panel = cx.new(|cx| PropertyPanel::new(relation_id, window, cx));
        Store::global(cx).update(cx, |s, cx| s.ensure_object(&object_id, cx));
        // Rows for the credential cells: the test credential and an object whose Error names it.
        let task = api::query_all(
            json!({ "filters": [{ "key": "id", "condition": "in", "value": ["props-test-cred", "props-test-task"] }] }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.cells = rows;
                    cx.notify();
                });
            }
        })
        .detach();
        Self {
            object_id,
            section,
            meta,
            pane,
            type_panel,
            property_panel,
            pop: None,
            property_suggest: None,
            type_suggest: None,
            flow: None,
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
            last: String::new(),
            cells: vec![],
            scroll: ScrollHandle::new(),
            _subs: vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())],
        }
    }

    fn open(&mut self, pop: Pop, window: &mut Window, cx: &mut Context<Self>) {
        if self.pop == Some(pop) {
            self.pop = None;
            cx.notify();
            return;
        }
        self.pop = Some(pop);
        let space = Store::global(cx)
            .read(cx)
            .channels
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        match pop {
            Pop::Property => {
                let s = cx.new(|cx| {
                    PropertySuggest::new(
                        space,
                        vec!["status".into()],
                        vec![
                            ("type".into(), "Type".into()),
                            ("createdAt".into(), "Created".into()),
                        ],
                        window,
                        cx,
                    )
                });
                let sub = cx.subscribe(&s, |this, _, ev: &SuggestEvent<PropertyPick>, cx| {
                    this.last = match ev {
                        SuggestEvent::Picked(PropertyPick::Relation(r)) => {
                            format!("PropertySuggest picked relation {} ({})", r.name, r.format)
                        }
                        SuggestEvent::Picked(PropertyPick::Extra(k)) => {
                            format!("PropertySuggest picked extra {k}")
                        }
                        SuggestEvent::Dismissed => "PropertySuggest dismissed".into(),
                    };
                    this.pop = None;
                    cx.notify();
                });
                self._subs.push(sub);
                self.property_suggest = Some(s);
            }
            Pop::Type => {
                let s = cx.new(|cx| {
                    TypeSuggest::new(
                        space,
                        vec!["page".into()],
                        "Filter types…".into(),
                        window,
                        cx,
                    )
                });
                let sub = cx.subscribe(&s, |this, _, ev: &SuggestEvent<String>, cx| {
                    this.last = match ev {
                        SuggestEvent::Picked(k) => format!("TypeSuggest picked {k}"),
                        SuggestEvent::Dismissed => "TypeSuggest dismissed".into(),
                    };
                    this.pop = None;
                    cx.notify();
                });
                self._subs.push(sub);
                self.type_suggest = Some(s);
            }
            Pop::Flow => {
                let items = self.flow_items.clone();
                let f = cx.new(|cx| PropertyFlow::new(items, vec![], space, window, cx));
                let sub = cx.subscribe(&f, |this, flow, ev: &FlowEvent, cx| {
                    match ev {
                        FlowEvent::Toggle(key, on) => {
                            if let Some(i) = this.flow_items.iter_mut().find(|i| &i.key == key) {
                                i.on = *on;
                            }
                            let items = this.flow_items.clone();
                            flow.update(cx, |f, cx| f.set_items(items, cx));
                            this.last = format!("PropertyFlow toggled {key} → {on}");
                        }
                        FlowEvent::Add(rel) => {
                            this.last = format!("PropertyFlow added {}", rel.name)
                        }
                        FlowEvent::Close => this.pop = None,
                    }
                    cx.notify();
                });
                self._subs.push(sub);
                self.flow = Some(f);
            }
            Pop::Emoji => {}
        }
        cx.notify();
    }

    fn section(&self, name: &str) -> bool {
        self.section.is_empty() || self.section == name
    }
}

fn heading(text: &str) -> Div {
    roostr_core::ui::section_label(text).mt(px(28.)).mb(px(10.))
}

fn button(id: &'static str, label: &'static str) -> gpui_kit::component::button::Button {
    roostr_core::ui::outline_button(id, label)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let object = store.object(&self.object_id);
        let mut main = div().flex().flex_col().px(px(32.)).pt(px(24.)).pb(px(80.));
        if let Some(object) = &object {
            let space = object.channel().to_string();
            if self.section("object") {
                main = main
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().w(px(36.)).text_size(px(30.)).child(object_icon(
                                store,
                                object.field_str("iconEmoji"),
                                &object.type_key,
                                &space,
                            )))
                            .child(
                                div()
                                    .text_size(px(34.))
                                    .font_weight(FontWeight(750.))
                                    .line_height(relative(1.25))
                                    .child(object.name().to_string()),
                            ),
                    )
                    .child(self.meta.clone());
            }
            if self.section("values") {
                main = main.child(heading("PropertyValue gallery"));
                let rels: Vec<_> = space_relations(&store.relations, &space)
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
                    if seen.contains(&(rel.format.clone(), rel.key == "chat_color"))
                        && rel.format != "object"
                    {
                        continue;
                    }
                    seen.push((rel.format.clone(), rel.key == "chat_color"));
                    let key = rel.key.clone();
                    let oid = object.id.clone();
                    main = main.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(16.))
                            .py(px(6.))
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .w(px(200.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .text_size(px(13.))
                                    .text_color(theme::muted())
                                    .child(
                                        gpui_kit::component::Icon::new(prop_icon(&rel.format))
                                            .size(px(14.)),
                                    )
                                    .child(format!("{} · {}", rel.name, rel.format)),
                            )
                            .child(
                                div().flex_1().max_w(px(420.)).child(
                                    PropertyValue::new(
                                        SharedString::from(format!("gallery-{key}")),
                                        rel.clone(),
                                        object.field(&key).cloned(),
                                    )
                                    .space(space.clone())
                                    .on_save(
                                        move |v, _, cx| {
                                            api::note::set_field(&oid, &key, v.clone(), cx)
                                                .detach_and_log_err(cx)
                                        },
                                    ),
                                ),
                            ),
                    );
                }
                if let Some(rel) = rels.iter().find(|r| r.format == "tag") {
                    main = main.child(
                        div()
                            .flex()
                            .gap(px(16.))
                            .py(px(6.))
                            .child(
                                div()
                                    .w(px(200.))
                                    .text_size(px(13.))
                                    .text_color(theme::muted())
                                    .child("read only · tag"),
                            )
                            .child(
                                PropertyValue::new(
                                    "gallery-ro",
                                    rel.clone(),
                                    object.field(&rel.key).cloned(),
                                )
                                .read_only(true),
                            ),
                    );
                }
            }
        }
        if self.section("pickers") {
            let entity = cx.entity().downgrade();
            let mut row = div().flex().flex_wrap().gap(px(10.)).items_start();
            let mut emoji = div().relative().child(
                button("emoji", "Emoji picker")
                    .on_click(cx.listener(|this, _, w, cx| this.open(Pop::Emoji, w, cx))),
            );
            if self.pop == Some(Pop::Emoji) {
                let e1 = entity.clone();
                let e2 = entity.clone();
                emoji = emoji.child(
                    div().absolute().top(px(40.)).left_0().child(deferred(
                        anchored().child(
                            EmojiPicker::new("emoji-picker")
                                .with_image(true)
                                .on_pick(move |s, _, cx| {
                                    let _ = e1.update(cx, |this, cx| {
                                        this.last = format!(
                                            "EmojiPicker picked {}",
                                            if s.len() > 40 {
                                                format!("{}…", &s[..40])
                                            } else {
                                                s.to_string()
                                            }
                                        );
                                        this.pop = None;
                                        cx.notify();
                                    });
                                })
                                .on_close(move |_, cx| {
                                    let _ = e2.update(cx, |this, cx| {
                                        this.pop = None;
                                        cx.notify();
                                    });
                                }),
                        ),
                    )),
                );
            }
            row = row.child(emoji);
            let mut prop = div().relative().child(
                button("prop", "Property suggest")
                    .on_click(cx.listener(|this, _, w, cx| this.open(Pop::Property, w, cx))),
            );
            if let (Some(Pop::Property), Some(s)) = (self.pop, &self.property_suggest) {
                prop = prop.child(
                    div()
                        .absolute()
                        .top(px(40.))
                        .left_0()
                        .child(deferred(anchored().child(s.clone()))),
                );
            }
            row = row.child(prop);
            let mut ty = div().relative().child(
                button("type", "Type suggest")
                    .on_click(cx.listener(|this, _, w, cx| this.open(Pop::Type, w, cx))),
            );
            if let (Some(Pop::Type), Some(s)) = (self.pop, &self.type_suggest) {
                ty = ty.child(
                    div().absolute().top(px(40.)).left_0().child(deferred(
                        anchored().child(
                            div()
                                .w(px(280.))
                                .bg(theme::panel())
                                .border_1()
                                .border_color(theme::border())
                                .rounded(px(10.))
                                .p(px(8.))
                                .child(s.clone()),
                        ),
                    )),
                );
            }
            row = row.child(ty);
            let mut flow = div().relative().child(
                button("flow", "Property flow")
                    .on_click(cx.listener(|this, _, w, cx| this.open(Pop::Flow, w, cx))),
            );
            if let (Some(Pop::Flow), Some(f)) = (self.pop, &self.flow) {
                flow = flow.child(
                    div()
                        .absolute()
                        .top(px(40.))
                        .left_0()
                        .child(deferred(anchored().child(f.clone()))),
                );
            }
            row = row.child(flow);
            let keys: Vec<String> = space_relations(
                &store.relations,
                store.channels.first().map_or("", |s| s.id.as_str()),
            )
            .iter()
            .map(|r| r.key.clone())
            .collect();
            let label: KeyLabel = Rc::new(|key: &str, cx: &App| {
                let store = Store::global(cx).read(cx);
                let space = store
                    .channels
                    .first()
                    .map(|s| s.id.clone())
                    .unwrap_or_default();
                match store.relation(key, &space) {
                    Some(r) => format!(
                        "{}{}",
                        r.icon_emoji
                            .as_ref()
                            .map(|e| format!("{e} "))
                            .unwrap_or_default(),
                        if r.name.is_empty() { &r.key } else { &r.name }
                    )
                    .into(),
                    None => key.to_string().into(),
                }
            });
            let e3 = entity.clone();
            row = row.child(
                KeyPicker::new("key-picker", self.sort_key.clone(), keys, label).on_pick(
                    move |k, _, cx| {
                        let _ = e3.update(cx, |this, cx| {
                            this.sort_key = k.to_string();
                            this.last = format!("KeyPicker picked {k}");
                            cx.notify();
                        });
                    },
                ),
            );
            main = main.child(heading("Pickers")).child(row);
            if !self.last.is_empty() {
                main = main.child(
                    div()
                        .mt(px(8.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(self.last.clone()),
                );
            }
            // Credential cells (set views).
            let mut cells = div().mt(px(16.)).flex().flex_col().gap(px(8.));
            for row in &self.cells {
                for key in ["status", "error"] {
                    let Some(target) = reconnect_target(row, key) else {
                        continue;
                    };
                    let (text, glyph, color) = credential_cell_badge(row, cx);
                    let colors = roostr_core::options::badge_colors(color);
                    let content: AnyElement = match (&target, key) {
                        (CredentialTarget::Credential(_), "status") => div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_color(colors.fg)
                            .child(
                                gpui_kit::component::Icon::new(glyph)
                                    .size(px(14.))
                                    .text_color(colors.icon),
                            )
                            .child(text)
                            .into_any_element(),
                        _ => div()
                            .text_color(theme::red())
                            .child(roostr_core::field_str(&row.fields, "error").to_string())
                            .into_any_element(),
                    };
                    cells = cells.child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .text_size(px(13.))
                            .child(div().w(px(200.)).text_color(theme::muted()).child(format!(
                                "{} · {key}",
                                roostr_core::field_str(&row.fields, "name")
                            )))
                            .child(
                                CredentialCell::new(
                                    SharedString::from(format!("cell-{}-{key}", row.id)),
                                    row.clone(),
                                    target,
                                    "Reconnect".into(),
                                )
                                .child(content),
                            ),
                    );
                }
            }
            main = main.child(heading("Credential cells")).child(cells);
        }
        if self.section("type") {
            main = main
                .child(heading("Type panel"))
                .child(self.type_panel.clone());
        }
        if self.section("property") {
            main = main
                .child(heading("Property panel"))
                .child(self.property_panel.clone());
        }
        let _ = window;
        let show_pane = self.section.is_empty() || self.section == "object";
        div()
            .size_full()
            .flex()
            .bg(theme::bg())
            .text_color(theme::fg())
            .child(
                div()
                    .id("main")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(main),
            )
            .when(show_pane, |d| {
                d.child(
                    div()
                        .id("pane")
                        .w(px(380.))
                        .flex_none()
                        .h_full()
                        .overflow_y_scroll()
                        .border_l_1()
                        .border_color(theme::border())
                        .p(px(12.))
                        .child(self.pane.clone()),
                )
            })
    }
}

fn main() {
    roostr_core::dev::run_example("Properties", |args, window, cx| {
        roostr_properties::init(cx);
        cx.new(|cx| Example::new(args, window, cx)).into()
    });
}
