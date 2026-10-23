//! A type object's page body (`TypePanel.svelte`): its definition (agents
//! read it), layout (Page | Task) and templates, with Anytype's template
//! gallery overlay (mini previews, default badge, duplicate, delete, new).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{Dialog, DialogEntrance};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::label::Label;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ElementExt as _, IconName, IconNamed as _, Selectable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::apply_template;
use roostr_core::types::style;
use roostr_core::{Fields, Glyph, Object, Route, Store, Value, api, motion, nav, theme};
use serde_json::json;

use crate::widgets::{note, rule, text_area};

/// The gallery card's chrome around its scrolling body: 18 + 22 padding and
/// the 2×1 border.
const GALLERY_CHROME: Pixels = px(42.);

const LAYOUTS: [(&str, &str, &str); 2] = [
    ("page", "Page", "Title, icon, and blocks"),
    ("task", "Task", "Checkbox bound to Done"),
];

#[derive(Clone)]
struct Template {
    id: String,
    name: String,
}

#[derive(Clone)]
enum Line {
    Divider,
    Head(String),
    Check(String),
    List(String),
    Custom(String),
    Para(String),
}

/// Type object page body: definition, layout, templates.
pub struct TypePanel {
    object_id: String,
    definition: Entity<TextareaState>,
    synced: Option<String>,
    templates: Vec<Template>,
    /// The gallery body's last laid-out height, to centre the dialog.
    gallery_height: Rc<Cell<Pixels>>,
    /// A card action was pressed: the preview under it must not open.
    action_press: bool,
    previews: HashMap<String, Vec<Line>>,
    _subs: Vec<Subscription>,
}

