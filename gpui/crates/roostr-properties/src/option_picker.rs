//! Anytype's option list + edit (`OptionPicker.svelte`): a filter box over
//! the property's options; typing an unknown name offers
//! `Create option "<name>"` (random palette color); every row has a ⋯ edit
//! affordance opening name + the 10-swatch palette + Delete option. Select
//! (status) picks one; multi-select (tag) toggles.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::options::{
    TAG_COLORS, add_option, delete_option, rename_option, set_option_color,
};
use roostr_core::ui::TagPill;
use roostr_core::{RelationDef, RelationOption, Store, theme};

use crate::widgets::{FieldSkin, empty_note, field, tip};

pub(crate) enum OptionEvent {
    /// Toggle/select this option text.
    Pick(String),
}

pub(crate) struct OptionPicker {
    rel: RelationDef,
    selected: Vec<String>,
    multi: bool,
    query: Entity<InputState>,
    placeholder: &'static str,
    /// Option id with the edit panel open.
    editing: String,
    name_draft: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<OptionEvent> for OptionPicker {}

impl OptionPicker {
    pub(crate) fn new(
        rel: RelationDef,
        selected: Vec<String>,
        multi: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let name_draft = cx.new(|cx| InputState::new(window, cx));
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
            cx.subscribe_in(&name_draft, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_rename(cx);
                }
            }),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            rel,
            selected,
            multi,
            query,
            editing: String::new(),
            name_draft,
            placeholder: "",
            _subs: subs,
        }
    }

    /// New props from the value editor.
    pub(crate) fn set_props(
        &mut self,
        rel: RelationDef,
        selected: Vec<String>,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        if self.rel != rel || self.selected != selected || self.multi != multi {
            self.rel = rel;
            self.selected = selected;
            self.multi = multi;
            cx.notify();
        }
    }

    /// The relation as the store has it now (options change under us).
    fn rel(&self, cx: &App) -> RelationDef {
        Store::global(cx)
            .read(cx)
            .relations
            .iter()
            .find(|r| r.id == self.rel.id)
            .cloned()
            .unwrap_or_else(|| self.rel.clone())
    }

    fn q(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_lowercase()
    }

    fn matches(&self, rel: &RelationDef, cx: &App) -> Vec<RelationOption> {
        let q = self.q(cx);
        rel.options
            .iter()
            .filter(|o| q.is_empty() || o.text.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn can_create(&self, rel: &RelationDef, cx: &App) -> bool {
        let q = self.q(cx);
        !q.is_empty() && !rel.options.iter().any(|o| o.text.to_lowercase() == q)
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rel = self.rel(cx);
        if let Some(first) = self.matches(&rel, cx).into_iter().next() {
            cx.emit(OptionEvent::Pick(first.text));
        } else if self.can_create(&rel, cx) {
            self.create(window, cx);
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rel = self.rel(cx);
        let text = self.query.read(cx).value().trim().to_string();
        let task = add_option(&rel, &text, cx);
        cx.spawn_in(window, async move |this, cx| match task.await {
            Ok(opt) => {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.query.update(cx, |s, cx| s.set_value("", window, cx));
                    cx.emit(OptionEvent::Pick(opt.text));
                });
            }
            Err(error) => log::error!("add option: {error:#}"),
        })
        .detach();
    }

    fn open_edit(&mut self, opt: &RelationOption, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = if self.editing == opt.id {
            String::new()
        } else {
            opt.id.clone()
        };
        self.name_draft
            .update(cx, |s, cx| s.set_value(opt.text.clone(), window, cx));
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_empty() {
            return;
        }
        let id = std::mem::take(&mut self.editing);
        let rel = self.rel(cx);
        let next = self.name_draft.read(cx).value().to_string();
        rename_option(&rel, &id, &next, cx).detach_and_log_err(cx);
        cx.notify();
    }
}

impl Render for OptionPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rel = self.rel(cx);
        let placeholder = if rel.options.is_empty() {
            "Type to create an option…"
        } else {
            "Filter or create…"
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.query
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let matches = self.matches(&rel, cx);
        let can_create = self.can_create(&rel, cx);
        let raw = self.query.read(cx).value().trim().to_string();
        let mut list = div().flex().flex_col().gap(px(2.)).min_w(px(220.));
        list = list.child(div().mb(px(2.)).child(field(
            &self.query,
            FieldSkin::Filter,
            window,
            cx,
        )));
        if can_create {
            list = list.child(
                div()
                    .id("create")
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .text_color(theme::accent())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hl_med()))
                    .on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))
                    .child(format!("＋ Create option “{raw}”")),
            );
        }
        for opt in &matches {
            let on = self.selected.contains(&opt.text);
            let text = opt.text.clone();
            let o = opt.clone();
            let group: SharedString = format!("opt-{}", opt.id).into();
            list = list.child(
                div()
                    .id(SharedString::from(format!("wrap-{}", opt.id)))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .id(SharedString::from(format!("row-{}", opt.id)))
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(6.))
                            .py(px(4.))
                            .rounded(theme::radius::sm())
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hl_med()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OptionEvent::Pick(text.clone()))
                            }))
                            .child(TagPill::new(opt.text.clone(), opt.color.clone()))
                            .when(on, |d| {
                                d.child(div().ml_auto().text_color(theme::muted()).child("✓"))
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("more-{}", opt.id)))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(theme::radius::xs())
                            .text_color(theme::muted())
                            .opacity(0.5)
                            .group_hover(group, |s| s.opacity(1.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hl_med()).text_color(theme::fg()))
                            .tooltip(tip("Edit option"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_edit(&o, window, cx)
                            }))
                            .child("⋯"),
                    ),
            );
            if self.editing == opt.id {
                let mut palette = div().flex().flex_wrap().gap(px(4.));
                for c in TAG_COLORS {
                    let active = opt.color == c.name;
                    let rel2 = rel.clone();
                    let oid = opt.id.clone();
                    palette = palette.child(
                        div()
                            .id(c.name)
                            .size(px(18.))
                            .rounded_full()
                            .bg(c.hex())
                            .border_2()
                            .border_color(if active {
                                theme::fg()
                            } else {
                                transparent_black()
                            })
                            .cursor_pointer()
                            .tooltip(tip(c.name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.editing.clear();
                                set_option_color(&rel2, &oid, c.name, cx).detach_and_log_err(cx);
                                cx.notify();
                            })),
                    );
                }
                let rel3 = rel.clone();
                let oid = opt.id.clone();
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .pt(px(6.))
                        .px(px(8.))
                        .pb(px(8.))
                        .border_1()
                        .border_color(theme::border())
                        .rounded(theme::radius::md())
                        .mt(px(2.))
                        .mb(px(4.))
                        .child(field(&self.name_draft, FieldSkin::Filter, window, cx))
                        .child(palette)
                        .child(
                            div()
                                .id("delete")
                                .pt(px(2.))
                                .text_size(px(12.))
                                .text_color(rgb(0xe05555))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.editing.clear();
                                    delete_option(&rel3, &oid, cx).detach_and_log_err(cx);
                                    cx.notify();
                                }))
                                .child("Delete option"),
                        ),
                );
            }
        }
        if matches.is_empty() && !can_create {
            list = list.child(
                empty_note("No options yet — type a name.")
                    .px(px(6.))
                    .py(px(2.))
                    .text_size(px(12.)),
            );
        }
        if !self.multi && !self.selected.is_empty() {
            let first = self.selected[0].clone();
            list = list.child(
                div()
                    .id("clear")
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hl_med()))
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(OptionEvent::Pick(first.clone()))),
                    )
                    .child("Clear"),
            );
        }
        list
    }
}
