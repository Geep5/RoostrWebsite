//! Shared pieces of the data views: the record query each view runs, the
//! space's relation defs, collection membership, cell text, and the small
//! styled primitives (menus, rows, tag pills) every view draws with.

use roostr_core::filters::{engine_filters_of, serving_filters_of, space_filter_of};
use roostr_core::lite_ui::Notice;
use roostr_core::nav::{self, Route};
use roostr_core::objects::{kind_name, object_icon};
use roostr_core::relations::{current_space_id, object_space_id, space_relations};
use roostr_core::{Object, QueryRow, RelationDef, State, Store, Value, api, field_str};
use roostr_rt::Task;
use roostr_ui::tags::{object_pill_paint, tag_colors, unknown_tag_paint};
use roostr_ui::{Button, Empty, Images, Theme, icon_text, outline_button, ty};
use rune::{Color, Cursor, El, TextStyle, col, el, row, text};
use serde_json::{Value as Json, json};

use crate::DataViewKind;
use crate::config;

/// The space's relation defs the view offers (`scopedRelations`).
pub(crate) fn scoped_relations(s: &State, object: &Object) -> Vec<RelationDef> {
    let space = object_space_id(&s.store, &object.fields);
    space_relations(&s.store.relations, &space)
        .into_iter()
        .cloned()
        .collect()
}

/// The record query of the page (`tableBody`): `None` when there is nothing
/// to query (a type without a key, an empty collection). Serving rules ride
/// along as `servingFilters` (client-only; peeled before sending).
pub(crate) fn table_body(
    s: &State,
    object: &Object,
    kind: DataViewKind,
    search: &str,
    relations: &[RelationDef],
) -> Option<Json> {
    let mut body = serde_json::Map::new();
    let text = search.trim();
    if !text.is_empty() {
        body.insert("textQuery".into(), json!(text));
    }
    // A bundled type (channel "") owns no space: its page anchors to the space being viewed.
    let space_filter = space_filter_of(&object.fields, &current_space_id(s));
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
    s: &mut State,
) -> Task<anyhow::Result<()>> {
    let task = api::note::set_field(
        collection_id,
        "collectionIds",
        Value::values(ids.iter().map(Value::string).collect()),
        s,
    );
    s.rt.spawn(async move |_| task.await.map(|_| ()))
}

/// Drop `ids` from a collection (membership only; the objects stay).
pub(crate) fn remove_members(
    collection_id: &str,
    ids: &[String],
    s: &mut State,
) -> Task<anyhow::Result<()>> {
    let current = s
        .store
        .object(collection_id)
        .map(|o| config::member_ids(&o.fields))
        .unwrap_or_default();
    let next: Vec<String> = current.into_iter().filter(|m| !ids.contains(m)).collect();
    set_members(collection_id, &next, s)
}

pub(crate) fn open_object(id: &str, s: &mut State) {
    nav::navigate(Route::Object(id.to_string()), s);
}

pub(crate) fn open_object_tab(id: &str, s: &mut State) {
    nav::open_tab(Route::Object(id.to_string()), true, s);
}

/// A row's display icon (`objectIcon(iconEmoji, typeKey)`).
pub(crate) fn row_icon(s: &State, row: &QueryRow) -> String {
    object_icon(
        &s.store,
        field_str(&row.fields, "iconEmoji"),
        &row.type_key,
        &current_space_id(s),
    )
}

pub(crate) fn row_name(row: &QueryRow) -> String {
    let name = field_str(&row.fields, "name");
    if name.is_empty() {
        "Untitled".into()
    } else {
        name.to_string()
    }
}

/// A type's display name, never the raw key.
pub(crate) fn type_name(s: &State, type_key: &str) -> String {
    s.store
        .types
        .iter()
        .find(|t| t.key == type_key)
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| kind_name(type_key))
}

/// Object ids a value links: a list, a bare link, or a bare id.
pub(crate) fn link_ids(v: &Value) -> Vec<String> {
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
    ids.into_iter().filter(|id| !id.is_empty()).collect()
}

/// A linked object's display name: its name, else the id's first 8.
fn link_name(store: &Store, id: &str) -> String {
    store
        .summary(id)
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| id.chars().take(8).collect())
}

