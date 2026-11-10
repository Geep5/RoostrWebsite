//! The line under an object's title (`ObjectMeta.svelte`): its type (opens
//! the type), who runs a recurring object, a query's source types (each
//! removable, more addable) and how many objects link here (`backlinks.ts`).

use std::collections::HashMap;

use roostr_core::objects::{builtin_glyph, kind_name, object_icon, type_glyph};
use roostr_core::relations::current_space_id;
use roostr_core::repeat::describe_repeat;
use roostr_core::types::{guest_agents, repeat_of};
use roostr_core::{Fields, Route, State, StoreEvent, Value, api, lite_ui, nav};
use roostr_ui::color::brighten;
use roostr_ui::tags::badge_colors;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Button, Glyph, Theme, glyph, motion, radius, ty};
use rune::{Cursor, El, ScrollSpec, TextStyle, col, el, row, text};
use serde_json::json;

use crate::suggest::{SuggestEvent, TypeSuggest, TypeSuggestState};
use crate::widgets::{backdrop, below, hover_tint, icon_text_el, later, pop_shadow, prefix, tip};

#[derive(Clone)]
struct Backlink {
    id: String,
    name: String,
    type_key: String,
    icon: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Sources,
    Backlinks,
}

/// One object's badges: its backlinks, the open popover and the source-type menu.
#[derive(Default)]
struct MetaState {
    backlinks: Vec<Backlink>,
    open: Option<Open>,
    suggest: Option<TypeSuggestState>,
}

/// Every open object's badges by object id.
#[derive(Default)]
struct Metas(HashMap<String, MetaState>);

/// Starts `object_id`'s badges: loads it into the store cache and fetches its
/// backlinks (GPUI's `ObjectMeta::new`). Idempotent.
pub fn open_meta(s: &mut State, object_id: &str) {
    if s.slot::<Metas>().0.contains_key(object_id) {
        return;
    }
    s.slot::<Metas>()
        .0
        .insert(object_id.to_string(), MetaState::default());
    s.store.ensure_object(object_id);
    load_backlinks(s, object_id);
}

/// Drops `object_id`'s badge state (GPUI dropped the view).
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Metas>().0.remove(object_id);
}

/// Lite: backlinks come from what this device holds; a refresh (a space
/// finished loading, objects fetched) may bring more.
pub(crate) fn register(s: &mut State) {
    s.on_store(on_store);
}

fn on_store(s: &mut State, event: &StoreEvent) {
    if !matches!(event, StoreEvent::Refreshed) || roostr_core::lite::status(s).is_none() {
        return;
    }
    let ids: Vec<String> = s.slot::<Metas>().0.keys().cloned().collect();
    for id in ids {
        load_backlinks(s, &id);
    }
}

/// Objects whose fields link to this one: the engine's `$links` (every
/// link field, list of links and collection membership) holds the id.
fn load_backlinks(s: &mut State, object_id: &str) {
    let task = api::query_all(
        json!({ "filters": [{ "key": "$links", "condition": "in", "value": [object_id] }] }),
        s,
    );
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(records) = task.await else { return };
        let out: Vec<Backlink> = records
            .iter()
            .filter(|r| r.id != id)
            .map(|r| Backlink {
                id: r.id.clone(),
                name: Some(roostr_core::field_str(&r.fields, "name"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Untitled")
                    .to_string(),
                type_key: r.type_key.clone(),
                icon: roostr_core::field_str(&r.fields, "iconEmoji").to_string(),
            })
            .collect();
        cx.update(|s| {
            if let Some(me) = s.slot::<Metas>().0.get_mut(&id) {
                me.backlinks = out;
            }
        });
    })
    .detach();
}

fn toggle(s: &mut State, object_id: &str, which: Open) {
    let Some(me) = s.slot::<Metas>().0.get_mut(object_id) else {
        return;
    };
    me.open = if me.open == Some(which) {
        None
    } else {
        Some(which)
    };
    me.suggest = None;
    if me.open != Some(Open::Sources) {
        return;
    }
    let Some(object) = s.store.object(object_id) else {
        return;
    };
    let keys = source_keys(&object.fields);
    let placeholder = if keys.is_empty() {
        "Search types… (e.g. p → Person, Project)"
    } else {
        "Add another type…"
    };
    let space = object_space(&object.fields, s);
    let suggest = TypeSuggestState::new(space, keys, placeholder.to_string());
    if let Some(me) = s.slot::<Metas>().0.get_mut(object_id) {
        me.suggest = Some(suggest);
    }
}

