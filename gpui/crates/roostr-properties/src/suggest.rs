//! Anytype's relationSuggest (`PropertySuggest.svelte`) and typeSuggest
//! (`TypeSuggest.svelte`) menus: one filter box over the space's properties
//! (with "Create property" and its format/type-limit steps) or types.

use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::objects::type_glyph;
use roostr_core::relations::{
    CREATABLE_FORMATS, create_relation, format_glyph, is_reserved_key, space_relations,
};
use roostr_core::{RelationDef, Store, TypeDef, theme};

use crate::type_limit::{TypeLimitPicker, toggled};
use crate::widgets::{FieldSkin, field, pop_shadow};

/// What a suggest menu reports to its host.
pub enum SuggestEvent<T> {
    /// The person chose this.
    Picked(T),
    /// Escape, or a press outside the menu.
    Dismissed,
}

/// A PropertySuggest choice.
pub enum PropertyPick {
    /// An existing or just-created property.
    Relation(RelationDef),
    /// One of the host's non-relation `extras` (its key).
    Extra(String),
}

fn format_name(format: &str) -> &str {
    match format {
        "shorttext" => "Text",
        "longtext" => "Long text",
        "number" => "Number",
        "status" => "Status",
        "tag" => "Tag",
        "date" => "Date",
        "checkbox" => "Checkbox",
        "url" => "URL",
        "email" => "Email",
        "phone" => "Phone",
        "object" => "Object",
        other => other,
    }
}

fn section(label: String) -> Div {
    div()
        .px(px(6.))
        .pt(px(6.))
        .pb(px(2.))
        .text_size(px(10.))
        .text_color(theme::muted())
        .child(SharedString::from(label.to_uppercase()))
}

fn item(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .justify_between()
        .items_center()
        .gap(px(10.))
        .px(px(8.))
        .py(px(6.))
        .rounded(theme::radius::sm())
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
}

fn glyph_name(glyph: String, name: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .child(div().flex_none().w(px(18.)).text_center().child(glyph))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        )
}

