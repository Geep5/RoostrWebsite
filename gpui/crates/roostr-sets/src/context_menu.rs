//! Right-click menu for records in a view (RowContextMenu.svelte): open,
//! open in new tab, change type, edit properties, add to / remove from a
//! collection, move to bin. Bulk by construction: it acts on every id given.

use std::collections::HashMap;

use roostr_core::nav::{self, Route};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::protected::blockers_for;
use roostr_core::relations::{is_reserved_key, space_relations};
use roostr_core::{Fields, Object, RelationDef, State, Value, api, field_str};
use roostr_properties::PropertyValue;
use roostr_rt::Task;
use roostr_ui::overlay::{self, Placement, scrim};
use roostr_ui::ty::{self, Weights};
use roostr_ui::{Glyph, Input, InputState, Theme, glyph, radius, rems, shadow};
use rune::{Cursor, Cx, El, Key, KeyEvent, PointerPress, ScrollSpec, Tx, col, el, row, text};

use crate::{DataView, common, config, dv, dv_mut};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fly {
    Types,
    Cols,
    Props,
}

/// Multi-value formats merge across records; everything else overwrites.
fn is_merge(rel: &RelationDef) -> bool {
    rel.format == "tag" || rel.format == "object"
}

fn list_of(fields: Option<&Fields>, key: &str) -> Vec<String> {
    fields
        .and_then(|f| f.get(key))
        .map(strings)
        .unwrap_or_default()
}

/// The non-empty string items of a value.
fn strings(v: &Value) -> Vec<String> {
    v.items()
        .iter()
        .filter_map(|i| i.string_value.clone())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The open row menu of one data view (`DataView.menu`).
pub(crate) struct RowMenu {
    /// The window point it opened at (its top-left corner).
    at: (f32, f32),
    ids: Vec<String>,
    space_id: String,
    /// Collections only: "Remove from collection" severs membership.
    collection: Option<String>,
    /// The view's visible relation keys head the Edit-properties list.
    relation_keys: Vec<String>,
    /// Row fields the host already holds.
    fields_of: HashMap<String, Fields>,
    fly: Option<Fly>,
    type_filter: InputState,
    prop_filter: InputState,
    edit_key: String,
    edit_value: Option<Value>,
    blocked: HashMap<String, String>,
}

impl RowMenu {
    fn suffix(&self) -> String {
        if self.ids.len() > 1 {
            format!(" ({})", self.ids.len())
        } else {
            String::new()
        }
    }

    fn deletable(&self) -> Vec<String> {
        self.ids
            .iter()
            .filter(|id| !self.blocked.contains_key(*id))
            .cloned()
            .collect()
    }
}

fn menu_of<'a>(s: &'a State, id: &str) -> Option<&'a RowMenu> {
    dv(s, id).and_then(|v| v.menu.as_ref())
}

fn menu_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut RowMenu> {
    dv_mut(s, id).and_then(|v| v.menu.as_mut())
}

/// Open the menu at window point `at` for `targets`, the protected ones
/// loading behind it.
pub(crate) fn open(
    s: &mut State,
    id: &str,
    targets: Vec<String>,
    at: (f32, f32),
    fields_of: HashMap<String, Fields>,
    relation_keys: Vec<String>,
) {
    let space_id = s
        .store
        .object(id)
        .map(|o| field_str(&o.fields, "channel").to_string())
        .unwrap_or_default();
    let Some(v) = dv_mut(s, id) else { return };
    let collection = v.collection_id();
    v.menu = Some(RowMenu {
        at,
        ids: targets.clone(),
        space_id,
        collection,
        relation_keys,
        fields_of,
        fly: None,
        type_filter: InputState::new(),
        prop_filter: InputState::new(),
        edit_key: String::new(),
        edit_value: None,
        blocked: HashMap::new(),
    });
    let task = blockers_for(&targets, s);
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Ok(blocked) = task.await {
            cx.update(|s| {
                // A menu opened since for other rows keeps its own answer.
                if let Some(m) = menu_mut(s, &view)
                    && m.ids == targets
                {
                    m.blocked = blocked;
                }
            });
        }
    })
    .detach();
}

