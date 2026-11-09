//! Anytype's option list + edit (`OptionPicker.svelte`): a filter box over
//! the property's options; typing an unknown name offers
//! `Create option "<name>"` (random palette color); every row has a ⋯ edit
//! affordance opening name + the 10-swatch palette + Delete option. Select
//! (status) picks one; multi-select (tag) toggles.

use std::rc::Rc;

use roostr_core::options::{add_option, delete_option, rename_option, set_option_color};
use roostr_core::{RelationDef, RelationOption, State};
use roostr_ui::tags::TAG_COLORS;
use roostr_ui::{Button, Glyph, Input, InputState, ListItem, TagPill, Theme, glyph, radius, ty};
use rune::{Color, Cursor, El, col, el, row, text};

use crate::widgets::{FieldSkin, Lens, empty_note, field, tip};

/// The "Delete option" red.
const DELETE_RED: u32 = 0xe05555;

/// A picker's own state (in the value editor's slot entry).
#[derive(Default)]
pub(crate) struct OptionPickerState {
    query: InputState,
    /// Option id with the edit panel open.
    editing: String,
    name_draft: InputState,
    /// The relation the open edit panel renames in.
    edit_rel: Option<RelationDef>,
    /// The name box had focus at the last poll (its blur commits the rename).
    name_had_focus: bool,
}

/// A rename the name box's blur asks for.
pub(crate) struct Rename {
    rel: RelationDef,
    id: String,
    next: String,
}

impl OptionPickerState {
    /// Called every poll: the name box just lost focus → its rename (and the
    /// panel closes), as GPUI's `InputEvent::Blur` did.
    pub(crate) fn blurred_rename(&mut self) -> Option<Rename> {
        let focused = self.name_draft.is_focused();
        let blurred = self.name_had_focus && !focused;
        self.name_had_focus = focused;
        if blurred { self.take_rename() } else { None }
    }

    fn take_rename(&mut self) -> Option<Rename> {
        if self.editing.is_empty() {
            return None;
        }
        let id = std::mem::take(&mut self.editing);
        let rel = self.edit_rel.take()?;
        Some(Rename {
            rel,
            id,
            next: self.name_draft.value().to_string(),
        })
    }
}

/// Runs a rename against the relation as the store has it now.
pub(crate) fn rename(s: &mut State, r: Rename) {
    let rel = current(s, &r.rel);
    rename_option(&rel, &r.id, &r.next, s).detach_and_log_err(&s.rt);
}

/// The relation as the store has it now (options change under us).
fn current(s: &State, rel: &RelationDef) -> RelationDef {
    s.store
        .relations
        .iter()
        .find(|r| r.id == rel.id)
        .cloned()
        .unwrap_or_else(|| rel.clone())
}

fn q(st: &OptionPickerState) -> String {
    st.query.value().trim().to_lowercase()
}

fn matches(rel: &RelationDef, q: &str) -> Vec<RelationOption> {
    rel.options
        .iter()
        .filter(|o| q.is_empty() || o.text.to_lowercase().contains(q))
        .cloned()
        .collect()
}

fn can_create(rel: &RelationDef, q: &str) -> bool {
    !q.is_empty() && !rel.options.iter().any(|o| o.text.to_lowercase() == q)
}

/// What the picker hands back to its value editor.
#[derive(Clone)]
pub(crate) struct OptionHooks {
    pub(crate) lens: Lens<OptionPickerState>,
    /// The editor still exists (an async create lands only then).
    pub(crate) alive: Rc<dyn Fn(&State) -> bool>,
    /// Toggle / select this option text.
    pub(crate) on_pick: Rc<dyn Fn(&mut State, String)>,
}

fn create(s: &mut State, rel: &RelationDef, hooks: &OptionHooks) {
    let rel = current(s, rel);
    let text = (hooks.lens)(s).query.value().trim().to_string();
    let task = add_option(&rel, &text, s);
    let hooks = hooks.clone();
    s.rt.spawn(async move |cx| match task.await {
        Ok(opt) => cx.update(move |s| {
            if !(hooks.alive)(s) {
                return;
            }
            (hooks.lens)(s).query.clear();
            (hooks.on_pick)(s, opt.text);
        }),
        Err(error) => log::error!("add option: {error:#}"),
    })
    .detach();
}

fn enter(s: &mut State, rel: &RelationDef, hooks: &OptionHooks) {
    let rel = current(s, rel);
    let q = q((hooks.lens)(s));
    if let Some(first) = matches(&rel, &q).into_iter().next() {
        (hooks.on_pick)(s, first.text);
    } else if can_create(&rel, &q) {
        create(s, &rel, hooks);
    }
}

fn open_edit(s: &mut State, rel: &RelationDef, opt: &RelationOption, hooks: &OptionHooks) {
    let st = (hooks.lens)(s);
    if st.editing == opt.id {
        st.editing.clear();
        st.edit_rel = None;
    } else {
        st.editing = opt.id.clone();
        st.edit_rel = Some(rel.clone());
    }
    st.name_draft.set_value(opt.text.clone());
}