/// Relation picker with search (+ "create property"); place inside a popover/anchored.
pub struct PropertySuggest {
    space_id: String,
    exclude: Vec<String>,
    extras: Vec<(String, String)>,
    query: Entity<InputState>,
    creating: bool,
    create_format: String,
    create_types: Vec<String>,
    fmt_idx: usize,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SuggestEvent<PropertyPick>> for PropertySuggest {}

impl Focusable for PropertySuggest {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl PropertySuggest {
    /// `exclude`: relation keys to hide (already present); `extras`: `(key, name)`
    /// non-relation options (system columns like Type/Created).
    pub fn new(
        space_id: String,
        exclude: Vec<String>,
        extras: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter or create a property…"));
        let subs = vec![
            cx.subscribe_in(
                &query,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.enter(window, cx),
                    _ => {}
                },
            ),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        let q = query.clone();
        window.defer(cx, move |window, cx| {
            q.update(cx, |s, cx| s.focus(window, cx))
        });
        Self {
            space_id,
            exclude,
            extras,
            query,
            creating: false,
            create_format: String::new(),
            create_types: Vec::new(),
            fmt_idx: 0,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn q(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_lowercase()
    }

    fn all(&self, cx: &App) -> Vec<RelationDef> {
        let store = Store::global(cx).read(cx);
        space_relations(&store.relations, &self.space_id)
            .into_iter()
            .filter(|r| !r.hidden && !is_reserved_key(&r.key) && !self.exclude.contains(&r.key))
            .cloned()
            .collect()
    }

    fn label(r: &RelationDef) -> String {
        if r.name.is_empty() {
            r.key.clone()
        } else {
            r.name.clone()
        }
    }

    fn filtered(&self, all: &[RelationDef], cx: &App) -> Vec<RelationDef> {
        let q = self.q(cx);
        all.iter()
            .filter(|r| q.is_empty() || Self::label(r).to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn filtered_extras(&self, cx: &App) -> Vec<(String, String)> {
        let q = self.q(cx);
        self.extras
            .iter()
            .filter(|(k, n)| {
                !self.exclude.contains(k) && (q.is_empty() || n.to_lowercase().contains(&q))
            })
            .cloned()
            .collect()
    }

    fn can_create(&self, all: &[RelationDef], cx: &App) -> bool {
        let q = self.q(cx);
        !q.is_empty() && !all.iter().any(|r| Self::label(r).to_lowercase() == q)
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all = self.all(cx);
        let filtered = self.filtered(&all, cx);
        let can_create = self.can_create(&all, cx);
        if !filtered.is_empty() && !self.creating {
            cx.emit(SuggestEvent::Picked(PropertyPick::Relation(
                filtered[0].clone(),
            )));
        } else if can_create && !self.creating {
            self.creating = true;
            cx.notify();
        } else if can_create && self.create_format.is_empty() {
            self.choose(CREATABLE_FORMATS[self.fmt_idx], window, cx);
        } else if can_create {
            self.create(window, cx);
        }
    }

    /// A new property's type is always asked: an Object then asks which types it may link.
    fn choose(&mut self, format: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.create_format = format.to_string();
        if format != "object" {
            self.create(window, cx);
        }
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.query.read(cx).value().trim().to_string();
        let task = create_relation(
            &name,
            &self.create_format,
            &self.create_types,
            &self.space_id,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| match task.await {
            Ok(rel) => {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(SuggestEvent::Picked(PropertyPick::Relation(rel)))
                });
            }
            Err(error) => log::error!("create property: {error:#}"),
        })
        .detach();
    }

    fn move_fmt(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = CREATABLE_FORMATS.len();
        self.fmt_idx = (self.fmt_idx + if down { 1 } else { n - 1 }) % n;
        cx.notify();
    }
}

impl Render for PropertySuggest {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.all(cx);
        let filtered = self.filtered(&all, cx);
        let extras = self.filtered_extras(cx);
        let can_create = self.can_create(&all, cx);
        let raw = self.query.read(cx).value().trim().to_string();
        let mut menu = div()
            .id("property-suggest")
            .track_focus(&self.focus)
            .w(px(280.))
            .max_h(px(340.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(8.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(SuggestEvent::Dismissed)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.creating && this.create_format.is_empty() {
                    cx.stop_propagation();
                    this.move_fmt(true, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.creating && this.create_format.is_empty() {
                    cx.stop_propagation();
                    this.move_fmt(false, cx);
                }
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(SuggestEvent::Dismissed);
                }
            }))
            .child(
                field(&self.query, FieldSkin::Search, window, cx)
                    .bg(theme::hl_light())
                    .border_0(),
            );
        if can_create {
            if !self.creating {
                menu = menu.child(
                    item("create")
                        .text_color(theme::accent())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.creating = true;
                            cx.notify();
                        }))
                        .child(format!("＋ Create property “{raw}”")),
                );
            } else if self.create_format.is_empty() {
                menu = menu.child(section(format!("Type of “{raw}”")));
                let mut list = div()
                    .id("formats")
                    .flex()
                    .flex_col()
                    .flex_shrink(1.)
                    .min_h_0()
                    .overflow_y_scroll();
                for (i, f) in CREATABLE_FORMATS.iter().enumerate() {
                    let active = i == self.fmt_idx;
                    list = list.child(
                        item(SharedString::from(format!("fmt-{f}")))
                            .when(active, |d| d.bg(theme::hover()))
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered && this.fmt_idx != i {
                                    this.fmt_idx = i;
                                    cx.notify();
                                }
                            }))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.choose(f, window, cx)),
                            )
                            .child(glyph_name(
                                format_glyph(f).to_string(),
                                format_name(f).to_string(),
                            )),
                    );
                }
                menu = menu.child(list);
            } else {
                menu = menu.child(section(format!("Object “{raw}” links to")));
                let entity = cx.entity().downgrade();
                menu = menu.child(TypeLimitPicker::new(
                    "limit",
                    self.create_types.clone(),
                    self.space_id.clone(),
                    move |id, _, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            this.create_types = toggled(&this.create_types, id);
                            cx.notify();
                        });
                    },
                ));
                let button = |id: &'static str, label: &'static str| {
                    div()
                        .id(id)
                        .border_1()
                        .border_color(theme::border())
                        .rounded(theme::radius::sm())
                        .px(px(10.))
                        .py(px(4.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme::accent()))
                        .child(label)
                };
                menu =
                    menu.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .justify_end()
                            .mt(px(6.))
                            .child(button("back", "Back").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.create_format.clear();
                                    cx.notify();
                                },
                            )))
                            .child(button("create-it", "Create").on_click(
                                cx.listener(|this, _, window, cx| this.create(window, cx)),
                            )),
                    );
            }
        }
        if !extras.is_empty() {
            menu = menu.child(section("System".into()));
            let mut list = div()
                .id("extras")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .overflow_y_scroll();
            for (key, name) in extras.iter().cloned() {
                list = list.child(
                    item(SharedString::from(format!("extra-{key}")))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SuggestEvent::Picked(PropertyPick::Extra(key.clone())))
                        }))
                        .child(div().child(name)),
                );
            }
            menu = menu.child(list);
        }
        if !filtered.is_empty() {
            menu = menu.child(section("My properties".into()));
            let mut list = div()
                .id("relations")
                .flex()
                .flex_col()
                .flex_shrink(1.)
                .min_h_0()
                .overflow_y_scroll();
            for rel in filtered {
                let r = rel.clone();
                list = list.child(
                    item(SharedString::from(format!("rel-{}", rel.key)))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SuggestEvent::Picked(PropertyPick::Relation(r.clone())))
                        }))
                        .child(glyph_name(
                            rel.icon_emoji.clone().unwrap_or_default(),
                            Self::label(&rel),
                        ))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child(rel.format.clone()),
                        ),
                );
            }
            menu = menu.child(list);
        } else if !can_create && extras.is_empty() {
            menu = menu.child(
                div()
                    .p(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("No properties yet — type a name to create one."),
            );
        }
        menu
    }
}