/// Close the menu and clear the host's selection.
fn close(s: &mut State, id: &str) {
    if let Some(v) = dv_mut(s, id) {
        v.menu = None;
    }
    crate::table::clear_selection(s, id);
}

/// Records changed: reload once `task` is done.
fn reload_after(s: &mut State, id: &str, task: Task<()>) {
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        task.await;
        cx.update(|s| crate::reload(s, &view));
    })
    .detach();
}

/// Space types plus bundled ones, filtered by the search box: (key, label).
fn filtered_types(s: &State, m: &RowMenu) -> Vec<(String, String)> {
    let q = m.type_filter.value().trim().to_lowercase();
    let store = &s.store;
    store
        .types
        .iter()
        .filter(|t| t.space.is_empty() || t.space == m.space_id)
        .filter(|t| {
            q.is_empty()
                || (if t.name.is_empty() { &t.key } else { &t.name })
                    .to_lowercase()
                    .contains(&q)
        })
        .map(|t| {
            let icon = if t.icon.is_empty() {
                type_glyph(store, &t.key, &m.space_id)
            } else {
                t.icon.clone()
            };
            (
                t.key.clone(),
                format!(
                    "{} {}",
                    icon,
                    if t.name.is_empty() { &t.key } else { &t.name }
                ),
            )
        })
        .collect()
}

fn prop_rels(s: &State, m: &RowMenu) -> Vec<RelationDef> {
    let all: Vec<&RelationDef> = space_relations(&s.store.relations, &m.space_id)
        .into_iter()
        .filter(|r| !r.hidden && !r.read_only && !is_reserved_key(&r.key))
        .collect();
    let head: Vec<&RelationDef> = m
        .relation_keys
        .iter()
        .filter_map(|k| all.iter().find(|r| r.key == *k).copied())
        .collect();
    let rest = all
        .iter()
        .filter(|r| !head.iter().any(|h| h.id == r.id))
        .copied();
    head.iter().copied().chain(rest).cloned().collect()
}

fn open_fly(s: &mut State, id: &str, kind: Fly) {
    let Some(m) = menu_mut(s, id) else { return };
    if kind == Fly::Types {
        m.type_filter.clear();
    }
    if kind == Fly::Props {
        leave_edit(s, id);
    }
    let Some(m) = menu_mut(s, id) else { return };
    m.fly = if m.fly == Some(kind) {
        None
    } else {
        Some(kind)
    };
    if m.fly == Some(Fly::Types) {
        m.type_filter.focus();
    }
}

fn open_rows(s: &mut State, id: &str) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.ids.clone();
    close(s, id);
    // One object opens in place; a multi-selection opens each as a tab.
    if targets.len() == 1 {
        common::open_object(&targets[0], s);
        return;
    }
    for target in targets {
        nav::open_tab(Route::Object(target), true, s);
    }
}

fn open_tabs(s: &mut State, id: &str) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.ids.clone();
    close(s, id);
    for target in targets {
        common::open_object_tab(&target, s);
    }
}

fn remove(s: &mut State, id: &str) {
    let Some(m) = menu_of(s, id) else { return };
    let Some(collection) = m.collection.clone() else {
        return;
    };
    let targets = m.ids.clone();
    close(s, id);
    let task = common::remove_members(&collection, &targets, s);
    let done = s.rt.spawn(async move |_| {
        let _ = task.await;
    });
    reload_after(s, id, done);
}

fn bin(s: &mut State, id: &str) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.deletable();
    close(s, id);
    let done = s.rt.spawn(async move |cx| {
        for target in targets {
            let task = cx.update(|s| api::note::delete(&target, s));
            let _ = task.await;
        }
    });
    reload_after(s, id, done);
}

/// Retype in place: blocks and fields survive.
fn retype(s: &mut State, id: &str, type_key: String) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.ids.clone();
    close(s, id);
    let done = s.rt.spawn(async move |cx| {
        for target in targets {
            let task = cx.update(|s| api::note::set_type(&target, &type_key, s));
            let _ = task.await;
        }
    });
    reload_after(s, id, done);
}

