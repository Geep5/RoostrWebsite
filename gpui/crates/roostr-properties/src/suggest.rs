//! Anytype's relationSuggest (`PropertySuggest.svelte`) and typeSuggest
//! (`TypeSuggest.svelte`) menus: one filter box over the space's properties
//! (with "Create property" and its format/type-limit steps) or types.
//!
//! Both are the kit `Command` palette: it owns the query field, the list,
//! hover/arrow highlight and Enter. Roostr supplies the rows (glyph + name),
//! its own filtering and ranking (`filterable(false)`), the create steps, and
//! dismisses on the first Escape (the palette alone would clear the query
//! first).

use std::rc::Rc;

use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::component::IndexPath;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::input::Escape;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::objects::type_glyph;
use roostr_core::relations::{
    CREATABLE_FORMATS, create_relation, format_glyph, is_reserved_key, space_relations,
};
use roostr_core::{RelationDef, Store, TypeDef, theme};

use crate::type_limit::{TypeLimitPicker, toggled};
use crate::widgets::{empty_note, pop_shadow};

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

/// A row's glyph (emoji or format/type glyph) and name, with an optional
/// muted note at the right end.
fn glyph_name(glyph: String, name: String, note: Option<String>) -> Div {
    div()
        .flex()
        .justify_between()
        .items_center()
        .gap(px(10.))
        .w_full()
        .min_w_0()
        .text_size(px(13.))
        .child(
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
                ),
        )
        .when_some(note, |d, note| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(note),
            )
        })
}

/// A palette row drawn by Roostr; `label` is what assistive tech reads.
fn row_item(label: impl Into<SharedString>, row: impl Fn() -> Div + 'static) -> CommandItem {
    CommandItem::new().label(label).child(move |_, _| row())
}

