//! The sidebar (`aside.widgets`, `+layout.svelte:1376-1521`, CSS 1728-2414):
//! space head (settings link, search, New pill + menu), one card per pinned
//! object, then Recently edited, Types and Properties.

use roostr_core::nav::{Route, navigate};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::relations::{format_glyph, is_reserved_key, space_relations};
use roostr_core::types::{Fields, RelationDef, Space, Value, is_system_type};
use roostr_core::{State, TypeDef};
use roostr_ui::{Glyph, KitSize, Theme, glyph, motion, radius, spaced_text, ty, ty::Weights as _};
use rune::{Color, Cursor, Cx, El, col, el, row, text};

use crate::drag::{self, Axis};
use crate::shell::{
    Shell, builtin_mark, current_space, fill, fill_glyph, hover_fg, icon_text, obj_icon,
    open_object_id, or_untitled, space_id,
};
use crate::{list_motion, pin_collapse};

/// Write a task's `done` (a row's checkbox), then refresh the lists.
pub(crate) fn set_done(s: &mut State, id: &str, done: bool) {
    let task = roostr_core::api::note::set_field(id, "done", Value::bool(done), s);
    s.rt.spawn(async move |cx| {
        if task.await.is_ok() {
            cx.update(|s| s.store.refresh_all());
        }
    })
    .detach();
}

fn open(id: String) -> impl Fn(&mut State) + 'static {
    move |s: &mut State| navigate(Route::Object(id.clone()), s)
}

/// The sidebar card, `width` wide.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    sh: &Shell,
    width: f32,
    layer: &mut Vec<El>,
) -> El {
    let card = col()
        .key("sidebar")
        .ml(6.)
        .w(width)
        .h_full()
        .radius(radius::PANE)
        .fill(t.bg)
        .clip_rounded(radius::PANE);
    let Some(current) = current_space(s) else {
        return card;
    };
    let open_id = open_object_id(s);
    let mut list = col()
        .key("sidebar-list")
        .w_full()
        .px(8.)
        .pt(8.)
        .pb(10.)
        .gap(8.);
    // Lite: the vault or this space not ready yet, or the space loading.
    if let Some(notice) = roostr_core::lite_ui::ready_notice(&current.id, s) {
        list = list.child(el().px(8.).child(notice_row(t, notice, "sidebar-notice")));
    }
    for id in &current.pinned_ids {
        if let Some(card) = pinned_card(s, cx, t, sh, id, open_id.as_deref(), layer) {
            list = list.child(list_motion::pin_card(s, "pins", id, card));
        }
    }
    list = list
        .child(recent(s, t, sh, current, open_id.as_deref()))
        .child(types(s, t, sh, open_id.as_deref()))
        .child(props(s, t, sh, open_id.as_deref()));
    widget_ghost(s, t, layer);
    card.child(space_head(s, t, current)).child(
        col()
            .key("sidebar-scroll")
            .grow(1.)
            .min_h(0.)
            .w_full()
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(list_motion::pin_list(s, "pins", list)),
    )
}

/// A lite notice (`notice_row`): flex wrap, centred, gap 10, py 6, 12 pt
/// muted; its button an outline button running the action.
pub(crate) fn notice_row(t: &Theme, notice: roostr_core::lite_ui::Notice, key: &str) -> El {
    let mut line = row()
        .key(key.to_string())
        .flex_wrap()
        .items_center()
        .gap(10.)
        .py(6.)
        .child(text(notice.text).font(ty::ui(12.)).color(t.muted).wrap());
    if let Some(action) = notice.action {
        line = line.child(
            roostr_ui::outline_button(format!("{key}-action"), action.label())
                .on_press(move |s: &mut State| action.clone().run(s))
                .build(t),
        );
    }
    line
}