impl TypePanel {
    /// The panel for the type object `object_id`.
    pub fn new(object_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let definition = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 20)
                .placeholder("What is this type for? Agents read this.")
        });
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&definition, |this: &mut Self, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Blur) {
                    this.save_definition(cx);
                }
            }),
        ];
        let mut this = Self {
            object_id,
            definition,
            synced: None,
            templates: vec![],
            gallery_height: Rc::new(Cell::new(px(300.))),
            action_press: false,
            previews: HashMap::new(),
            _subs: subs,
        };
        this.load_templates(false, cx);
        this
    }

    fn object(&self, cx: &App) -> Option<std::sync::Arc<Object>> {
        Store::global(cx).read(cx).object(&self.object_id)
    }

    fn save_definition(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let draft = self.definition.read(cx).value().trim().to_string();
        if draft == object.field_str("description") {
            return;
        }
        self.synced = Some(draft.clone());
        api::note::set_field(&self.object_id, "description", Value::string(draft), cx)
            .detach_and_log_err(cx);
    }

    fn set_layout(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let layout = match object.field_str("layout") {
            "" => "page",
            l => l,
        };
        if id == layout {
            return;
        }
        let task = api::note::set_field(&self.object_id, "layout", Value::string(id), cx);
        let store = Store::global(cx);
        cx.spawn(async move |_, cx| {
            if task.await.is_ok() {
                store.update(cx, |s, cx| s.refresh_all(cx));
            }
        })
        .detach();
    }

    fn load_templates(&mut self, previews: bool, cx: &mut Context<Self>) {
        let task = api::query(
            json!({ "type": "template", "filters": [{ "key": "target_type", "condition": "equal", "value": self.object_id }], "limit": 50 }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(res) = task.await else { return };
            let list: Vec<Template> = res
                .records
                .into_iter()
                .map(|r| Template {
                    name: roostr_core::field_str(&r.fields, "name").to_string(),
                    id: r.id,
                })
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.templates = list;
                if previews {
                    let ids: Vec<String> = this
                        .templates
                        .iter()
                        .filter(|t| !this.previews.contains_key(&t.id))
                        .map(|t| t.id.clone())
                        .collect();
                    for id in ids {
                        this.load_preview(id, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// First content blocks of a template, flattened for the mini preview.
    fn load_preview(&mut self, id: String, cx: &mut Context<Self>) {
        let task = Store::global(cx).read(cx).fetch_object(&id, cx);
        cx.spawn(async move |this, cx| {
            let lines = match task.await {
                Ok(tpl) => {
                    let mut skip = std::collections::HashSet::new();
                    fn mark(id: &str, tpl: &Object, skip: &mut std::collections::HashSet<String>) {
                        if !skip.insert(id.to_string()) {
                            return;
                        }
                        if let Some(b) = tpl.block(id) {
                            for c in &b.children_ids {
                                mark(c, tpl, skip);
                            }
                        }
                    }
                    for b in &tpl.blocks {
                        if b.content
                            .custom
                            .as_ref()
                            .is_some_and(|c| c.content_type == "discussion")
                        {
                            mark(&b.id, &tpl, &mut skip);
                        }
                    }
                    let mut lines = Vec::new();
                    for b in &tpl.blocks {
                        if skip.contains(&b.id) || lines.len() >= 7 {
                            continue;
                        }
                        if let Some(c) = &b.content.custom {
                            lines.push(if c.content_type == "divider" {
                                Line::Divider
                            } else {
                                Line::Custom(if c.content_type.is_empty() {
                                    "block".into()
                                } else {
                                    c.content_type.clone()
                                })
                            });
                        } else if let Some(t) = &b.content.text {
                            let text = t.text.clone();
                            lines.push(match t.style {
                                style::HEADER1 | style::HEADER2 | style::HEADER3 => {
                                    Line::Head(text)
                                }
                                style::CHECKBOX => Line::Check(text),
                                style::BULLET | style::NUMBERED => Line::List(text),
                                _ => Line::Para(text),
                            });
                        }
                    }
                    lines
                }
                Err(_) => vec![],
            };
            let _ = this.update(cx, |this, cx| {
                this.previews.insert(id, lines);
                cx.notify();
            });
        })
        .detach();
    }

    fn template_fields(object: &Object) -> Fields {
        let mut f = Fields::from([("target_type".to_string(), Value::string(object.id.clone()))]);
        let space = object.field_str("channel");
        if !space.is_empty() {
            f.insert("channel".into(), Value::string(space));
        }
        f
    }

    /// Open the template gallery in the window's dialog layer; it renders
    /// this panel's state (weak: the dialog closes once the panel is gone).
    fn open_gallery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_templates(true, cx);
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| match this.upgrade() {
            Some(view) => view.update(cx, |this, cx| this.gallery(dialog, window, cx)),
            None => {
                window.defer(cx, |window, cx| window.close_dialog(cx));
                dialog.close_button(false)
            }
        });
    }

    fn add_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        window.close_dialog(cx);
        let task = api::note::create(
            "New template",
            Some("template"),
            Some(Self::template_fields(&object)),
            cx,
        );
        cx.spawn(async move |_, cx| {
            if let Ok(id) = task.await {
                cx.update(|cx| nav::navigate(Route::Object(id), cx));
            }
        })
        .detach();
        cx.notify();
    }

    fn duplicate(&mut self, t: Template, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let name = format!(
            "{} copy",
            if t.name.is_empty() {
                "Untitled"
            } else {
                &t.name
            }
        );
        let task = api::note::create(
            &name,
            Some("template"),
            Some(Self::template_fields(&object)),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(id) = task.await else { return };
            let apply = cx.update(|cx| apply_template(&id, &t.id, cx));
            let _ = apply.await;
            let _ = this.update(cx, |this, cx| {
                this.load_templates(false, cx);
                this.load_preview(id, cx);
            });
        })
        .detach();
    }

    fn set_default(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let task = if object.field_str("default_template_id") == id {
            api::note::delete_field(&self.object_id, "default_template_id", cx)
        } else {
            api::note::set_field(
                &self.object_id,
                "default_template_id",
                Value::string(id),
                cx,
            )
        };
        let store = Store::global(cx);
        cx.spawn(async move |_, cx| {
            if task.await.is_ok() {
                store.update(cx, |s, cx| s.refresh_all(cx));
            }
        })
        .detach();
    }

    fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Warning,
            "Delete this template?",
            None,
            &["Delete", "Cancel"],
            cx,
        );
        let is_default = self
            .object(cx)
            .is_some_and(|o| o.field_str("default_template_id") == id);
        let object_id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            if is_default {
                let t =
                    cx.update(|cx| api::note::delete_field(&object_id, "default_template_id", cx));
                let _ = t.await;
            }
            let t = cx.update(|cx| api::note::delete(&id, cx));
            let _ = t.await;
            let _ = this.update(cx, |this, cx| {
                this.load_templates(false, cx);
                Store::global(cx).update(cx, |s, cx| s.refresh_all(cx));
            });
        })
        .detach();
    }

    /// The template gallery dialog: mini-preview cards (open, default ★,
    /// duplicate, delete) and the New template card. It rises 8px into
    /// place while the layer fades in, centred on its last laid-out height.
    fn gallery(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let Some(object) = self.object(cx) else {
            window.defer(cx, |window, cx| window.close_dialog(cx));
            return dialog.close_button(false);
        };
        let default_id = object.field_str("default_template_id").to_string();
        let name = match object.field_str("name") {
            "" => "object",
            n => n,
        };
        let mut grid = div().flex().flex_wrap().gap(px(12.));
        let card_w = px(182.);
        // Card actions: transparent, `hover` fill; the glyph takes its hover
        // colour from the button's group.
        let action_variant = ButtonCustomVariant::new(cx)
            .hover(theme::hover())
            .active(theme::hover());
        for t in &self.templates {
            let is_default = default_id == t.id;
            let g: SharedString = format!("tg-{}", t.id).into();
            let mut preview = div()
                .w_full()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(3.))
                .h(px(108.))
                .py(px(10.))
                .px(px(12.))
                .overflow_hidden();
            let lines = self.previews.get(&t.id).cloned().unwrap_or_default();
            let line = |text: String| {
                div()
                    .flex_shrink_0()
                    .max_w_full()
                    .text_size(px(9.))
                    .line_height(relative(1.35))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(text)
            };
            for l in &lines {
                preview = preview.child(match l {
                    Line::Divider => rule().flex_shrink_0().my(px(2.)).into_any_element(),
                    Line::Head(t) => line(if t.is_empty() {
                        "Heading".into()
                    } else {
                        t.clone()
                    })
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD)
                    .into_any_element(),
                    Line::Check(t) => line(format!("☐ {t}")).into_any_element(),
                    Line::List(t) => line(format!("• {t}")).into_any_element(),
                    Line::Custom(t) => line(format!("▨ {t}"))
                        .text_color(theme::muted())
                        .into_any_element(),
                    Line::Para(t) => line(t.clone()).into_any_element(),
                });
            }
            if lines.is_empty() {
                preview = preview.child(line("Empty template".into()).text_color(theme::muted()));
            }
            let open_id = t.id.clone();
            let (d_id, dup, rm_id) = (t.id.clone(), t.clone(), t.id.clone());
            let action = |id: &'static str,
                          label: &'static str,
                          title: &'static str,
                          fg: Hsla,
                          hover_fg: Hsla| {
                let group: SharedString = format!("{g}-{id}").into();
                Button::new(id)
                    .custom(action_variant)
                    .group(group.clone())
                    .rounded(px(5.))
                    .h_auto()
                    .px(px(5.))
                    .py(px(2.))
                    .cursor_pointer()
                    .tooltip(title)
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(fg)
                            .group_hover(group, move |s| s.text_color(hover_fg))
                            .child(label),
                    )
            };
            grid = grid.child(
                div()
                    .id(g.clone())
                    .group(g.clone())
                    .relative()
                    .w(card_w)
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(if is_default {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .rounded(theme::radius::menu())
                    .bg(theme::bg())
                    .overflow_hidden()
                    .child(
                        Button::new("preview")
                            .custom(ButtonCustomVariant::new(cx))
                            .rounded(px(0.))
                            .w_full()
                            .justify_start()
                            .h_auto()
                            .p_0()
                            .text_color(theme::fg())
                            .cursor_pointer()
                            .tooltip("Edit template")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if std::mem::take(&mut this.action_press) {
                                    return;
                                }
                                window.close_dialog(cx);
                                nav::navigate(Route::Object(open_id.clone()), cx);
                            }))
                            .child(preview),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(10.))
                            .py(px(7.))
                            .border_t_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if t.name.is_empty() {
                                        "Untitled".to_string()
                                    } else {
                                        t.name.clone()
                                    }),
                            )
                            .when(is_default, |d| {
                                d.child(
                                    Tag::custom(
                                        transparent_black(),
                                        theme::accent(),
                                        theme::accent(),
                                    )
                                    .flex_none()
                                    .rounded(px(5.))
                                    .px(px(5.))
                                    .py(px(1.))
                                    .text_size(px(10.))
                                    .child("Default"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.action_press = true),
                            )
                            .top(px(6.))
                            .right(px(6.))
                            .flex()
                            .invisible()
                            .group_hover(g.clone(), |s| s.visible())
                            .gap(px(2.))
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(px(7.))
                            .p(px(2.))
                            .child(
                                action(
                                    "star",
                                    "★",
                                    if is_default {
                                        "Unset default"
                                    } else {
                                        "Set as default"
                                    },
                                    if is_default {
                                        theme::accent()
                                    } else {
                                        theme::muted()
                                    },
                                    theme::fg(),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.action_press = false;
                                        this.set_default(d_id.clone(), cx)
                                    },
                                )),
                            )
                            .child(
                                action("dup", "⧉", "Duplicate", theme::muted(), theme::fg())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.action_press = false;
                                        this.duplicate(dup.clone(), cx)
                                    })),
                            )
                            .child(
                                action("del", "🗑", "Delete", theme::muted(), rgb(0xe05555).into())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.action_press = false;
                                        this.remove(rm_id.clone(), window, cx)
                                    })),
                            ),
                    ),
            );
        }
        grid = grid.child(
            Button::new("tg-new")
                .outline()
                .group("tg-new")
                .w(card_w)
                .min_h(px(140.))
                .h_auto()
                .rounded(theme::radius::menu())
                .border_dashed()
                .border_color(theme::border())
                .bg(theme::bg())
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.add_template(window, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .group_hover("tg-new", |s| s.text_color(theme::fg()))
                        .child(
                            svg()
                                .path(Glyph::Plus.path())
                                .size(px(22.))
                                .text_color(theme::muted())
                                .group_hover("tg-new", |s| s.text_color(theme::fg())),
                        )
                        .child("New template"),
                ),
        );
        let size = window.viewport_size();
        // The card rises 8px into place as it opens; closing drops it at once.
        let appear = motion::appear_frame("tg-modal-appear", 8., window, cx);
        let card = self.gallery_height.get() + GALLERY_CHROME;
        let top = ((size.height - card) / 2.).max(px(0.)) + px(appear.target().y_px());
        let height = self.gallery_height.clone();
        dialog
            .entrance(DialogEntrance::Fade)
            .close_button(false)
            .w(px(640.))
            .max_w(size.width - px(48.))
            .margin_top(top)
            .pt(px(18.))
            .px(px(22.))
            .pb(px(22.))
            .bg(theme::panel())
            .border_color(theme::border())
            .rounded(theme::radius::modal())
            .text_color(theme::fg())
            .child(
                div()
                    .id("tg-modal")
                    .max_h(size.height * 0.78 - GALLERY_CHROME)
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                Label::new("⊞ Templates")
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::fg()),
                            )
                            .child(
                                Button::new("tg-x")
                                    .ghost()
                                    .icon(IconName::Close)
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .accessibility_label("Close")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                    )
                    .child(
                        note(
                            format!(
                                "A default template pre-fills every new {name}; the New button's ▾ picks one per object."
                            ),
                            12.,
                        )
                        .mt(px(4.))
                        .mb(px(14.)),
                    )
                    .child(grid)
                    .on_prepaint(move |bounds, window, _| {
                        if height.get() != bounds.size.height {
                            height.set(bounds.size.height);
                            window.request_animation_frame();
                        }
                    }),
            )
    }
}

