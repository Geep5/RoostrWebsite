//! Shared pieces of the data views: the record query each view runs, the
//! space's relation defs, collection membership, cell text, and the small
//! styled primitives (menus, rows, tag pills) every view draws with.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::filters::{engine_filters_of, serving_filters_of, space_filter_of};
use roostr_core::nav::{self, Route};
use roostr_core::objects::{kind_name, object_icon};
use roostr_core::options::{tag_colors, unknown_tag_bg};
use roostr_core::relations::{current_space_id, object_space_id, space_relations};
use roostr_core::{Object, QueryRow, RelationDef, Store, Value, api, field_str, theme};
use serde_json::{Value as Json, json};

use crate::DataViewKind;
use crate::config;

/// The space's relation defs the view offers (`scopedRelations`).
pub(crate) fn scoped_relations(object: &Object, cx: &App) -> Vec<RelationDef> {
    let store = Store::global(cx).read(cx);
    let space = object_space_id(store, &object.fields);
    space_relations(&store.relations, &space)
        .into_iter()
        .cloned()
        .collect()
}

/// The record query of the page (`tableBody`): `None` when there is nothing
/// to query (a type without a key, an empty collection). Serving rules ride
/// along as `servingFilters` (client-only; peeled before sending).
pub(crate) fn table_body(
    object: &Object,
    kind: DataViewKind,
    search: &str,
    relations: &[RelationDef],
    cx: &App,
) -> Option<Json> {
    let mut body = serde_json::Map::new();
    let text = search.trim();
    if !text.is_empty() {
        body.insert("textQuery".into(), json!(text));
    }
    // A bundled type (channel "") owns no space: its page anchors to the space being viewed.
    let space_filter = space_filter_of(&object.fields, &current_space_id(cx));
    let engine = engine_filters_of(&object.fields, relations);
    let serving: Vec<Json> = serving_filters_of(&object.fields)
        .into_iter()
        .map(|s| json!({"condition": s.condition, "value": s.value}))
        .collect();
    body.insert("servingFilters".into(), Json::Array(serving));
    match kind {
        DataViewKind::Query => {
            let mut filters = engine;
            filters.push(space_filter);
            body.insert("setId".into(), json!(object.id));
            body.insert("filters".into(), Json::Array(filters));
        }
        DataViewKind::Type => {
            let key = field_str(&object.fields, "key");
            if key.is_empty() {
                return None;
            }
            // A type page names its own type: never apply the unsourced-query exclusion.
            let mut filters: Vec<Json> = engine
                .into_iter()
                .filter(|f| {
                    !(f["key"] == "typeKey"
                        && f["condition"] == "notIn"
                        && f["value"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v == key)))
                })
                .collect();
            // Computers and skills are vault-wide rosters.
            if key != "machine" && key != "skill" {
                filters.push(space_filter);
            }
            body.insert("type".into(), json!(key));
            body.insert("filters".into(), Json::Array(filters));
        }
        DataViewKind::Collection => {
            let members = config::member_ids(&object.fields);
            if members.is_empty() {
                return None;
            }
            // View filters stack on top of membership (AND semantics).
            let mut filters = vec![json!({"key": "id", "condition": "in", "value": members})];
            filters.extend(engine);
            filters.push(space_filter);
            body.insert("filters".into(), Json::Array(filters));
        }
    }
    Some(Json::Object(body))
}

/// `body` without the client-only keys, merged with `extra`.
pub(crate) fn engine_body(body: &Json, extra: Json) -> Json {
    let mut out = body.as_object().cloned().unwrap_or_default();
    out.remove("servingFilters");
    if let Json::Object(extra) = extra {
        for (k, v) in extra {
            out.insert(k, v);
        }
    }
    Json::Object(out)
}