/// The option list (min 220 wide): filter box, create row, options with
/// their edit panels, the empty note and (status) Clear.
#[allow(clippy::too_many_arguments)]
pub(crate) fn option_picker(
    s: &State,
    t: &Theme,
    rel: &RelationDef,
    selected: &[String],
    multi: bool,
    st: &OptionPickerState,
    hooks: OptionHooks,
) -> El {
    let rel = current(s, rel);
    let placeholder = if rel.options.is_empty() {
        "Type to create an option…"
    } else {
        "Filter or create…"
    };
    let q = q(st);
    let found = matches(&rel, &q);
    let creatable = can_create(&rel, &q);
    let raw = st.query.value().trim().to_string();

    let query = {
        let lens = hooks.lens.clone();
        let (r, h) = (rel.clone(), hooks.clone());
        Input::new("option-query", &st.query)
            .placeholder(placeholder)
            .on_edit(move |s: &mut State| &mut lens(s).query)
            .on_submit(move |s: &mut State, _| enter(s, &r, &h))
    };
    let mut list = col().gap(2.0).min_w(220.0).child(el().mb(2.0).child(field(
        t,
        query,
        st.query.is_focused(),
        FieldSkin::Filter,
    )));
    if creatable {
        let (r, h) = (rel.clone(), hooks.clone());
        list = list.child(
            ListItem::new("create")
                .px(6.0)
                .py(4.0)
                .radius(radius::SM)
                .text_size(13.0)
                .text_color(t.accent)
                .pointer()
                .label(format!("＋ Create option “{raw}”"))
                .on_press(move |s: &mut State| create(s, &r, &h))
                .build(t),
        );
    }
    for opt in &found {
        let on = selected.contains(&opt.text);
        let mut line = row()
            .grow(1.0)
            .center_y()
            .gap(8.0)
            .child(TagPill::new(opt.text.clone(), opt.color.clone()).build(t));
        if on {
            line = line
                .child(el().grow(1.0))
                .child(glyph(Glyph::Check, 13.0).color(t.muted));
        }
        let pick = {
            let (text, h) = (opt.text.clone(), hooks.clone());
            move |s: &mut State| (h.on_pick)(s, text.clone())
        };
        let edit = {
            let (r, o, h) = (rel.clone(), opt.clone(), hooks.clone());
            move |s: &mut State| open_edit(s, &r, &o, &h)
        };
        list = list.child(
            row()
                .key(format!("wrap-{}", opt.id))
                .hover_group()
                .items_center()
                .gap(2.0)
                .child(
                    ListItem::new(format!("row-{}", opt.id))
                        .grow()
                        .px(6.0)
                        .py(4.0)
                        .radius(radius::SM)
                        .text_size(13.0)
                        .pointer()
                        .child(line)
                        .on_press(pick)
                        .build(t),
                )
                .child(
                    Button::new(format!("more-{}", opt.id))
                        .ghost()
                        .xsmall()
                        .icon(Glyph::Ellipsis)
                        .text_color(t.muted)
                        .tooltip("Edit option")
                        .on_press(edit)
                        .build(t)
                        .group_opacity(0.5, 1.0),
                ),
        );
        if st.editing == opt.id {
            list = list.child(edit_panel(t, &rel, opt, st, &hooks));
        }
    }
    if found.is_empty() && !creatable {
        list = list.child(el().px(6.0).py(2.0).child(empty_note(
            t,
            "No options yet — type a name.",
            12.0,
        )));
    }
    if !multi && let Some(first) = selected.first() {
        let (first, h) = (first.clone(), hooks.clone());
        list = list.child(
            ListItem::new("clear")
                .px(6.0)
                .py(4.0)
                .radius(radius::SM)
                .text_size(13.0)
                .text_color(t.muted)
                .pointer()
                .label("Clear")
                .on_press(move |s: &mut State| (h.on_pick)(s, first.clone()))
                .build(t),
        );
    }
    list
}

/// Name + the 10-swatch palette + Delete option, under the edited row.
fn edit_panel(
    t: &Theme,
    rel: &RelationDef,
    opt: &RelationOption,
    st: &OptionPickerState,
    hooks: &OptionHooks,
) -> El {
    let mut palette = row().flex_wrap().gap(4.0);
    for c in TAG_COLORS {
        let active = opt.color == c.name;
        let (r, oid, lens) = (rel.clone(), opt.id.clone(), hooks.lens.clone());
        let name = c.name;
        palette = palette.child(tip(
            el().key(name)
                .size(18.0, 18.0)
                .radius(9.0)
                .fill(c.hex())
                .border(2.0, if active { t.fg } else { Color::TRANSPARENT })
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut State| {
                    let st = lens(s);
                    st.editing.clear();
                    st.edit_rel = None;
                    let rel = current(s, &r);
                    set_option_color(&rel, &oid, name, s).detach_and_log_err(&s.rt);
                }),
            name,
        ));
    }
    let name = {
        let lens = hooks.lens.clone();
        let commit = hooks.lens.clone();
        Input::new("option-name", &st.name_draft)
            .on_edit(move |s: &mut State| &mut lens(s).name_draft)
            .on_submit(move |s: &mut State, _| {
                if let Some(r) = commit(s).take_rename() {
                    rename(s, r);
                }
            })
    };
    let delete = {
        let (r, oid, lens) = (rel.clone(), opt.id.clone(), hooks.lens.clone());
        Button::new("delete")
            .ghost()
            .xsmall()
            .px(2.0)
            .text_color(roostr_ui::color::rgb(DELETE_RED))
            .child(
                text("Delete option")
                    .font(ty::ui(12.0))
                    .color(roostr_ui::color::rgb(DELETE_RED)),
            )
            .on_press(move |s: &mut State| {
                let st = lens(s);
                st.editing.clear();
                st.edit_rel = None;
                let rel = current(s, &r);
                delete_option(&rel, &oid, s).detach_and_log_err(&s.rt);
            })
            .build(t)
            .self_start()
    };
    col()
        .key(format!("edit-{}", opt.id))
        .gap(6.0)
        .pt(6.0)
        .px(8.0)
        .pb(8.0)
        .border(1.0, t.border)
        .radius(radius::MD)
        .mt(2.0)
        .mb(4.0)
        .child(field(
            t,
            name,
            st.name_draft.is_focused(),
            FieldSkin::Filter,
        ))
        .child(palette)
        .child(delete)
}