fn space_head(s: &State, t: &Theme, current: &Space) -> El {
    let icon = if current.icon.is_empty() {
        "◍".to_string()
    } else {
        current.icon.clone()
    };
    let head = fill("space-head", t.hl_med)
        .grow(1.)
        .min_w(0.)
        .px(6.)
        .py(4.)
        .radius(radius::SM)
        .justify_start()
        .gap(10.)
        .label(current.name.clone())
        .child(
            row()
                .size(22., 22.)
                .center()
                .radius(radius::SM)
                .clip_rounded(radius::SM)
                .child(icon_text(s, &icon, 22., ty::ui(17.))),
        )
        .child(
            text(current.name.clone())
                .font(ty::ui(15.).semibold().lh(22.))
                .color(t.fg)
                .truncate()
                .shrink()
                .min_w(0.),
        )
        .on_press(open(current.id.clone()));
    let head = motion::pressable(head);
    let search = fill("head-search", t.hl_med)
        .size(30., 30.)
        .radius(radius::MD)
        .label("Search")
        .child(fill_glyph(t, Glyph::Search, 17.))
        .on_press(crate::search::open);
    let search = motion::pressable(search);
    let create = fill("create-btn", t.hover)
        .h(30.)
        .px(7.)
        .radius(radius::PILL)
        .fill(t.hl_med)
        .gap(3.)
        .label("New object")
        .child(fill_glyph(t, Glyph::Compose, 15.))
        .child(fill_glyph(t, Glyph::ChevronDown, 11.))
        .on_press(crate::create_menu::toggle);
    let create = motion::pressable(create);
    row()
        .key("space-head-row")
        .w_full()
        .items_center()
        .gap(2.)
        .px(8.)
        .pt(10.)
        .pb(6.)
        .fill(t.bg)
        .child(roostr_ui::tooltip(head, "Space settings"))
        .child(roostr_ui::tooltip(search, "Search (⌘K)"))
        .child(roostr_ui::tooltip(create, "New object"))
}

fn flip_section(s: &mut State, key: &str) {
    let shell = s.slot::<Shell>();
    let collapsed = !shell.section_collapsed.get(key).copied().unwrap_or(false);
    shell.section_collapsed.insert(key.to_string(), collapsed);
    let map = shell.section_collapsed.clone();
    s.ui.set("section-collapsed", &map);
}

/// Pin `id`'s card: its fold arrow, its head (opens it; current: `hl-med`),
/// and its widget body while unfolded. It drags to reorder the pins.
fn pinned_card(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    sh: &Shell,
    id: &str,
    open_id: Option<&str>,
    layer: &mut Vec<El>,
) -> Option<El> {
    let p = s.store.summary(id)?;
    let collapsed = pin_collapse::collapsed(s, id);
    // Only this device's own fold moves (Svelte popped the body); one sync
    // brings in snaps.
    let own = pin_collapse::own(s, id);
    let icon = object_icon(&s.store, &p.icon, &p.type_key, s.nav.space());
    let name = or_untitled(&p.name);
    let is_current = open_id == Some(id);
    let body = (!collapsed).then(|| {
        // The body's column: the card's 8 pt padding and border inside the sidebar list.
        let width = sh.side_width - 6. - 16. - 18.;
        crate::pinned::widget(s, cx, t, id, width, layer)
    });
    let flip = id.to_string();
    let arrow = fill(format!("widget-collapse:{id}"), Color::TRANSPARENT)
        .w(16.)
        .h(24.)
        .label("Fold")
        .child(
            list_motion::arrow(!collapsed, 7., t.muted)
                .group_color(t.fg)
                .group_opacity(0.6, 1.0),
        )
        .on_press(move |s: &mut State| pin_collapse::toggle(s, &flip));
    let arrow = motion::pressable(arrow);
    let mut head = fill(format!("widget-head:{id}"), t.hl_med)
        .grow(1.)
        .min_w(0.)
        .h(28.)
        .pl(2.)
        .pr(8.)
        .radius(radius::SM)
        .justify_start()
        .gap(6.)
        .label(name.clone())
        .child(obj_icon(s, t, &icon))
        .child(
            text(name.clone())
                .font(ty::ui(14.).semibold().lh(22.))
                .color(t.fg)
                .truncate()
                .shrink()
                .min_w(0.),
        )
        .on_press(open(id.to_string()));
    if is_current {
        head = head.fill(t.hl_med);
    }
    let head = motion::pressable_row(head);
    let boxed = col()
        .key(format!("widget-box:{id}"))
        .w_full()
        .fill(t.panel)
        .border(1., t.border)
        .border_layout()
        .radius(radius::MENU)
        .padding(8.)
        .child(row().w_full().items_center().child(arrow).child(head))
        .child(list_motion::fold(
            format!("widget-fold:{id}"),
            own,
            true,
            body,
        ));
    let mut card = col().key(format!("widget:{id}")).w_full().child(boxed);
    // A held card shrinks; the one being dragged doesn't (its ghost follows the pointer).
    if drag::dragging(s, "widgets") == Some(id) {
        card = card.opacity(0.4);
    } else {
        card = motion::pressable(card);
    }
    if let Some(bottom) = drag::over(s, "widgets", id) {
        let edge = if bottom {
            (None, Some(-5.))
        } else {
            (Some(-5.), None)
        };
        card = card.child(
            el().pin(Some(12.), edge.0, Some(12.), edge.1)
                .h(2.)
                .radius(2.)
                .fill(t.accent),
        );
    }
    Some(drag::item(card, "widgets", id, Axis::Y, drop_widget))
}