fn add_to(s: &mut State, id: &str, collection_id: String) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.ids.clone();
    close(s, id);
    let fetch = s.store.fetch_object(&collection_id);
    let done = s.rt.spawn(async move |cx| {
        if let Ok(col) = fetch.await {
            let have = config::member_ids(&col.fields);
            let mut merged = have.clone();
            for target in targets {
                if !merged.contains(&target) {
                    merged.push(target);
                }
            }
            if merged.len() != have.len() {
                let task = cx.update(|s| common::set_members(&collection_id, &merged, s));
                let _ = task.await;
            }
        }
    });
    reload_after(s, id, done);
}

/// Fields of every target: the host's rows, else fetched.
type Pending = Vec<Result<Fields, Task<anyhow::Result<Object>>>>;

fn values_of(s: &State, m: &RowMenu) -> Pending {
    m.ids
        .iter()
        .map(|id| match m.fields_of.get(id) {
            Some(f) => Ok(f.clone()),
            None => Err(s.store.fetch_object(id)),
        })
        .collect()
}

async fn resolve(pending: Pending) -> Vec<Option<Fields>> {
    let mut out = Vec::with_capacity(pending.len());
    for t in pending {
        out.push(match t {
            Ok(f) => Some(f),
            Err(task) => task.await.ok().map(|o| o.fields),
        });
    }
    out
}

/// Back to the property list: the open editor goes with its state.
fn leave_edit(s: &mut State, id: &str) {
    let Some(m) = menu_mut(s, id) else { return };
    let key = std::mem::take(&mut m.edit_key);
    m.edit_value = None;
    if !key.is_empty() {
        roostr_properties::value::forget(s, &edit_key(id, &key));
    }
}

fn edit_key(id: &str, key: &str) -> String {
    format!("{id}/ctx-edit-{key}")
}

/// Open one property: the initial value is the cross-record intersection.
fn open_prop(s: &mut State, id: &str, key: String) {
    leave_edit(s, id);
    let Some(m) = menu_mut(s, id) else { return };
    m.edit_key = key.clone();
    let Some(m) = menu_of(s, id) else { return };
    let Some(rel) = prop_rels(s, m).into_iter().find(|r| r.key == key) else {
        return;
    };
    let pending = values_of(s, m);
    let ids = m.ids.clone();
    let view = id.to_string();
    s.rt.spawn(async move |cx| {
        let all = resolve(pending).await;
        let initial = if is_merge(&rel) {
            let mut inter = list_of(all.first().and_then(Option::as_ref), &key);
            for f in all.iter().skip(1) {
                let l = list_of(f.as_ref(), &key);
                inter.retain(|v| l.contains(v));
            }
            Some(Value::values(
                inter.into_iter().map(Value::string).collect(),
            ))
        } else {
            let first = all
                .first()
                .and_then(|f| f.as_ref().and_then(|f| f.get(&key).cloned()));
            if all
                .iter()
                .skip(1)
                .all(|f| f.as_ref().and_then(|f| f.get(&key).cloned()) == first)
            {
                first
            } else {
                None
            }
        };
        cx.update(|s| {
            if let Some(m) = menu_mut(s, &view)
                && m.ids == ids
                && m.edit_key == key
            {
                m.edit_value = initial;
            }
        });
    })
    .detach();
}

fn apply_prop(s: &mut State, id: &str, rel: RelationDef, v: Value) {
    let Some(m) = menu_of(s, id) else { return };
    let targets = m.ids.clone();
    let base = m.edit_value.as_ref().map(strings).unwrap_or_default();
    let pending = values_of(s, m);
    // Back to the property list; the menu stays open for the next one.
    leave_edit(s, id);
    let done = s.rt.spawn(async move |cx| {
        if is_merge(&rel) {
            let next = strings(&v);
            let added: Vec<String> = next.iter().filter(|x| !base.contains(x)).cloned().collect();
            let removed: Vec<String> = base.iter().filter(|x| !next.contains(x)).cloned().collect();
            let all = resolve(pending).await;
            for (target, fields) in targets.iter().zip(all) {
                let cur = list_of(fields.as_ref(), &rel.key);
                let mut merged: Vec<String> = cur
                    .iter()
                    .filter(|x| !removed.contains(x))
                    .cloned()
                    .collect();
                merged.extend(added.iter().filter(|x| !cur.contains(x)).cloned());
                let value = Value::values(merged.into_iter().map(Value::string).collect());
                let task = cx.update(|s| api::note::set_field(target, &rel.key, value, s));
                let _ = task.await;
            }
        } else {
            for target in &targets {
                let task = cx.update(|s| api::note::set_field(target, &rel.key, v.clone(), s));
                let _ = task.await;
            }
        }
    });
    reload_after(s, id, done);
}