/// Type picker with filter input.
pub struct TypeSuggest {
    space_id: String,
    exclude: Vec<String>,
    query: Entity<InputState>,
    selected: usize,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SuggestEvent<String>> for TypeSuggest {}

impl Focusable for TypeSuggest {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl TypeSuggest {
    /// The types of `space_id` (plus Query and Collection) minus `exclude` keys.
    pub fn new(
        space_id: String,
        exclude: Vec<String>,
        placeholder: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subs = vec![
            cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    if let Some(t) = this.matches(cx).get(this.selected) {
                        let key = t.key.clone();
                        this.pick(key, cx);
                    }
                }
                _ => {}
            }),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        let q = query.clone();
        window.defer(cx, move |window, cx| {
            q.update(cx, |s, cx| s.focus(window, cx))
        });
        Self {
            space_id,
            exclude,
            query,
            selected: 0,
            _subs: subs,
        }
    }

    /// Replace the excluded keys (the host's list changed).
    pub(crate) fn set_exclude(&mut self, exclude: Vec<String>, cx: &mut Context<Self>) {
        if self.exclude != exclude {
            self.exclude = exclude;
            cx.notify();
        }
    }

    fn matches(&self, cx: &App) -> Vec<TypeDef> {
        let store = Store::global(cx).read(cx);
        let q = self.query.read(cx).value().trim().to_lowercase();
        let space = if self.space_id.is_empty() {
            store
                .channels
                .first()
                .map(|s| s.id.clone())
                .unwrap_or_default()
        } else {
            self.space_id.clone()
        };
        // Queries and collections are built-in kinds, not type objects, but a query can list them.
        let view_kind = |key: &str, name: &str| TypeDef {
            id: String::new(),
            key: key.into(),
            name: name.into(),
            icon: type_glyph(store, key, &space),
            layout: "page".into(),
            default_template_id: String::new(),
            space: String::new(),
            builtin: true,
        };
        let pool: Vec<TypeDef> = store
            .types
            .iter()
            .filter(|t| t.space == space)
            .cloned()
            .chain([
                view_kind("query", "Query"),
                view_kind("collection", "Collection"),
            ])
            .filter(|t| !self.exclude.contains(&t.key))
            .collect();
        if q.is_empty() {
            return pool;
        }
        let starts: Vec<TypeDef> = pool
            .iter()
            .filter(|t| t.name.to_lowercase().starts_with(&q) || t.key.starts_with(&q))
            .cloned()
            .collect();
        let contains = pool.into_iter().filter(|t| {
            !starts.iter().any(|s| s.id == t.id && s.key == t.key)
                && (t.name.to_lowercase().contains(&q) || t.key.contains(&q))
        });
        starts.iter().cloned().chain(contains).collect()
    }

    fn pick(&mut self, key: String, cx: &mut Context<Self>) {
        cx.emit(SuggestEvent::Picked(key));
    }
}

impl Render for TypeSuggest {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let store = Store::global(cx).read(cx);
        let mut options = div()
            .id("type-options")
            .flex()
            .flex_col()
            .max_h(px(240.))
            .overflow_y_scroll();
        for (i, t) in matches.iter().enumerate() {
            let key = t.key.clone();
            let icon = if t.icon.is_empty() {
                type_glyph(store, &t.key, &self.space_id)
            } else {
                t.icon.clone()
            };
            options = options.child(
                div()
                    .id(SharedString::from(format!("type-{}-{}", t.id, t.key)))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(i == self.selected, |d| d.bg(theme::hl_med()))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.selected != i {
                            this.selected = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(key.clone(), cx)))
                    .child(div().flex_none().w(px(20.)).text_center().child(icon))
                    .child(div().flex_1().child(if t.name.is_empty() {
                        t.key.clone()
                    } else {
                        t.name.clone()
                    }))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(t.key.clone()),
                    ),
            );
        }
        if matches.is_empty() {
            options = options.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("No matching types."),
            );
        }
        let n = matches.len();
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .min_w(px(240.))
            .text_color(theme::fg())
            .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.selected = (this.selected + 1).min(n.saturating_sub(1));
                cx.notify();
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.selected = this.selected.saturating_sub(1);
                cx.notify();
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .child(field(&self.query, FieldSkin::Suggest, window, cx))
            .child(options)
    }
}