/// The serving rules the body carries.
pub(crate) fn body_serving_rules(body: &Json) -> Vec<(String, String)> {
    body["servingFilters"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    (
                        r["condition"].as_str().unwrap_or("equal").to_string(),
                        r["value"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Sorts for kanban/gallery: the view's, else updatedAt desc; empties last.
pub(crate) fn view_sorts_json(sorts: &[config::SortRule]) -> Json {
    if sorts.is_empty() {
        return json!([{"key": "updatedAt", "type": "desc", "emptyPlacement": "end"}]);
    }
    Json::Array(
        sorts
            .iter()
            .map(|s| json!({"key": s.key, "type": s.type_str(), "emptyPlacement": "end"}))
            .collect(),
    )
}

/// Replace a collection's members.
pub(crate) fn set_members(
    collection_id: &str,
    ids: &[String],
    cx: &mut App,
) -> Task<anyhow::Result<()>> {
    let task = api::note::set_field(
        collection_id,
        "collectionIds",
        Value::values(ids.iter().map(Value::string).collect()),
        cx,
    );
    cx.background_spawn(async move { task.await.map(|_| ()) })
}

/// Drop `ids` from a collection (membership only; the objects stay).
pub(crate) fn remove_members(
    collection_id: &str,
    ids: &[String],
    cx: &mut App,
) -> Task<anyhow::Result<()>> {
    let current = Store::global(cx)
        .read(cx)
        .object(collection_id)
        .map(|o| config::member_ids(&o.fields))
        .unwrap_or_default();
    let next: Vec<String> = current.into_iter().filter(|m| !ids.contains(m)).collect();
    set_members(collection_id, &next, cx)
}

pub(crate) fn open_object(id: &str, cx: &mut App) {
    nav::navigate(Route::Object(id.to_string()), cx);
}

pub(crate) fn open_object_tab(id: &str, cx: &mut App) {
    nav::open_tab(Route::Object(id.to_string()), true, cx);
}

/// A row's display icon (`objectIcon(iconEmoji, typeKey)`).
pub(crate) fn row_icon(row: &QueryRow, cx: &App) -> String {
    let store = Store::global(cx).read(cx);
    object_icon(
        store,
        field_str(&row.fields, "iconEmoji"),
        &row.type_key,
        &current_space_id(cx),
    )
}

pub(crate) fn row_name(row: &QueryRow) -> SharedString {
    let name = field_str(&row.fields, "name");
    if name.is_empty() {
        "Untitled".into()
    } else {
        SharedString::from(name.to_string())
    }
}

/// A type's display name, never the raw key.
pub(crate) fn type_name(type_key: &str, cx: &App) -> String {
    let store = Store::global(cx).read(cx);
    store
        .types
        .iter()
        .find(|t| t.key == type_key)
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| kind_name(type_key))
}

/// Object ids a value names, as display names (`cell` for object values).
pub(crate) fn link_names(v: &Value, cx: &App) -> String {
    let store = Store::global(cx).read(cx);
    let ids: Vec<String> = if v.values_value.is_some() {
        v.items()
            .iter()
            .map(|i| {
                i.link_value
                    .as_ref()
                    .map(|l| l.target_id.clone())
                    .or_else(|| i.string_value.clone())
                    .unwrap_or_default()
            })
            .collect()
    } else {
        vec![
            v.link_value
                .as_ref()
                .map(|l| l.target_id.clone())
                .or_else(|| v.string_value.clone())
                .unwrap_or_default(),
        ]
    };
    ids.iter()
        .filter(|id| !id.is_empty())
        .map(|id| {
            store
                .summary(id)
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.chars().take(8).collect())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// String items of a list value (or a lone string).
pub(crate) fn string_items(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else { return Vec::new() };
    if v.values_value.is_some() {
        return v
            .items()
            .iter()
            .filter_map(|i| i.string_value.clone())
            .filter(|s| !s.is_empty())
            .collect();
    }
    v.string_value
        .clone()
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect()
}

pub(crate) fn find_rel<'a>(relations: &'a [RelationDef], key: &str) -> Option<&'a RelationDef> {
    relations.iter().find(|r| r.key == key)
}

// ── Styled primitives ────────────────────────────────────────────

/// Anytype tagItem.isSmall: filled pill, the option's own colours.
pub(crate) fn tag_pill(text: impl Into<SharedString>, color: &str, small: bool) -> Div {
    let (bg, fg) = tag_colors(color).map_or((unknown_tag_bg(), None), |(bg, fg)| (bg, Some(fg)));
    div()
        .flex_none()
        .rounded(px(10.))
        .px(px(6.))
        .h(px(if small { 18. } else { 20. }))
        .line_height(px(if small { 18. } else { 20. }))
        .text_size(px(if small { 11. } else { 12. }))
        .bg(bg)
        .when_some(fg, |d, fg| d.text_color(fg))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(text.into())
}

/// The popover/menu surface (`.vmenu`, `.plist`, `.ctx-menu`).
pub(crate) fn menu_surface() -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .rounded(theme::radius::menu())
        .p(px(6.))
        .gap(px(2.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .shadow(vec![shadow(hsla(0., 0., 0., 0.5), 0., 12., 32., 0.)])
}

/// A `.vrow` menu row: label left, trailer right; hover/active hl-med.
pub(crate) fn vrow(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .rounded(px(6.))
        .px(px(8.))
        .py(px(6.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .cursor_pointer()
        .when(active, |d| d.bg(theme::hl_med()))
        .hover(|s| s.bg(theme::hl_med()))
}

/// The uppercase title of a menu (`.ptitle`, `.more-title`).
pub(crate) fn menu_title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(theme::muted())
        .px(px(8.))
        .pt(px(2.))
        .pb(px(4.))
        .child(text.into().to_uppercase())
}

/// A 1px separator (`.sort-div`).
pub(crate) fn separator() -> Div {
    div().flex_none().h(px(1.)).bg(theme::border()).my(px(4.))
}

/// Arc-friendly read of the cached object.
pub(crate) fn cached_object(id: &str, cx: &App) -> Option<Arc<Object>> {
    Store::global(cx).read(cx).object(id)
}

/// The accent "New" button (`.new-btn`).
pub(crate) fn new_button(id: impl Into<ElementId>, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(8.))
        .bg(theme::accent())
        .text_color(theme::white())
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.hover(|s| s.bg(rgb(0x2b95ff))))
}

/// One outer box shadow.
pub(crate) fn shadow(color: Hsla, x: f32, y: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(x), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

/// Shared bounds of a laid-out element, captured each frame.
pub(crate) type BoundsCell = Rc<Cell<Bounds<Pixels>>>;

pub(crate) fn bounds_cell() -> BoundsCell {
    Rc::new(Cell::new(Bounds::default()))
}

/// An invisible overlay recording its parent's bounds into `cell` (parent must be `relative`).
pub(crate) fn track_bounds(cell: &BoundsCell) -> impl IntoElement {
    let cell = cell.clone();
    canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// Is `point` inside any of `cells`?
pub(crate) fn inside_any(cells: &[&BoundsCell], point: Point<Pixels>) -> bool {
    cells.iter().any(|c| c.get().contains(&point))
}

/// Where a floating menu sits relative to its (relative) host element.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// Under the host, left edges aligned.
    BelowLeft,
    /// Under the host, right edges aligned.
    BelowRight,
    /// Beside the host's right edge, tops aligned.
    Beside,
    /// Beside the host's left edge, tops aligned.
    BesideLeft,
}

impl Place {
    /// `keepInView`'s `beside`: a `width`-wide menu goes to the left of
    /// `host`, else its right, else - no room either way, a narrow window -
    /// under it.
    pub(crate) fn beside(host: Bounds<Pixels>, width: Pixels, viewport: Size<Pixels>) -> Self {
        const GAP: Pixels = px(12.);
        const MARGIN: Pixels = px(8.);
        if host.left() - GAP - MARGIN >= width {
            Place::BesideLeft
        } else if viewport.width - host.right() - GAP - MARGIN >= width {
            Place::Beside
        } else {
            Place::BelowLeft
        }
    }
}

/// A floating menu anchored to its host (`keepInView`): child of a `relative` host.
pub(crate) fn floating(place: Place, gap: f32, content: impl IntoElement) -> Div {
    let (anchor, slot) = match place {
        Place::BelowLeft => (
            Anchor::TopLeft,
            div().absolute().left_0().top_full().mt(px(gap)),
        ),
        Place::BelowRight => (
            Anchor::TopRight,
            div().absolute().right_0().top_full().mt(px(gap)),
        ),
        Place::Beside => (
            Anchor::TopLeft,
            div().absolute().left(relative(1.)).top_0().ml(px(gap)),
        ),
        Place::BesideLeft => (
            Anchor::TopRight,
            div().absolute().right(relative(1.)).top_0().mr(px(gap)),
        ),
    };
    // In-tree (not `deferred`): deferred content is off the focus path, so its
    // inputs would lose Enter/Escape. The data view paints its chrome after
    // the records (flex_col_reverse) so these menus sit above the rows.
    slot.child(
        anchored()
            .anchor(anchor)
            .snap_to_window_with_margin(px(8.))
            .child(content),
    )
}