/// Object ids a value names, as display names (`cell` for object values).
pub(crate) fn link_names(s: &State, v: &Value) -> String {
    link_ids(v)
        .iter()
        .map(|id| link_name(&s.store, id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An emoji, glyph or image icon drawn inline with text of `style`: emoji as
/// text, a URL / data URL as a `style.size` image (Roostr's `icon_text`).
pub(crate) fn icon(s: &State, icon: &str, style: TextStyle) -> El {
    match s.get::<Images>() {
        Some(images) => icon_text(icon, style.size, style, images),
        None => roostr_ui::emoji_text(icon, style),
    }
}

/// A linked object as a filled pill: its emoji (else its type's) and name.
/// Option tags are outlines ([`tag_pill`]), so the two read apart.
pub(crate) fn object_pill(s: &State, t: &Theme, id: &str, space: &str) -> El {
    let (glyph, type_key) = s
        .store
        .summary(id)
        .map(|o| (o.icon.clone(), o.type_key.clone()))
        .unwrap_or_default();
    let glyph = object_icon(&s.store, &glyph, &type_key, space);
    let paint = object_pill_paint(t, t.fg);
    let mut pill = roostr_ui::tag(paint, 20.0, 12.0).gap(4.0);
    if !glyph.is_empty() {
        pill = pill.child(icon(s, &glyph, ty::ui(11.0).lh(18.0)));
    }
    pill.child(
        text(link_name(&s.store, id))
            .font(ty::ui(12.0).lh(18.0))
            .color(paint.fg)
            .truncate()
            .shrink(),
    )
    .build(t)
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

/// A tag/status option: an outline pill in the option's own colour. An
/// unknown colour gets a neutral outline and `ink`, the text colour around it.
pub(crate) fn tag_pill(t: &Theme, label: &str, color: &str, small: bool, ink: Color) -> El {
    let paint = tag_colors(t, color).unwrap_or_else(|| unknown_tag_paint(ink));
    let (height, size) = if small { (18.0, 11.0) } else { (20.0, 12.0) };
    roostr_ui::tag(paint, height, size)
        .label(label.to_string())
        .build(t)
}

/// The popover/menu surface (`.vmenu`, `.plist`, `.ctx-menu`): panel fill,
/// border, menu radius, padding 6, rows 2 apart, the `0 12 32 /.5` shadow.
pub(crate) fn menu_surface(t: &Theme) -> El {
    col()
        .items_stretch()
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(roostr_ui::radius::MENU)
        .padding(6.0)
        .gap(2.0)
        .box_shadow(roostr_ui::shadow::gpui(12.0, 32.0, 0.0, 0.5))
}

/// A `.vrow` menu row, the kit list row: gap 12, radius 6, padding 6 × 8,
/// 13px, the kit list hover and active fills. Add its content and handler.
pub(crate) fn vrow(t: &Theme, key: &str, active: bool) -> El {
    let e = row()
        .key(key.to_string())
        .center_y()
        .gap(12.0)
        .radius(6.0)
        .px(8.0)
        .py(6.0)
        .cursor(Cursor::Pointer)
        .focusable(true)
        .no_focus_ring()
        .focus_highlight(t.kit.list_hover);
    if active {
        e.fill(t.kit.list_active)
    } else {
        e.hover(t.kit.list_hover)
    }
}

/// A [`vrow`]'s content: `label` left, `trailer` right.
pub(crate) fn vrow_content(label: El, trailer: Option<El>) -> El {
    row()
        .grow(1.0)
        .min_w(0.0)
        .center_y()
        .justify_between()
        .gap(12.0)
        .child(label)
        .children(trailer)
}

/// 13px fg text, one line (menu and row labels).
pub(crate) fn label(t: &Theme, s: impl Into<String>) -> El {
    text(s.into()).font(ty::ui(13.0)).color(t.fg).truncate()
}

/// The uppercase title of a menu (`.ptitle`, `.more-title`).
pub(crate) fn menu_title(t: &Theme, s: &str) -> El {
    el().px(8.0)
        .pt(2.0)
        .pb(4.0)
        .child(text(s.to_uppercase()).font(ty::ui(11.0)).color(t.muted))
}

/// A 1px separator (`.sort-div`), the kit separator.
pub(crate) fn separator(t: &Theme) -> El {
    el().h(1.0).w_full().my(4.0).fill(t.border).a11y_hidden()
}

/// The kit empty state with one muted line of `size` px (`.empty`, e.g. "No
/// objects match."); callers set the padding.
pub(crate) fn empty_state(t: &Theme, line: &str, size: f32) -> El {
    Empty::new()
        .text(line.to_string(), size, None)
        .build(t)
        .items_start()
        .gap(0.0)
        .padding(0.0)
}

/// The accent "New" button (`.new-btn`), the kit primary button; `disabled`
/// while a create runs.
pub(crate) fn new_button(key: &str, disabled: bool) -> Button {
    roostr_ui::disabled(roostr_ui::primary_button(key.to_string(), "New"), disabled)
}

/// A lite notice row as roostr-core's `notice_row` drew it: wrapping, gap 10,
/// py 6, 12px muted text, its action an outline button keyed `{key}-action`.
pub(crate) fn notice_row(t: &Theme, key: &str, notice: Notice) -> El {
    let mut out = row()
        .key(key.to_string())
        .flex_wrap()
        .center_y()
        .gap(10.0)
        .py(6.0)
        .child(text(notice.text).font(ty::ui(12.0)).color(t.muted));
    if let Some(action) = notice.action {
        out = out.child(
            outline_button(format!("{key}-action"), action.label())
                .on_press(move |s: &mut State| action.run(s))
                .build(t),
        );
    }
    out
}

/// A window point's rectangle test.
pub(crate) fn inside(r: [f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
}