// ── Drawing ──────────────────────────────────────────────────────

/// A flyout row (`.ctx-item`), the kit list row: gap 9, padding 7 × 10,
/// radius 6, 13px fg, the kit list hover.
fn menu_item(t: &Theme, key: String) -> El {
    row()
        .key(key)
        .center_y()
        .gap(9.0)
        .w_full()
        .px(10.0)
        .py(7.0)
        .radius(6.0)
        .cursor(Cursor::Pointer)
        .focusable(true)
        .no_focus_ring()
        .hover(t.kit.list_hover)
        .focus_highlight(t.kit.list_hover)
}

fn item_label(t: &Theme, s: impl Into<String>) -> El {
    text(s.into()).font(ty::ui(13.0)).color(t.fg).truncate()
}

/// A flyout's "nothing here" line, the kit empty state.
fn none_row(t: &Theme, line: &str) -> El {
    common::empty_state(t, line, 12.0).px(8.0).py(4.0)
}

/// One top-level row's content (`.ctx-item`: 16px icon, 9px gap, 13px label,
/// 7×10 padding). It spans the kit row edge to edge (the kit pads 0.5 rem)
/// so the open-flyout highlight covers the whole row.
fn row_body(t: &Theme, g: Glyph, label: String, ink: Option<rune::Color>) -> El {
    let (fg, lit) = match ink {
        Some(c) => (c, c),
        None => (t.kit.foreground, t.kit.accent_foreground),
    };
    row()
        .grow(1.0)
        .shrink()
        .center_y()
        .gap(9.0)
        .mx(-rems(0.5))
        .px(10.0)
        .py(7.0)
        .radius(6.0)
        .child(glyph(g, 16.0).color(fg).group_color(lit))
        .child(
            text(label)
                .font(ty::ui(13.0))
                .color(fg)
                .group_color(lit)
                .truncate()
                .grow(1.0)
                .shrink(),
        )
}

/// A kit menu row (`PopupMenuItem::element`) around `content`: at least an
/// item tall, the accent highlight shared by pointer and keyboard.
fn kit_row(t: &Theme, key: String, content: El, disabled: bool) -> El {
    let e = row()
        .key(key)
        .role("menuitem")
        .px(rems(0.5))
        .gap(rems(0.25))
        .radius(radius::KIT.min(8.0))
        .center_y()
        .min_h(rems(1.625))
        .child(content);
    if disabled {
        return e.disabled(true);
    }
    e.focusable(true)
        .no_focus_ring()
        .hover_group_focus()
        .focus_highlight(t.kit.accent)
        .cursor(Cursor::Pointer)
}

/// The kit separator between groups.
fn kit_separator(t: &Theme) -> El {
    el().h(2.0)
        .my(rems(0.125))
        .mx(-rems(0.25))
        .fill(t.kit.border)
        .a11y_hidden()
}