/// What a PropertySuggest row does.
#[derive(Clone)]
enum Target {
    Create,
    Format(&'static str),
    Extra(String),
    Relation(RelationDef),
}

/// Relation picker with search (+ "create property"); place inside a popover/anchored.
pub struct PropertySuggest {
    space_id: String,
    exclude: Vec<String>,
    extras: Vec<(String, String)>,
    command: Entity<CommandState>,
    creating: bool,
    create_format: String,
    create_types: Vec<String>,
    /// The query and step the default highlight was last placed for.
    highlighted_for: Option<(String, bool, String)>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SuggestEvent<PropertyPick>> for PropertySuggest {}

impl Focusable for PropertySuggest {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.command.focus_handle(cx)
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
        let command = cx.new(|cx| CommandState::new(window, cx));
        let subs = vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())];
        let c = command.clone();
        window.defer(cx, move |window, cx| {
            c.update(cx, |c, cx| c.focus(window, cx))
        });
        Self {
            space_id,
            exclude,
            extras,
            command,
            creating: false,
            create_format: String::new(),
            create_types: Vec::new(),
            highlighted_for: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn raw(&self, cx: &App) -> String {
        self.command.read(cx).query(cx).trim().to_string()
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

    fn filtered(all: &[RelationDef], q: &str) -> Vec<RelationDef> {
        all.iter()
            .filter(|r| q.is_empty() || Self::label(r).to_lowercase().contains(q))
            .cloned()
            .collect()
    }

    fn filtered_extras(&self, q: &str) -> Vec<(String, String)> {
        self.extras
            .iter()
            .filter(|(k, n)| {
                !self.exclude.contains(k) && (q.is_empty() || n.to_lowercase().contains(q))
            })
            .cloned()
            .collect()
    }

    fn can_create(all: &[RelationDef], q: &str) -> bool {
        !q.is_empty() && !all.iter().any(|r| Self::label(r).to_lowercase() == q)
    }

    fn confirm(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::Create => {
                self.creating = true;
                cx.notify();
            }
            Target::Format(f) => self.choose(f, window, cx),
            Target::Extra(key) => cx.emit(SuggestEvent::Picked(PropertyPick::Extra(key))),
            Target::Relation(rel) => cx.emit(SuggestEvent::Picked(PropertyPick::Relation(rel))),
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
        let name = self.raw(cx);
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

    /// The step-3 panel (an Object's type limits + Back / Create), shown under
    /// the list once a format that needs it is chosen.
    fn limit_panel(&self, raw: &str, cx: &mut Context<Self>) -> impl Fn() -> AnyElement + 'static {
        let entity = cx.entity().downgrade();
        let types = self.create_types.clone();
        let space = self.space_id.clone();
        let raw = raw.to_string();
        move || {
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
            let (toggle, back, create) = (entity.clone(), entity.clone(), entity.clone());
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(4.))
                .child(
                    div()
                        .px(px(6.))
                        .pt(px(6.))
                        .pb(px(2.))
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .child(format!("Object “{raw}” links to").to_uppercase()),
                )
                .child(TypeLimitPicker::new(
                    "limit",
                    types.clone(),
                    space.clone(),
                    move |id, _, cx| {
                        let _ = toggle.update(cx, |this, cx| {
                            this.create_types = toggled(&this.create_types, id);
                            cx.notify();
                        });
                    },
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .justify_end()
                        .mt(px(6.))
                        .child(button("back", "Back").on_click(move |_, _, cx| {
                            let _ = back.update(cx, |this, cx| {
                                this.create_format.clear();
                                cx.notify();
                            });
                        }))
                        .child(
                            button("create-it", "Create").on_click(move |_, window, cx| {
                                let _ = create.update(cx, |this, cx| this.create(window, cx));
                            }),
                        ),
                )
                .into_any_element()
        }
    }
}

impl Render for PropertySuggest {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let raw = self.raw(cx);
        let q = raw.to_lowercase();
        let all = self.all(cx);
        let filtered = Self::filtered(&all, &q);
        let extras = self.filtered_extras(&q);
        let can_create = Self::can_create(&all, &q);
        let pick_format = can_create && self.creating && self.create_format.is_empty();
        let limits = can_create && self.creating && !self.create_format.is_empty();

        // Rows, one group per section; `targets` maps each row's path to its action.
        let mut groups: Vec<CommandGroup> = Vec::new();
        let mut targets: Vec<(IndexPath, Target)> = Vec::new();
        let mut add_group = |heading: Option<String>, rows: Vec<(CommandItem, Target)>| {
            let section = groups.len();
            let mut group = CommandGroup::new();
            if let Some(h) = heading {
                group = group.label(h.to_uppercase());
            }
            for (row, (item, target)) in rows.into_iter().enumerate() {
                targets.push((IndexPath::new(row).section(section), target));
                group = group.item(item);
            }
            groups.push(group);
        };
        if can_create && !self.creating {
            let text = format!("＋ Create property “{raw}”");
            add_group(
                None,
                vec![(
                    row_item(text.clone(), move || {
                        div()
                            .text_size(px(13.))
                            .text_color(theme::accent())
                            .child(text.clone())
                    }),
                    Target::Create,
                )],
            );
        }
        if pick_format {
            add_group(
                Some(format!("Type of “{raw}”")),
                CREATABLE_FORMATS
                    .iter()
                    .map(|&f| {
                        (
                            row_item(format_name(f).to_string(), move || {
                                glyph_name(
                                    format_glyph(f).to_string(),
                                    format_name(f).to_string(),
                                    None,
                                )
                            }),
                            Target::Format(f),
                        )
                    })
                    .collect(),
            );
        }
        if !extras.is_empty() {
            add_group(
                Some("System".into()),
                extras
                    .into_iter()
                    .map(|(key, name)| {
                        let n = name.clone();
                        (
                            row_item(name, move || div().text_size(px(13.)).child(n.clone())),
                            Target::Extra(key),
                        )
                    })
                    .collect(),
            );
        }
        if !filtered.is_empty() {
            add_group(
                Some("My properties".into()),
                filtered
                    .into_iter()
                    .map(|rel| {
                        let (glyph, name, format) = (
                            rel.icon_emoji.clone().unwrap_or_default(),
                            Self::label(&rel),
                            rel.format.clone(),
                        );
                        (
                            row_item(name.clone(), move || {
                                glyph_name(glyph.clone(), name.clone(), Some(format.clone()))
                            }),
                            Target::Relation(rel),
                        )
                    })
                    .collect(),
            );
        }

        // Enter does what the web menu's did: the first matching property,
        // else "Create property", else the first format. The palette
        // highlights its first row, so the highlight is placed once per query
        // and step; arrows and hover move it from there.
        let step = (raw.clone(), self.creating, self.create_format.clone());
        if self.highlighted_for.as_ref() != Some(&step) {
            self.highlighted_for = Some(step);
            let want = if pick_format {
                targets
                    .iter()
                    .find(|(_, t)| matches!(t, Target::Format(_)))
                    .map(|(ix, _)| *ix)
            } else if !self.creating {
                let first_relation = targets
                    .iter()
                    .find(|(_, t)| matches!(t, Target::Relation(_)))
                    .map(|(ix, _)| *ix);
                first_relation.or_else(|| {
                    targets
                        .iter()
                        .find(|(_, t)| matches!(t, Target::Create))
                        .map(|(ix, _)| *ix)
                })
            } else {
                None
            };
            if let Some(ix) = want {
                // After this render installs the rows the path points into.
                let command = self.command.downgrade();
                window.defer(cx, move |window, cx| {
                    let _ = command.update(cx, |c, cx| c.set_selected_index(Some(ix), window, cx));
                });
            }
        }

        let targets = Rc::new(targets);
        let entity = cx.entity().downgrade();
        let on_query = entity.clone();
        let mut command = Command::new(&self.command)
            .filterable(false)
            .bordered(false)
            .placeholder("Filter or create a property…")
            .max_h(px(280.))
            .on_query(move |_, _, cx| {
                let _ = on_query.update(cx, |_, cx| cx.notify());
            })
            .on_confirm(move |ix, window, cx| {
                let Some(target) = targets
                    .iter()
                    .find(|(p, _)| *p == ix)
                    .map(|(_, t)| t.clone())
                else {
                    return;
                };
                let _ = entity.update(cx, |this, cx| this.confirm(target, window, cx));
            })
            .empty(|_, _, _| {
                empty_note("No properties yet — type a name to create one.")
                    .p(px(6.))
                    .text_size(px(12.))
            });
        for group in groups {
            command = command.group(group);
        }
        if limits {
            let panel = self.limit_panel(&raw, cx);
            command = command.footer(move |_, _, _| panel());
        }

        div()
            .id("property-suggest")
            .track_focus(&self.focus)
            .w(px(280.))
            .max_h(px(340.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .p(px(4.))
            .shadow(pop_shadow())
            .text_color(theme::fg())
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(SuggestEvent::Dismissed)))
            // Step 3 has no row to confirm: Enter creates the property.
            .capture_action(cx.listener(move |this, _: &Confirm, window, cx| {
                if limits {
                    cx.stop_propagation();
                    this.create(window, cx);
                }
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .capture_action(cx.listener(|_, _: &Cancel, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(SuggestEvent::Dismissed);
                }
            }))
            .child(command.bg(theme::panel()).flex_1().min_h_0())
    }
}

/// Type picker with filter input.
pub struct TypeSuggest {
    space_id: String,
    exclude: Vec<String>,
    command: Entity<CommandState>,
    placeholder: SharedString,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SuggestEvent<String>> for TypeSuggest {}

impl Focusable for TypeSuggest {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.command.focus_handle(cx)
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
        let command = cx.new(|cx| CommandState::new(window, cx));
        let subs = vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())];
        let c = command.clone();
        window.defer(cx, move |window, cx| {
            c.update(cx, |c, cx| c.focus(window, cx))
        });
        Self {
            space_id,
            exclude,
            command,
            placeholder,
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

    /// The space's types, name/key prefix matches first, then the rest that contain the query.
    fn matches(&self, cx: &App) -> Vec<TypeDef> {
        let store = Store::global(cx).read(cx);
        let q = self.command.read(cx).query(cx).trim().to_lowercase();
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
}

impl Render for TypeSuggest {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let store = Store::global(cx).read(cx);
        let keys: Rc<Vec<String>> = Rc::new(matches.iter().map(|t| t.key.clone()).collect());
        let items: Vec<CommandItem> = matches
            .iter()
            .map(|t| {
                let icon = if t.icon.is_empty() {
                    type_glyph(store, &t.key, &self.space_id)
                } else {
                    t.icon.clone()
                };
                let name = if t.name.is_empty() {
                    t.key.clone()
                } else {
                    t.name.clone()
                };
                let key = t.key.clone();
                row_item(name.clone(), move || {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .w_full()
                        .text_size(px(13.))
                        .child(
                            div()
                                .flex_none()
                                .w(px(20.))
                                .text_center()
                                .child(icon.clone()),
                        )
                        .child(div().flex_1().child(name.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child(key.clone()),
                        )
                })
            })
            .collect();
        let entity = cx.entity().downgrade();
        let on_query = entity.clone();
        div()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .text_color(theme::fg())
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .capture_action(cx.listener(|_, _: &Cancel, _, cx| {
                cx.stop_propagation();
                cx.emit(SuggestEvent::Dismissed);
            }))
            .child(
                Command::new(&self.command)
                    .filterable(false)
                    .bordered(false)
                    .placeholder(self.placeholder.clone())
                    .max_h(px(240.))
                    .bg(theme::panel())
                    .items(items)
                    .on_query(move |_, _, cx| {
                        let _ = on_query.update(cx, |_, cx| cx.notify());
                    })
                    .on_confirm(move |ix, _, cx| {
                        if let Some(key) = keys.get(ix.row).cloned() {
                            let _ = entity.update(cx, |_, cx| cx.emit(SuggestEvent::Picked(key)));
                        }
                    })
                    .empty(|_, _, _| {
                        empty_note("No matching types.")
                            .px(px(8.))
                            .py(px(6.))
                            .text_size(px(12.))
                    }),
            )
    }
}