/// A pinned widget dropped next to another: this device's reorder glides.
fn drop_widget(s: &mut State, dragged: &str, target: &str, bottom: bool) {
    let Some(space) = current_space(s).cloned() else {
        return;
    };
    list_motion::arm(s, "pins");
    roostr_core::pins::move_pin(&space, dragged, target, bottom, s).detach_and_log_err(&s.rt);
}

/// The floating copy of a dragged widget head.
fn widget_ghost(s: &State, t: &Theme, layer: &mut Vec<El>) {
    let Some(id) = drag::dragging(s, "widgets") else {
        return;
    };
    let Some(p) = s.store.summary(id) else {
        return;
    };
    let icon = object_icon(&s.store, &p.icon, &p.type_key, s.nav.space());
    let ghost = row()
        .items_center()
        .gap(6.)
        .px(8.)
        .h(28.)
        .radius(radius::MENU)
        .fill(t.panel)
        .border(1., t.border)
        .opacity(0.8)
        .child(obj_icon(s, t, &icon))
        .child(
            text(or_untitled(&p.name))
                .font(ty::ui(14.).semibold())
                .color(t.fg),
        );
    drag::ghost(s, "widgets", ghost, layer);
}

/// A sidebar section: its folding name (`.section-name`, muted → fg on
/// hover over 0.15 s), an optional ＋, and its rows on a panel card.
fn section(
    t: &Theme,
    sh: &Shell,
    key: &'static str,
    label: &'static str,
    add: Option<El>,
    rows: Vec<El>,
) -> El {
    let open = !sh.section_collapsed.get(key).copied().unwrap_or(false);
    // A long section only fades: sliding it would lay out every row each frame.
    let slide = rows.len() <= 30;
    let body = open.then(|| {
        rows.into_iter().enumerate().fold(
            col()
                .key(format!("section-box:{key}"))
                .w_full()
                .fill(t.panel)
                .border(1., t.border)
                .border_layout()
                .radius(radius::MENU)
                .px(8.)
                .py(4.),
            |list, (i, r)| {
                let r = if i > 0 {
                    el().w_full()
                        .border_sides([1., 0., 0., 0.], t.border)
                        .border_layout()
                        .child(r)
                } else {
                    r
                };
                list.child(r)
            },
        )
    });
    let name = row()
        .key(format!("section:{key}"))
        .items_center()
        .gap(6.)
        .pt(2.)
        .px(8.)
        .pb(6.)
        .cursor(Cursor::Pointer)
        .hover_group()
        .hover_motion(motion::CONTROL.anim(), motion::CONTROL.anim())
        .child(
            list_motion::arrow(open, 7., t.muted)
                .group_color(t.fg)
                .opacity(0.7),
        )
        .child(
            spaced_text(label.to_uppercase(), 0.05)
                .size(11.)
                .weight(ty::SEMIBOLD)
                .line_height(18.)
                .color(t.muted)
                .build(t)
                .group_color(t.fg),
        )
        .on_press(move |s: &mut State| flip_section(s, key));
    let name = motion::pressable(name);
    col()
        .key(format!("section-wrap:{key}"))
        .w_full()
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .px(4.)
                .child(name)
                .children(add),
        )
        .child(list_motion::fold(
            format!("section-fold:{key}"),
            true,
            slide,
            body,
        ))
}

/// `.section-add`: a 20 pt ＋, half faded until the pointer is over it.
fn section_add(t: &Theme, key: &'static str, tip: &'static str) -> El {
    let button = fill(key, t.hl_med)
        .size(20., 20.)
        .mr(2.)
        .radius(radius::XS)
        .label(tip)
        .child(hover_fg(glyph(Glyph::Plus, 12.), t).group_opacity(0.5, 1.0));
    roostr_ui::tooltip(motion::pressable(button), tip)
}