/// The top-level rows: Open, Open in new tab, the three flyout rows, Remove
/// from collection, a separator, then Move to bin or the locked "Can't
/// delete" line.
fn menu_rows(t: &Theme, id: &str, m: &RowMenu) -> El {
    let suffix = m.suffix();
    let k = |name: &str| format!("{id}/ctx-{name}");
    let act = |f: fn(&mut State, &str)| {
        let id = id.to_string();
        move |s: &mut State| f(s, &id)
    };
    let fly_row = |g: Glyph, label: String, kind: Fly| {
        let open = m.fly == Some(kind);
        let mut body =
            row_body(t, g, label, None).child(glyph(Glyph::ChevronRight, 14.0).color(t.muted));
        if open {
            body = body.fill(t.hover);
        }
        let view = id.to_string();
        // A flyout row toggles its flyout; the menu stays open.
        kit_row(t, fly_key(id, kind), body, false)
            .expanded(open)
            .on_press(move |s: &mut State| open_fly(s, &view, kind))
    };

    let mut rows = col()
        .key("items")
        .items_stretch()
        .padding(rems(0.25))
        .gap(rems(0.125))
        .min_w(200.0)
        .max_w(rems(31.25))
        .child(
            kit_row(
                t,
                k("open"),
                row_body(t, Glyph::ArrowUpRight, format!("Open{suffix}"), None),
                false,
            )
            .on_press(act(open_rows)),
        )
        .child(
            kit_row(
                t,
                k("open-tab"),
                row_body(
                    t,
                    Glyph::ExternalLink,
                    format!("Open in new tab{suffix}"),
                    None,
                ),
                false,
            )
            .on_press(act(open_tabs)),
        )
        .child(fly_row(
            Glyph::ArrowLeftRight,
            format!("Change type{suffix}"),
            Fly::Types,
        ))
        .child(fly_row(
            Glyph::SlidersHorizontal,
            format!("Edit properties{suffix}"),
            Fly::Props,
        ))
        .child(fly_row(
            Glyph::FolderPlus,
            format!("Add to collection{suffix}"),
            Fly::Cols,
        ));
    if m.collection.is_some() {
        rows = rows.child(
            kit_row(
                t,
                k("remove"),
                row_body(
                    t,
                    Glyph::FolderMinus,
                    format!("Remove from collection{suffix}"),
                    None,
                ),
                false,
            )
            .on_press(act(remove)),
        );
    }
    rows = rows.child(kit_separator(t));
    let deletable = m.deletable();
    if deletable.is_empty() {
        let why = format!(
            "Can't delete: {}",
            m.blocked.values().next().cloned().unwrap_or_default()
        );
        let locked = roostr_ui::tooltip(
            row()
                .key(k("locked"))
                .grow(1.0)
                .shrink()
                .center_y()
                .gap(9.0)
                .mx(-rems(0.5))
                .px(10.0)
                .py(7.0)
                .child(glyph(Glyph::Lock, 16.0).color(t.muted))
                .child(text(why).font(ty::ui(12.0)).color(t.muted).grow(1.0)),
            "Types and properties Roostr or your queries rely on can't be deleted",
        );
        rows.child(kit_row(t, k("locked-row"), locked, true))
    } else {
        let label = format!(
            "Move to bin{}{}",
            if deletable.len() > 1 {
                format!(" ({})", deletable.len())
            } else {
                String::new()
            },
            if m.blocked.is_empty() {
                String::new()
            } else {
                format!(" - {} protected kept", m.blocked.len())
            }
        );
        rows.child(
            kit_row(
                t,
                k("bin"),
                row_body(t, Glyph::Trash, label, Some(t.red)),
                false,
            )
            .on_press(act(bin)),
        )
    }
}

fn fly_key(id: &str, kind: Fly) -> String {
    let name = match kind {
        Fly::Types => "types",
        Fly::Props => "props",
        Fly::Cols => "cols",
    };
    format!("{id}/ctx-row-{name}")
}

/// A filter field atop a flyout (13px; its wrapper sets `mx 6, mt 4, mb 6`).
fn filter_field(
    key: String,
    state: &InputState,
    placeholder: &str,
    lens: impl for<'a> Fn(&'a mut State) -> &'a mut InputState + 'static,
) -> Input {
    Input::new(key, state)
        .placeholder(placeholder)
        .text_size(13.0)
        .on_edit(lens)
}

fn type_filter_lens(id: String) -> impl for<'a> Fn(&'a mut State) -> &'a mut InputState + 'static {
    move |s: &mut State| {
        &mut menu_mut(s, &id)
            .expect("row menu open while its field is edited")
            .type_filter
    }
}

fn prop_filter_lens(id: String) -> impl for<'a> Fn(&'a mut State) -> &'a mut InputState + 'static {
    move |s: &mut State| {
        &mut menu_mut(s, &id)
            .expect("row menu open while its field is edited")
            .prop_filter
    }
}

