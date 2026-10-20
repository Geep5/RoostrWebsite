//! A type object's page body (`TypePanel.svelte`): its definition (agents
//! read it), layout (Page | Task) and templates, with Anytype's template
//! gallery overlay (mini previews, default badge, duplicate, delete, new).

use std::collections::HashMap;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::apply_template;
use roostr_core::types::style;
use roostr_core::{Fields, Object, Route, Store, Value, api, motion, nav, theme};
use serde_json::json;

use crate::widgets::{modal_shadow, text_area, tip};

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
    gallery_open: bool,
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
            gallery_open: false,
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

    fn open_gallery(&mut self, cx: &mut Context<Self>) {
        self.gallery_open = true;
        self.load_templates(true, cx);
        cx.notify();
    }

    fn add_template(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        self.gallery_open = false;
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

    fn render_gallery(
        &self,
        object: &Object,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let default_id = object.field_str("default_template_id").to_string();
        let name = match object.field_str("name") {
            "" => "object",
            n => n,
        };
        let mut grid = div().flex().flex_wrap().gap(px(12.));
        let card_w = px(182.);
        for t in &self.templates {
            let is_default = default_id == t.id;
            let g: SharedString = format!("tg-{}", t.id).into();
            let mut preview = div()
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
                    Line::Divider => div()
                        .flex_shrink_0()
                        .w_full()
                        .h(px(1.))
                        .bg(theme::border())
                        .my(px(2.)),
                    Line::Head(t) => line(if t.is_empty() {
                        "Heading".into()
                    } else {
                        t.clone()
                    })
                    .text_size(px(11.))
                    .font_weight(FontWeight::BOLD),
                    Line::Check(t) => line(format!("☐ {t}")),
                    Line::List(t) => line(format!("• {t}")),
                    Line::Custom(t) => line(format!("▨ {t}")).text_color(theme::muted()),
                    Line::Para(t) => line(t.clone()),
                });
            }
            if lines.is_empty() {
                preview = preview.child(line("Empty template".into()).text_color(theme::muted()));
            }
            let open_id = t.id.clone();
            let (d_id, dup, rm_id) = (t.id.clone(), t.clone(), t.id.clone());
            let action =
                |id: &'static str, label: &'static str, title: &'static str, hover_fg: Hsla| {
                    div()
                        .id(id)
                        .px(px(5.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(move |s| s.bg(theme::hover()).text_color(hover_fg))
                        .tooltip(tip(title))
                        .child(label)
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
                        div()
                            .id("preview")
                            .cursor_pointer()
                            .tooltip(tip("Edit template"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if std::mem::take(&mut this.action_press) {
                                    return;
                                }
                                this.gallery_open = false;
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
                                    div()
                                        .flex_none()
                                        .text_size(px(10.))
                                        .text_color(theme::accent())
                                        .border_1()
                                        .border_color(theme::accent())
                                        .rounded(px(5.))
                                        .px(px(5.))
                                        .py(px(1.))
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
                                    theme::fg(),
                                )
                                .when(is_default, |d| d.text_color(theme::accent()))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.action_press = false;
                                        this.set_default(d_id.clone(), cx)
                                    },
                                )),
                            )
                            .child(action("dup", "⧉", "Duplicate", theme::fg()).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.action_press = false;
                                    this.duplicate(dup.clone(), cx)
                                }),
                            ))
                            .child(action("del", "🗑", "Delete", rgb(0xe05555).into()).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.action_press = false;
                                    this.remove(rm_id.clone(), window, cx)
                                }),
                            )),
                    ),
            );
        }
        grid = grid.child(
            div()
                .id("tg-new")
                .w(card_w)
                .min_h(px(140.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .border_1()
                .border_dashed()
                .border_color(theme::border())
                .rounded(theme::radius::menu())
                .bg(theme::bg())
                .text_size(px(13.))
                .text_color(theme::muted())
                .cursor_pointer()
                .hover(|s| s.text_color(theme::fg()).border_color(theme::muted()))
                .on_click(cx.listener(|this, _, _, cx| this.add_template(cx)))
                .child(div().text_size(px(22.)).child("＋"))
                .child("New template"),
        );
        let size = window.viewport_size();
        // The modal rises in as it opens; closing drops it at once.
        let appear = motion::appear_frame("tg-modal-appear", 8., window, cx);
        deferred(
            anchored().position(point(px(0.), px(0.))).position_mode(AnchoredPositionMode::Window).child(
                div()
                    .id("tg-overlay")
                    .w(size.width)
                    .h(size.height)
                    .bg(hsla(0., 0., 0., 0.55))
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.gallery_open = false;
                            cx.notify();
                        }),
                    )
                    .child(appear.apply(
                        div()
                            .id("tg-modal")
                            .w(px(640.))
                            .max_w(size.width - px(48.))
                            .max_h(size.height * 0.78)
                            .overflow_y_scroll()
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .rounded(theme::radius::modal())
                            .pt(px(18.))
                            .px(px(22.))
                            .pb(px(22.))
                            .shadow(modal_shadow())
                            .text_color(theme::fg())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("⊞ Templates"))
                                    .child(
                                        div()
                                            .id("tg-x")
                                            .text_size(px(20.))
                                            .text_color(theme::muted())
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.gallery_open = false;
                                                cx.notify();
                                            }))
                                            .child("×"),
                                    ),
                            )
                            .child(div().mt(px(4.)).mb(px(14.)).text_size(px(12.)).text_color(theme::muted()).child(format!(
                                "A default template pre-fills every new {name}; the New button's ▾ picks one per object."
                            )))
                            .child(grid),
                    )),
            ),
        )
        .with_priority(4)
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
            layouts = layouts.child(
                div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(2.))
                    .min_w(px(160.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_1()
                    .border_color(if layout == id {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .rounded(theme::radius::menu())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_layout(id, cx)))
                    .child(
                        div()
                            .text_size(px(14.))
                            .child(format!("{} {name}", if id == "task" { "✅" } else { "📄" })),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(hint),
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
                div()
                    .mb(px(10.))
                    .hover(|s| s.border_color(theme::muted()))
                    // A textarea blurs on any outside click (the web's onblur save).
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        if this.definition.focus_handle(cx).is_focused(window) {
                            window.blur(cx);
                        }
                    }))
                    .child(
                        text_area(
                            &self.definition,
                            window,
                            cx,
                            theme::hover(),
                            theme::radius::md(),
                            (8., 10.),
                        )
                        .text_size(px(13.5)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(8.))
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child("LAYOUT"),
                    )
                    .child(layouts),
            )
            .child(
                div().mt(px(-8.)).flex().child(
                    div()
                        .id("tpl-chip")
                        .border_1()
                        .border_color(theme::border())
                        .rounded_full()
                        .px(px(12.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::fg()).border_color(theme::muted()))
                        .on_click(cx.listener(|this, _, _, cx| this.open_gallery(cx)))
                        .child(if n > 0 {
                            format!("⊞ Templates · {n}")
                        } else {
                            "⊞ Templates".into()
                        }),
                ),
            )
            .when(self.gallery_open, |d| {
                d.child(self.render_gallery(&object, window, cx))
            })
    }
}