impl Render for TypePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return div();
        };
        let description = object.field_str("description").to_string();
        if self.synced.as_deref() != Some(description.as_str())
            && !self.definition.focus_handle(cx).is_focused(window)
        {
            self.definition
                .update(cx, |s, cx| s.set_value(description.clone(), window, cx));
            self.synced = Some(description);
        }
        let layout = match object.field_str("layout") {
            "" => "page",
            l => l,
        }
        .to_string();
        let mut layouts = div().flex().gap(px(8.));
        for (id, name, hint) in LAYOUTS {
            let selected = layout == id;
            layouts = layouts.child(
                Button::new(id)
                    .outline()
                    .selected(selected)
                    .h_auto()
                    .min_w(px(160.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(theme::radius::menu())
                    .bg(transparent_black())
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .text_color(theme::fg())
                    // Title + hint sit left like the web card, not kit-centred.
                    .justify_start()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_layout(id, cx)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(2.))
                            .child(
                                Label::new(format!(
                                    "{} {name}",
                                    if id == "task" { "✅" } else { "📄" }
                                ))
                                .text_size(px(14.))
                                .text_color(theme::fg()),
                            )
                            .child(note(hint, 11.)),
                    ),
            );
        }
        let n = self.templates.len();
        let narrow = roostr_core::layout::is_mobile(window);
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .mt(px(16.))
            // `.type-panel`: `margin: 16px 0 0 48px`, `16px 16px 0` at ≤720px.
            .when(narrow, |d| d.mx(px(16.)))
            .when(!narrow, |d| d.ml(px(48.)))
            .text_color(theme::fg())
            .child(
                // The textarea itself (no wrapper); it blurs on any outside
                // click (the web's onblur save).
                text_area(
                    &self.definition,
                    window,
                    cx,
                    theme::hover(),
                    theme::radius::md(),
                    (8., 10.),
                )
                .mb(px(10.))
                .text_size(px(13.5))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    if this.definition.focus_handle(cx).is_focused(window) {
                        window.blur(cx);
                    }
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(note("LAYOUT", 11.).mb(px(8.)))
                    .child(layouts),
            )
            .child(
                div().mt(px(-8.)).flex().child(
                    Button::new("tpl-chip")
                        .outline()
                        .group("tpl-chip")
                        .rounded_full()
                        .h_auto()
                        .px(px(12.))
                        .py(px(4.))
                        .bg(transparent_black())
                        .border_color(theme::border())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.open_gallery(window, cx)))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .group_hover("tpl-chip", |s| s.text_color(theme::fg()))
                                .child(if n > 0 {
                                    format!("⊞ Templates · {n}")
                                } else {
                                    "⊞ Templates".into()
                                }),
                        ),
                ),
            )
    }
}