fn rel_label(r: &RelationDef) -> String {
    format!(
        "{}{}",
        r.icon_emoji
            .as_deref()
            .filter(|e| !e.is_empty())
            .map(|e| format!("{e} "))
            .unwrap_or_default(),
        if r.name.is_empty() { &r.key } else { &r.name }
    )
}

fn render_fly(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    id: &str,
    m: &RowMenu,
    kind: Fly,
) -> El {
    // `.ctx-fly`: max-height min(340px, 60vh) (content-box: + padding and border).
    let window_h = cx.layout_with("kit-root", |r| r[3]).unwrap_or(f32::MAX);
    let fly = col()
        .key(format!("{id}/ctx-fly-{}", kind as usize))
        .items_stretch()
        .w(250.0)
        .max_h((window_h * 0.6).min(340.0) + 10.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(10.0)
        .padding(4.0)
        // `.ctx-fly`'s shadow: `0 8 24 rgba(0,0,0,.4)`.
        .box_shadow(shadow::gpui(8.0, 24.0, 0.0, 0.4));
    let mut list = col().items_stretch();
    match kind {
        Fly::Types => {
            let view = id.to_string();
            let enter = id.to_string();
            let field = filter_field(
                format!("{id}/ctx-types-q"),
                &m.type_filter,
                "Search types...",
                type_filter_lens(id.to_string()),
            )
            .on_submit(move |s: &mut State, _: &mut Tx| {
                let first = menu_of(s, &enter)
                    .and_then(|m| filtered_types(s, m).into_iter().next())
                    .map(|t| t.0);
                if let Some(key) = first {
                    retype(s, &enter, key);
                }
            })
            // The field sees Escape first: it clears, then closes the flyout.
            .on_key(move |s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
                if ev.key != Key::Escape {
                    return false;
                }
                let Some(m) = menu_mut(s, &view) else {
                    return false;
                };
                if m.type_filter.value().is_empty() {
                    m.fly = None;
                    // Keep keyboard focus in the menu: the next Escape closes it.
                    tx.focus(fly_key(&view, Fly::Types), false);
                } else {
                    m.type_filter.clear();
                }
                true
            });
            let fly = fly.child(el().mx(6.0).mt(4.0).mb(6.0).child(field.build(t)));
            let types = filtered_types(s, m);
            if types.is_empty() {
                let q = m.type_filter.value().trim();
                list = list.child(none_row(t, &format!("No type matches “{q}”")));
            }
            for (key, label) in types {
                let view = id.to_string();
                list = list.child(
                    menu_item(t, format!("{id}/type-{key}"))
                        .child(item_label(t, label))
                        .on_press(move |s: &mut State| retype(s, &view, key.clone())),
                );
            }
            fly.child(list)
        }
        Fly::Cols => {
            let store = &s.store;
            let cols: Vec<(String, String)> = store
                .summaries
                .iter()
                .filter(|o| {
                    o.type_key == "collection"
                        && o.channel_id == m.space_id
                        && !m.ids.contains(&o.id)
                })
                .map(|o| {
                    let icon = object_icon(store, &o.icon, &o.type_key, &m.space_id);
                    (
                        o.id.clone(),
                        format!(
                            "{icon} {}",
                            if o.name.is_empty() {
                                "Untitled"
                            } else {
                                &o.name
                            }
                        ),
                    )
                })
                .collect();
            if cols.is_empty() {
                list = list.child(none_row(t, "No collections in this space"));
            }
            for (col_id, label) in cols {
                let view = id.to_string();
                list = list.child(
                    menu_item(t, format!("{id}/col-{col_id}"))
                        .child(item_label(t, label))
                        .on_press(move |s: &mut State| add_to(s, &view, col_id.clone())),
                );
            }
            fly.child(list)
        }
        Fly::Props => {
            let rels = prop_rels(s, m);
            if let Some(rel) = rels.iter().find(|r| r.key == m.edit_key).cloned() {
                let back = id.to_string();
                let save = id.to_string();
                let save_rel = rel.clone();
                let editor =
                    PropertyValue::new(edit_key(id, &rel.key), rel.clone(), m.edit_value.clone())
                        .auto_open(true)
                        .on_save(move |s: &mut State, v: Value| {
                            apply_prop(s, &save, save_rel.clone(), v)
                        })
                        .build(s, cx, t, layer);
                return fly
                    .child(
                        menu_item(t, format!("{id}/ctx-back"))
                            .child(glyph(Glyph::ChevronLeft, 14.0).color(t.fg))
                            .child(
                                text(rel_label(&rel))
                                    .font(ty::ui(13.0).semibold())
                                    .color(t.fg)
                                    .truncate(),
                            )
                            .on_press(move |s: &mut State| leave_edit(s, &back)),
                    )
                    .child(el().px(6.0).pt(2.0).pb(8.0).child(editor));
            }
            let mut fly = fly;
            if rels.len() > 6 {
                let field = filter_field(
                    format!("{id}/ctx-props-q"),
                    &m.prop_filter,
                    "Filter properties...",
                    prop_filter_lens(id.to_string()),
                );
                fly = fly.child(el().mx(6.0).mt(4.0).mb(6.0).child(field.build(t)));
            }
            let q = m.prop_filter.value().trim().to_lowercase();
            let filtered: Vec<RelationDef> = rels
                .into_iter()
                .filter(|r| {
                    q.is_empty()
                        || (if r.name.is_empty() { &r.key } else { &r.name })
                            .to_lowercase()
                            .contains(&q)
                })
                .collect();
            if filtered.is_empty() {
                list = list.child(none_row(t, "No properties"));
            }
            for r in filtered {
                let key = r.key.clone();
                let view = id.to_string();
                list = list.child(
                    menu_item(t, format!("{id}/prop-{key}"))
                        .child(item_label(t, rel_label(&r)))
                        .on_press(move |s: &mut State| open_prop(s, &view, key.clone())),
                );
            }
            fly.child(list)
        }
    }
}