/// A sidebar row (`.item`, gpui-kit's SidebarMenuItem held to Roostr's h26,
/// padding 0 8, radius 6, 13 pt; hover and current are the kit's sidebar
/// accent): its lead over the label's 26 pt inset, an optional suffix.
fn item_row(
    t: &Theme,
    key: String,
    label: String,
    current: bool,
    lead: El,
    suffix: Option<El>,
) -> El {
    let fg = if current {
        t.kit.sidebar_accent_foreground
    } else {
        t.kit.sidebar_foreground
    };
    let mut r = row()
        .key(key)
        .w_full()
        .h(26.)
        .px(8.)
        .radius(radius::SM)
        .items_center()
        .cursor(Cursor::Pointer)
        .hover(t.kit.sidebar_accent)
        .press(t.kit.sidebar_accent)
        .label(label.clone())
        .child(row().w(26.).items_center().child(lead))
        .child(
            text(label)
                .font(ty::ui(13.).lh(22.))
                .color(fg)
                .truncate()
                .grow(1.)
                .shrink()
                .min_w(0.),
        )
        .children(suffix);
    if current {
        r = r.fill(t.kit.sidebar_accent);
    }
    motion::pressable_row(r)
}

/// `.none`: the empty-section note, a muted 12 pt line.
fn none(t: &Theme, note: &'static str) -> El {
    row()
        .px(8.)
        .child(text(note).font(ty::ui(12.)).color(t.muted))
}