fn save_sources(s: &mut State, object_id: &str, keys: Vec<String>) {
    if let Some(suggest) = s
        .slot::<Metas>()
        .0
        .get_mut(object_id)
        .and_then(|me| me.suggest.as_mut())
    {
        suggest.set_exclude(keys.clone());
    }
    let v = Value::values(keys.into_iter().map(Value::string).collect());
    api::note::set_field(object_id, "setOf", v, s).detach_and_log_err(&s.rt);
}

fn close(s: &mut State, object_id: &str) {
    if let Some(me) = s.slot::<Metas>().0.get_mut(object_id) {
        me.open = None;
        me.suggest = None;
    }
}

fn source_keys(fields: &Fields) -> Vec<String> {
    fields
        .get("setOf")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The object's own space, else the open one.
fn object_space(fields: &Fields, s: &State) -> String {
    match roostr_core::field_str(fields, "channel") {
        "" => current_space_id(s),
        x => x.to_string(),
    }
}

/// A badge: Roostr's tag look (26px, radius md, medium 13px label after a
/// 16px lead) with hover brighten, its tooltip and, when `clickable`, a
/// pointer and the press shrink (the caller adds `on_press`).
fn badge(t: &Theme, key: &str, lead: El, label: String, clickable: bool, tooltip: &str) -> El {
    let label = text(label).font(badge_font()).color(badge_colors(t, "").fg);
    badge_el(t, key, lead, label, clickable, tooltip)
}

/// The badge label's type: medium 13px on the badge's 26px line.
fn badge_font() -> TextStyle {
    ty::ui(13.0).lh(26.0).medium()
}

/// [`badge`] around a label already built (a rolling count).
fn badge_el(t: &Theme, key: &str, lead: El, label: El, clickable: bool, tooltip: &str) -> El {
    let c = badge_colors(t, "");
    let lit = brighten(c.bg, 1.18);
    let mut b = row()
        .key(key.to_string())
        .h(26.0)
        .items_center()
        .gap(6.0)
        .pl(6.0)
        .pr(10.0)
        .radius(radius::MD)
        .fill(c.bg)
        .hover(lit)
        .press(lit)
        .cursor(if clickable {
            Cursor::Pointer
        } else {
            Cursor::Default
        })
        .child(lead)
        .child(label);
    if clickable {
        b = motion::pressable(b);
    }
    tip(b, tooltip)
}

fn emoji(e: String) -> El {
    row()
        .w(16.0)
        .justify_center()
        .child(text(e).font(ty::ui(14.0)))
}

fn badge_glyph(t: &Theme, g: Glyph) -> El {
    glyph(g, 16.0).color(badge_colors(t, "").icon)
}

/// The popovers' surface: 280–380px wide, at most 360px tall (scrolls).
fn meta_pop(t: &Theme, key: &str, body: El) -> El {
    col()
        .key(key.to_string())
        .min_w(280.0)
        .max_w(380.0)
        .max_h(360.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .radius(radius::MENU)
        .box_shadow(pop_shadow())
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(body.gap(4.0).padding(10.0).w_full())
}

/// The type / repeat / query source / backlink badges under `object_id`'s
/// title; `flush` drops their 48px left indent (wide data pages).
pub fn object_meta(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    flush: bool,
) -> El {
    let fallback = MetaState::default();
    let me = match s.get::<Metas>().and_then(|m| m.0.get(object_id)) {
        Some(me) => me,
        None => {
            let id = object_id.to_string();
            later(s, move |s| open_meta(s, &id));
            &fallback
        }
    };
    let Some(object) = s.store.object(object_id) else {
        return el();
    };
    let space = object_space(&object.fields, s);
    let store = &s.store;
    let type_def = store
        .types
        .iter()
        .find(|t| t.key == object.type_key && t.space == space)
        .cloned();
    let type_icon = type_def
        .as_ref()
        .map(|t| t.icon.clone())
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| builtin_glyph(&object.type_key).to_string());
    let type_name = type_def
        .as_ref()
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| kind_name(&object.type_key));
    let lead = if type_icon.is_empty() {
        badge_glyph(t, Glyph::BadgeDot)
    } else {
        emoji(type_icon)
    };

    // `.meta`: `margin: 2px 0 14px 48px`, `0 16px 10px` at ≤720px. The window's
    // width comes from an inert full-window probe in the layer.
    let probe = format!("meta-viewport-{object_id}");
    let narrow = cx
        .layout(&probe)
        .is_some_and(|r| r[2] > 0.0 && roostr_ui::layout::is_mobile(r[2]));
    layer.push(el().key(probe).absolute(0.0, 0.0).w_full().h_full().inert());
    let mut line = row()
        .key(format!("meta-{object_id}"))
        .flex_wrap()
        .items_center()
        .gap(6.0);
    line = if narrow {
        line.mb(10.0).mx(16.0)
    } else {
        line.mt(2.0).mb(14.0)
    };
    if !narrow && !flush {
        line = line.ml(48.0);
    }

    if me.open.is_some() {
        let id = object_id.to_string();
        backdrop(layer, "meta-backdrop", move |s: &mut State| close(s, &id));
    }

    line = line.child(match &type_def {
        Some(td) => {
            let id = td.id.clone();
            badge(t, "type", lead, type_name, true, "Type")
                .on_press(move |s: &mut State| nav::navigate(Route::Object(id.clone()), s))
        }
        None => badge(t, "type", lead, type_name, false, "Type"),
    });

    if let Some(rule) = repeat_of(&object.fields) {
        let mut ids = guest_agents(&object.fields);
        if let Some(a) = object.field("assignee") {
            ids.extend(guest_agents(&Fields::from([(
                "agent".to_string(),
                a.clone(),
            )])));
        }
        let desc = describe_repeat(&rule);
        line = line.child(match ids.first() {
            Some(id) => {
                let a = store.agent(id);
                let icon = a
                    .map(|a| a.icon.clone())
                    .filter(|i| !i.is_empty())
                    .unwrap_or_else(|| "🤖".into());
                let name = a
                    .map(|a| a.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("{}…", prefix(id, 8)));
                let target = id.clone();
                let tooltip = format!("{name} runs this · {desc}");
                badge(t, "runner", emoji(icon), name, true, &tooltip)
                    .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
            }
            None => badge(
                t,
                "runner",
                emoji("↻".into()),
                "Repeats".into(),
                false,
                &format!("Repeats · {desc} · no agent: add one to its Agent property"),
            ),
        });
    }

    if object.type_key == "query" {
        let keys = source_keys(&object.fields);
        let sources: Vec<(String, String, String, String)> = keys
            .iter()
            .map(|key| {
                let t = store
                    .types
                    .iter()
                    .find(|x| &x.key == key && x.space == space);
                let built_in = match key.as_str() {
                    "query" => "Query",
                    "collection" => "Collection",
                    _ => "",
                };
                let name = t
                    .map(|t| t.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| {
                        if built_in.is_empty() {
                            key.clone()
                        } else {
                            built_in.into()
                        }
                    });
                let icon = t
                    .map(|t| t.icon.clone())
                    .filter(|i| !i.is_empty())
                    .unwrap_or_else(|| type_glyph(store, key, &space));
                (
                    key.clone(),
                    t.map(|t| t.id.clone()).unwrap_or_default(),
                    name,
                    icon,
                )
            })
            .collect();
        let (lead, label) = match sources.first() {
            Some(first) => (
                emoji(first.3.clone()),
                format!(
                    "Object {}: {}",
                    if sources.len() == 1 { "Type" } else { "Types" },
                    sources
                        .iter()
                        .map(|s| s.2.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            None => (badge_glyph(t, Glyph::BadgeDot), "All objects".to_string()),
        };
        let id = object_id.to_string();
        line = line.child(
            badge(t, "sources", lead, label, true, "What this query shows")
                .on_press(move |s: &mut State| toggle(s, &id, Open::Sources)),
        );
        if me.open == Some(Open::Sources) {
            let mut body = col();
            for (key, type_id, name, icon) in sources {
                let main = row()
                    .items_center()
                    .gap(12.0)
                    .grow(1.0)
                    .min_w(0.0)
                    .child(
                        row()
                            .size(40.0, 40.0)
                            .radius(radius::MD)
                            .fill(t.hover)
                            .center()
                            .child(icon_text_el(s, &icon, 20.0, ty::ui(20.0))),
                    )
                    .child(
                        col()
                            .gap(2.0)
                            .min_w(0.0)
                            .shrink()
                            .child(text("Object Type").font(ty::ui(14.0)).color(t.fg))
                            .child(
                                text(name.clone())
                                    .font(ty::ui(13.0))
                                    .color(t.muted)
                                    .truncate(),
                            ),
                    );
                let main = if type_id.is_empty() {
                    main
                } else {
                    let id = object_id.to_string();
                    motion::pressable(main)
                        .key(format!("src-{key}"))
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut State| {
                            close(s, &id);
                            nav::navigate(Route::Object(type_id.clone()), s);
                        })
                };
                let (id, k) = (object_id.to_string(), key.clone());
                let remove = Button::new(format!("src-rm-{key}"))
                    .ghost()
                    .xsmall()
                    .px(4.0)
                    .text_color(t.muted)
                    .pointer()
                    .tooltip("Remove")
                    .child(hover_tint(glyph(Glyph::Close, 12.0).color(t.muted), t.fg))
                    .on_press(move |s: &mut State| {
                        let Some(object) = s.store.object(&id) else {
                            return;
                        };
                        let keys = source_keys(&object.fields)
                            .into_iter()
                            .filter(|x| *x != k)
                            .collect();
                        save_sources(s, &id, keys);
                    })
                    .build(t);
                body = body.child(
                    row()
                        .items_center()
                        .gap(8.0)
                        .padding(8.0)
                        .radius(radius::MD)
                        .fill(t.hl_med)
                        .child(main)
                        .child(el().hover_group().child(remove)),
                );
            }
            if let Some(state) = &me.suggest {
                let (lens_id, event_id) = (object_id.to_string(), object_id.to_string());
                body = body.child(
                    TypeSuggest::new(format!("meta-suggest-{object_id}"), state)
                        .on_state(move |s: &mut State| {
                            s.slot::<Metas>()
                                .0
                                .get_mut(&lens_id)
                                .and_then(|me| me.suggest.as_mut())
                                .expect("the source menu is open")
                        })
                        .on_event(move |s: &mut State, ev: SuggestEvent<String>| match ev {
                            SuggestEvent::Picked(key) => {
                                let Some(object) = s.store.object(&event_id) else {
                                    return;
                                };
                                let mut keys = source_keys(&object.fields);
                                keys.push(key);
                                save_sources(s, &event_id, keys);
                            }
                            SuggestEvent::Dismissed => close(s, &event_id),
                        })
                        .build(s, cx, t, layer),
                );
            }
            below(layer, "sources", 6.0, meta_pop(t, "sources-pop", body));
        }
    }

    let n = me.backlinks.len();
    if n > 0 {
        let id = object_id.to_string();
        // The count rolls as backlinks come and go.
        let label = motion::rolling_text(
            t,
            format!("backlinks-count-{object_id}"),
            format!("{n} backlink{}", if n == 1 { "" } else { "s" }),
            n as f64,
            badge_font(),
            badge_colors(t, "").fg,
        );
        line = line.child(
            badge_el(
                t,
                "backlinks",
                badge_glyph(t, Glyph::BadgeLink),
                label,
                true,
                "Backlinks",
            )
            .on_press(move |s: &mut State| toggle(s, &id, Open::Backlinks)),
        );
        if me.open == Some(Open::Backlinks) {
            let mut body = col().child(
                text("LINKED FROM")
                    .font(ty::ui(11.0))
                    .color(t.muted)
                    .mb(4.0),
            );
            for b in &me.backlinks {
                let (id, target) = (object_id.to_string(), b.id.clone());
                let icon = object_icon(store, &b.icon, &b.type_key, &space);
                body = body.child(
                    motion::pressable_row(row())
                        .key(format!("bl-{}", b.id))
                        .w_full()
                        .min_w(0.0)
                        .items_center()
                        .gap(8.0)
                        .px(8.0)
                        .py(5.0)
                        .radius(radius::SM)
                        .hover(t.kit.secondary_hover)
                        .press(t.kit.secondary_active)
                        .cursor(Cursor::Pointer)
                        .on_press(move |s: &mut State| {
                            close(s, &id);
                            nav::navigate(Route::Object(target.clone()), s);
                        })
                        .child(row().w(18.0).justify_center().child(icon_text_el(
                            s,
                            &icon,
                            13.0,
                            ty::ui(13.0),
                        )))
                        .child(
                            text(b.name.clone())
                                .font(ty::ui(13.0))
                                .color(t.fg)
                                .truncate(),
                        )
                        .child(el().grow(1.0))
                        .child(text(b.type_key.clone()).font(ty::ui(11.0)).color(t.muted)),
                );
            }
            below(layer, "backlinks", 6.0, meta_pop(t, "backlinks-pop", body));
        }
    }

    // Lite: backlinks above are only what this device holds.
    if let Some(status) = roostr_core::lite::status(s)
        && !status.is_full(&space)
    {
        let label = match status.loading(&space) {
            Some(load) => lite_ui::loading_text(load),
            None => lite_ui::LOAD_SPACE.to_string(),
        };
        let target = space.clone();
        line = line.child(
            badge(
                t,
                "backlinks-lite",
                badge_glyph(t, Glyph::BadgeLink),
                label,
                true,
                "Backlinks from what this device holds; load the space for all of them",
            )
            .on_press(move |s: &mut State| lite_ui::load_space(&target, s)),
        );
    }
    line
}

#[cfg(test)]
mod tests {
    use roostr_core::types::{QueryResult, Value};
    use roostr_ui::ThemePref;
    use rune::Cx;

    use super::*;
    use crate::test_support::{App, Vault, object};

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        let t = roostr_ui::theme(ThemePref::Dark, cx);
        let mut layer = vec![];
        let body = object_meta(s, cx, t, &mut layer, "page-1", false);
        roostr_ui::root(body, layer)
    }

    fn texts(app: &App) -> Vec<String> {
        app.rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect()
    }

    #[test]
    fn shows_type_and_backlinks() {
        let page = object("page-1", "page", &[("name", Value::string("Plan"))]);
        let a = object(
            "note-a",
            "note",
            &[
                ("name", Value::string("Alpha")),
                ("iconEmoji", Value::string("📝")),
            ],
        );
        let b = object("note-b", "note", &[("name", Value::string("Beta"))]);
        let vault = Vault::new(vec![page, a, b], vec![]);
        *vault.on_query.lock() = Some(Box::new(|v: &Vault, body| {
            body.to_string().contains("$links").then(|| {
                let records: Vec<_> = v
                    .objects
                    .lock()
                    .iter()
                    .filter(|o| o.id.starts_with("note-"))
                    .map(Vault::row)
                    .collect();
                QueryResult {
                    total: records.len() as u64,
                    records,
                }
            })
        }));
        let mut app = App::new(vault, view);
        app.until("the backlinks", |s| {
            s.store.object("page-1").is_some()
                && s.get::<Metas>()
                    .and_then(|m| m.0.get("page-1"))
                    .is_some_and(|me| me.backlinks.len() == 2)
        });
        assert!(app.shown("type"), "type badge");
        let shown = texts(&app);
        assert!(shown.iter().any(|t| t == "Page"), "type name: {shown:?}");
        assert!(shown.iter().any(|t| t == "2 backlinks"), "count: {shown:?}");

        app.click("backlinks");
        assert!(app.shown("bl-note-a") && app.shown("bl-note-b"));
        let shown = texts(&app);
        assert!(shown.iter().any(|t| t == "LINKED FROM"));
        assert!(shown.iter().any(|t| t == "Alpha") && shown.iter().any(|t| t == "Beta"));

        app.click("bl-note-a");
        assert!(!app.shown("backlinks-pop"), "picking a backlink closes");
        assert_eq!(*app.s().nav.route(), Route::Object("note-a".into()));
    }

    #[test]
    fn source_keys_skip_empty() {
        let fields = Fields::from([(
            "setOf".to_string(),
            Value::values(vec![Value::string("task"), Value::string("")]),
        )]);
        assert_eq!(source_keys(&fields), vec!["task".to_string()]);
    }
}