/// The open menu, pushed into `layer`: a full-window modal holding the
/// click-away backdrop, the menu at its point and the open flyout beside its
/// row (flipped to the menu's left near the window edge). The flyout's own
/// popovers mount inside it so they stay live under the modal.
pub(crate) fn view(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>, v: &DataView) {
    let Some(m) = v.menu.as_ref() else { return };
    let id = v.id.as_str();
    let closer = |id: &str| {
        let id = id.to_string();
        move |s: &mut State| close(s, &id)
    };
    let right = closer(id);
    let backdrop = scrim(format!("{id}/ctx-backdrop"), closer(id))
        .on_secondary_press(move |s: &mut State, _: PointerPress, _: &mut Tx| right(s));
    let point = format!("{id}/ctx-point");
    let window_w = cx.layout_with("kit-root", |r| r[2]).unwrap_or(f32::MAX);
    let mut surface = col()
        .key(format!("{id}/ctx-menu"))
        .role("menu")
        .focus_group()
        .autofocus()
        .items_stretch()
        .fill(t.kit.popover)
        .radius(radius::KIT);
    for sh in shadow::kit_popover(t) {
        surface = surface.box_shadow(sh);
    }
    let surface = surface.child(menu_rows(t, id, m));
    let mut inner = Vec::new();
    let flyout = m.fly.map(|kind| {
        let content = render_fly(s, cx, t, &mut inner, id, m, kind);
        // Flush against the menu's edge (the kit row sits 0.25 rem inside it,
        // GPUI left 5 px), centred on its row; Rune flips it to the left
        // edge when it doesn't fit and keeps it 8 points inside the window.
        overlay::anchored(
            content,
            &fly_key(id, kind),
            Placement::Right,
            5.0 + rems(0.25),
        )
    });
    let modal = el()
        .key(format!("{id}/ctx"))
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .child(backdrop)
        .child(
            // Kept 230 in from the window's right edge, as GPUI placed it.
            el().key(point.clone())
                .absolute(m.at.0.min(window_w - 230.0), m.at.1)
                .size(0.0, 0.0),
        )
        .child(overlay::anchored(
            surface,
            &point,
            Placement::BottomStart,
            0.0,
        ))
        .children(flyout)
        .children(inner)
        .modal(false, closer(id));
    layer.push(modal);
}