fn recent(s: &State, t: &Theme, sh: &Shell, current: &Space, open_id: Option<&str>) -> El {
    let store = &s.store;
    let active = s.nav.space();
    let rows: Vec<El> = store
        .summaries
        .iter()
        .filter(|o| {
            !current.pinned_ids.contains(&o.id)
                && !is_system_type(&o.type_key)
                && o.channel_id == current.id
        })
        .take(8)
        .map(|r| {
            let lead = if store.layout_of(&r.type_key) == "task" {
                let id = r.id.clone();
                roostr_ui::checkbox(format!("row-check:{}", r.id), r.done == Some(true))
                    .size(KitSize::Medium)
                    .on_change(move |s: &mut State, done: bool| set_done(s, &id, done))
                    .build(t)
                    .mr(6.)
            } else {
                obj_icon(s, t, &object_icon(store, &r.icon, &r.type_key, active))
            };
            item_row(
                t,
                format!("recent:{}", r.id),
                or_untitled(&r.name),
                open_id == Some(r.id.as_str()),
                lead,
                None,
            )
            .on_press(open(r.id.clone()))
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![none(t, "Nothing yet")]
    } else {
        rows
    };
    section(t, sh, "recent", "Recently edited", None, rows)
}

fn types(s: &State, t: &Theme, sh: &Shell, open_id: Option<&str>) -> El {
    let space = space_id(s);
    let store = &s.store;
    let active = s.nav.space();
    let rows: Vec<El> = store
        .types
        .iter()
        .filter(|ty| ty.space == space)
        .map(|ty| {
            let icon = if ty.icon.is_empty() {
                type_glyph(store, &ty.key, active)
            } else {
                ty.icon.clone()
            };
            let name = if ty.name.is_empty() {
                ty.key.clone()
            } else {
                ty.name.clone()
            };
            let current = open_id == Some(ty.id.as_str());
            let suffix = ty
                .builtin
                .then(|| builtin_mark(t, format!("type-lock:{}", ty.id)));
            let r = item_row(
                t,
                format!("type:{}", ty.id),
                name,
                false,
                obj_icon(s, t, &icon),
                suffix,
            )
            .on_press(open(ty.id.clone()));
            if current { r.fill(t.hover) } else { r }
        })
        .collect();
    let add = section_add(t, "new-type", "New type").on_press(new_type);
    section(t, sh, "types", "Types", Some(add), rows)
}

fn props(s: &State, t: &Theme, sh: &Shell, open_id: Option<&str>) -> El {
    let space = space_id(s);
    let store = &s.store;
    let mut list: Vec<&RelationDef> = space_relations(&store.relations, &space)
        .into_iter()
        .filter(|r| !r.hidden && !is_reserved_key(&r.key))
        .collect();
    let label = |r: &RelationDef| {
        if r.name.is_empty() {
            r.key.to_lowercase()
        } else {
            r.name.to_lowercase()
        }
    };
    list.sort_by_key(|r| label(r));
    let rows: Vec<El> = list
        .into_iter()
        .map(|r| {
            let icon = r
                .icon_emoji
                .clone()
                .filter(|i| !i.is_empty())
                .unwrap_or_else(|| format_glyph(&r.format).to_string());
            let name = if r.name.is_empty() {
                r.key.clone()
            } else {
                r.name.clone()
            };
            let suffix = store
                .builtin_ids
                .contains(&r.id)
                .then(|| builtin_mark(t, format!("prop-lock:{}", r.id)));
            item_row(
                t,
                format!("prop:{}", r.id),
                name,
                open_id == Some(r.id.as_str()),
                obj_icon(s, t, &icon),
                suffix,
            )
            .on_press(open(r.id.clone()))
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![none(t, "No properties yet")]
    } else {
        rows
    };
    let add = section_add(t, "new-prop", "New property").on_press(open_new_property);
    section(t, sh, "props", "Properties", Some(add), rows)
}

/// `newType`: name prompt; per-space key clash check; restore a binned
/// type with that key, else create it; open it.
pub(crate) fn new_type(s: &mut State) {
    crate::modal::ask(s, "Type name:", |s, name| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let key = type_key_of(&name);
        if key.is_empty() {
            return;
        }
        let space = space_id(s);
        let clash = s
            .store
            .types
            .iter()
            .any(|t: &TypeDef| t.key == key && (t.space == space || t.space.is_empty()));
        if clash {
            crate::modal::alert(
                s,
                format!("A type with key \"{key}\" already exists in this space."),
            );
            return;
        }
        let binned = roostr_core::api::query_all(
            serde_json::json!({ "type": "type", "includeDeleted": true }),
            s,
        );
        s.rt.spawn(async move |cx| {
            let rows = binned.await.unwrap_or_default();
            let found = rows.into_iter().find(|r| {
                r.deleted == Some(true)
                    && roostr_core::types::field_str(&r.fields, "key") == key
                    && roostr_core::types::field_str(&r.fields, "channel") == space
            });
            let id = if let Some(row) = found {
                let task = cx.update(|s| roostr_core::api::note::restore(&row.id, s));
                if task.await.is_err() {
                    return;
                }
                row.id
            } else {
                let mut fields = Fields::new();
                fields.insert("channel".into(), Value::string(space));
                fields.insert("key".into(), Value::string(key));
                fields.insert("layout".into(), Value::string("page"));
                let task = cx.update(|s| {
                    roostr_core::api::note::create(&name, Some("type"), Some(fields), s)
                });
                let Ok(id) = task.await else { return };
                id
            };
            cx.update(|s| {
                s.store.refresh_all();
                navigate(Route::Object(id), s);
            });
        })
        .detach();
    });
}

/// Type key from a name: lowercase, runs of non-alphanumerics → `_`, trimmed.
fn type_key_of(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// The New property suggest (roostr-properties' `PropertySuggest`) while open:
/// toggled by the Properties section's ＋, anchored under it (`new-prop`).
#[derive(Default)]
pub(crate) struct NewProp {
    pub(crate) suggest: Option<roostr_properties::PropertySuggestState>,
}

/// The Properties section's ＋: open the suggest for the space (or close it).
pub(crate) fn open_new_property(s: &mut State) {
    if s.slot::<NewProp>().suggest.take().is_some() {
        return;
    }
    let space = space_id(s);
    s.slot::<NewProp>().suggest = Some(roostr_properties::PropertySuggestState::new(
        space,
        Vec::new(),
        Vec::new(),
    ));
}

/// The open suggest, anchored under the ＋ (`new-prop`): picking a property
/// opens it; any answer (or Escape, a press outside) closes the menu.
pub(crate) fn new_prop_overlay(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let Some(state) = s.get::<NewProp>().and_then(|np| np.suggest.as_ref()) else {
        return;
    };
    let panel = roostr_properties::PropertySuggest::new("new-prop-suggest", state)
        .on_state(|s: &mut State| {
            s.slot::<NewProp>().suggest.get_or_insert_with(|| {
                roostr_properties::PropertySuggestState::new(String::new(), Vec::new(), Vec::new())
            })
        })
        .on_event(
            |s: &mut State,
             ev: roostr_properties::SuggestEvent<roostr_properties::PropertyPick>| {
                s.slot::<NewProp>().suggest = None;
                if let roostr_properties::SuggestEvent::Picked(
                    roostr_properties::PropertyPick::Relation(rel),
                ) = ev
                {
                    navigate(Route::Object(rel.id.clone()), s);
                }
            },
        )
        .build(s, cx, t, layer);
    layer.push(roostr_ui::overlay::anchored(
        panel,
        "new-prop",
        roostr_ui::Placement::BottomStart,
        4.,
    ));
}

#[cfg(test)]
mod tests {
    use super::type_key_of;

    #[test]
    fn type_keys_from_names() {
        assert_eq!(type_key_of("Game publisher"), "game_publisher");
        assert_eq!(type_key_of("  --Hello, World!! "), "hello_world");
        assert_eq!(type_key_of("!!"), "");
    }
}
