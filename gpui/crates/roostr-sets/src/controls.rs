//! The dataview controls row (QueryControls.svelte): view chip, Group by /
//! Date chip, search, filter / sort panels, the view settings menu (layout
//! tiles + property list), and the accent New button (+ template picker).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _, Toggle, ToggleGroup};
use gpui_kit::component::input::{Escape as InputEscape, Input, InputEvent, InputState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, IconName, IconNamed as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::filters::space_filter_of;
use roostr_core::motion::{self, LayoutGroup, Motions};
use roostr_core::relations::{create_relation, current_space_id, format_glyph, object_space_id};
use roostr_core::{Glyph, Object, RelationDef, Store, Value, api, field_str, theme};
use roostr_properties::KeyPicker;
use serde_json::{Value as Json, json};

use crate::DataViewKind;
use crate::common::{
    self, BoundsCell, Place, appear, bounds_cell, floating, inside_any, kit_menu, kit_row,
    kit_title, menu_surface, menu_title, new_button, separator, tag_pill, track_bounds, vrow,
    vrow_content,
};
use crate::config::{self, FilterRule, SortRule, ViewProp};
use crate::layout_icon::layout_icon;

/// What the controls tell the data view.
pub(crate) enum ControlsEvent {
    /// The search text changed (live filter).
    Search(String),
    /// Table views: the accent New opens the in-table entry row.
    NewInline,
    /// Table views: a record was created by the entry row / New.
    Created(String),
}

/// A KeyPicker label function.
type KeyLabel = Rc<dyn Fn(&str, &App) -> SharedString>;

const LAYOUTS: &[(&str, &str)] = &[
    ("table", "Table"),
    ("gallery", "Gallery"),
    ("kanban", "Kanban"),
    ("calendar", "Calendar"),
];
const USED_SAMPLE: u64 = 500;
const SYSTEM_DATES: &[(&str, &str)] = &[
    ("createdDate", "Created date"),
    ("modifiedDate", "Modified date"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Filter,
    Sort,
}

/// Inputs of one filter rule's value editor.
struct RuleInputs {
    single: Entity<InputState>,
    multi: Entity<InputState>,
}

/// Payload of a sort-rule drag.
#[derive(Clone)]
struct SortDrag(usize);

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(crate) struct QueryControls {
    object_id: String,
    kind: DataViewKind,
    /// The view's record query (ranks the properties its records use first).
    body: Option<Json>,
    search_open: bool,
    search: Entity<InputState>,
    open: Panel,
    sort_more: Option<usize>,
    sort_drag: Option<usize>,
    sort_over: Option<usize>,
    settings_open: bool,
    prop_list_open: bool,
    chip_open: bool,
    used_keys: HashSet<String>,
    creating: bool,
    type_templates: Vec<(String, String)>,
    tpl_pick_open: bool,
    /// "＋ New … property" name entry (the web's `prompt`).
    new_prop: Option<Entity<InputState>>,
    cond_open: Option<usize>,
    rule_inputs: HashMap<usize, RuleInputs>,
    settings_bounds: BoundsCell,
    vmenu_bounds: BoundsCell,
    /// The view menu's row whose property list opens beside it.
    vsub_bounds: BoundsCell,
    plist_bounds: BoundsCell,
    chip_bounds: BoundsCell,
    tpl_bounds: BoundsCell,
    /// Positions of the filter / sort rules: they slide to their new place
    /// after a reorder, an add or a removal.
    rules_layout: LayoutGroup,
    /// Rules entering, keyed by [`rule_ids`].
    rule_motions: Motions<SharedString>,
    /// The open rules panel and how many rules it showed last frame: rules
    /// past that were just added and enter. `None` while no panel is open.
    rules_seen: Option<(Panel, usize)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ControlsEvent> for QueryControls {}

impl QueryControls {
    pub(crate) fn new(
        object_id: String,
        kind: DataViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let subs = vec![cx.subscribe(&search, |_, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let q = input.read(cx).value().to_string();
                cx.emit(ControlsEvent::Search(q));
            }
        })];
        let mut this = Self {
            object_id,
            kind,
            body: None,
            search_open: false,
            search,
            open: Panel::None,
            sort_more: None,
            sort_drag: None,
            sort_over: None,
            settings_open: false,
            prop_list_open: false,
            chip_open: false,
            used_keys: HashSet::new(),
            creating: false,
            type_templates: Vec::new(),
            tpl_pick_open: false,
            new_prop: None,
            cond_open: None,
            rule_inputs: HashMap::new(),
            settings_bounds: bounds_cell(),
            vsub_bounds: bounds_cell(),
            vmenu_bounds: bounds_cell(),
            plist_bounds: bounds_cell(),
            chip_bounds: bounds_cell(),
            tpl_bounds: bounds_cell(),
            rules_layout: LayoutGroup::default(),
            rule_motions: Motions::default(),
            rules_seen: None,
            _subs: subs,
        };
        this.load_templates(cx);
        this
    }

    pub(crate) fn set_body(&mut self, body: Option<Json>) {
        self.body = body;
    }

    fn object(&self, cx: &App) -> Option<std::sync::Arc<Object>> {
        common::cached_object(&self.object_id, cx)
    }

    /// Close the floating menus (Escape).
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.settings_open
            || self.chip_open
            || self.tpl_pick_open
            || self.sort_more.is_some()
            || self.cond_open.is_some();
        self.settings_open = false;
        self.chip_open = false;
        self.prop_list_open = false;
        self.tpl_pick_open = false;
        self.sort_more = None;
        self.cond_open = None;
        self.new_prop = None;
        cx.notify();
        any
    }

    // ── Templates on New (type pages) ─────────────────────────────

    fn load_templates(&mut self, cx: &mut Context<Self>) {
        if self.kind != DataViewKind::Type {
            return;
        }
        let task = api::query(
            json!({"type": "template", "filters": [{"key": "target_type", "condition": "equal", "value": self.object_id}], "limit": 50}),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(res) = task.await else { return };
            let list = res
                .records
                .iter()
                .map(|r| (r.id.clone(), field_str(&r.fields, "name").to_string()))
                .collect();
            let _ = this.update(cx, |this, cx| {
                this.type_templates = list;
                cx.notify();
            });
        })
        .detach();
    }

    // ── Saving the view config ───────────────────────────────────

    fn save_filters(&mut self, next: Vec<FilterRule>, cx: &mut Context<Self>) {
        api::note::set_field(
            &self.object_id,
            "viewFilters",
            config::filters_value(&next),
            cx,
        )
        .detach_and_log_err(cx);
    }

    fn save_sorts(&mut self, next: Vec<SortRule>, cx: &mut Context<Self>) {
        api::note::set_field(&self.object_id, "viewSorts", config::sorts_value(&next), cx)
            .detach_and_log_err(cx);
    }

    fn update_filter(
        &mut self,
        idx: usize,
        key: Option<String>,
        condition: Option<String>,
        value: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let relations = common::scoped_relations(&object, cx);
        let mut next = config::parse_filters(&object.fields);
        let Some(rule) = next.get_mut(idx) else {
            return;
        };
        if let Some(v) = value {
            rule.value = v;
        }
        if let Some(c) = condition.clone() {
            rule.condition = c;
        }
        // Reset condition/value when the key's format changes the catalog.
        if let Some(k) = key {
            rule.condition = config::conditions_for(&relations, &k)[0].id.to_string();
            rule.key = k;
            rule.value.clear();
        }
        // A one-value condition keeps one value.
        if let Some(c) = condition
            && !config::multi_value(&c)
            && rule.value.len() > 1
        {
            rule.value.truncate(1);
        }
        self.rule_inputs.remove(&idx);
        self.save_filters(next, cx);
    }

    // ── View layout ──────────────────────────────────────────────

    /// Property keys this view's objects - or its source types' objects - hold a value for.
    fn load_used_keys(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let type_key = field_str(&object.fields, "key").to_string();
        let types: Vec<String> = if self.kind == DataViewKind::Type {
            if type_key.is_empty() {
                vec![]
            } else {
                vec![type_key]
            }
        } else {
            config::sources(&object.fields)
        };
        let body_task = self
            .body
            .as_ref()
            .map(|b| api::query(common::engine_body(b, json!({"limit": USED_SAMPLE})), cx));
        let types_task = (!types.is_empty()).then(|| {
            let space = space_filter_of(&object.fields, &current_space_id(cx));
            api::query(
                json!({"filters": [{"key": "typeKey", "condition": "in", "value": types}, space], "limit": USED_SAMPLE}),
                cx,
            )
        });
        cx.spawn(async move |this, cx| {
            let mut keys = HashSet::new();
            for task in [body_task, types_task].into_iter().flatten() {
                if let Ok(res) = task.await {
                    for r in res.records {
                        for (k, v) in r.fields {
                            if config::has_value(&v) {
                                keys.insert(k);
                            }
                        }
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.used_keys = keys;
                cx.notify();
            });
        })
        .detach();
    }

    fn group_choices(&self, relations: &[RelationDef]) -> (Vec<ViewProp>, Vec<ViewProp>) {
        let rank = |f: &str| match f {
            "status" => Some(0),
            "tag" => Some(1),
            "checkbox" => Some(2),
            _ => None,
        };
        let mut all: Vec<&RelationDef> = relations
            .iter()
            .filter(|r| !r.hidden && rank(&r.format).is_some())
            .collect();
        all.sort_by(|a, b| {
            rank(&a.format).cmp(&rank(&b.format)).then_with(|| {
                let an = if a.name.is_empty() { &a.key } else { &a.name };
                let bn = if b.name.is_empty() { &b.key } else { &b.name };
                an.to_lowercase().cmp(&bn.to_lowercase())
            })
        });
        let props: Vec<ViewProp> = all.into_iter().map(view_prop).collect();
        props
            .into_iter()
            .partition(|p| self.used_keys.contains(&p.key))
    }

    fn date_choices(&self, relations: &[RelationDef]) -> (Vec<ViewProp>, Vec<ViewProp>) {
        let mut dates: Vec<ViewProp> = relations
            .iter()
            .filter(|r| {
                !r.hidden && r.format == "date" && !SYSTEM_DATES.iter().any(|(k, _)| *k == r.key)
            })
            .map(view_prop)
            .collect();
        dates.sort_by_key(|p| p.name.to_lowercase());
        let stamps = SYSTEM_DATES.iter().map(|(key, name)| {
            relations
                .iter()
                .find(|r| r.key == *key)
                .map(view_prop)
                .unwrap_or_else(|| ViewProp {
                    key: (*key).into(),
                    name: (*name).into(),
                    emoji: format_glyph("date").into(),
                })
        });
        let (used, rest): (Vec<ViewProp>, Vec<ViewProp>) = dates
            .into_iter()
            .partition(|p| self.used_keys.contains(&p.key));
        (used.into_iter().chain(stamps).collect(), rest)
    }

    /// Layout tile click. Kanban and calendar open their property list every time.
    fn pick_layout(&mut self, v: &'static str, cx: &mut Context<Self>) {
        self.prop_list_open = false;
        let choosing = v == "kanban" || v == "calendar";
        let save = api::note::set_field(&self.object_id, "viewType", Value::string(v), cx);
        if choosing {
            self.load_used_keys(cx);
        }
        let object_id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            let _ = save.await;
            let _ = this.update(cx, |this, cx| {
                if v == "kanban"
                    && let Some(object) = this.object(cx)
                {
                    // A board needs a group property: the top of the list stands in, saved.
                    let relations = common::scoped_relations(&object, cx);
                    let (used, rest) = this.group_choices(&relations);
                    let group = config::group_key(&object.fields).to_string();
                    let all: Vec<ViewProp> = used.into_iter().chain(rest).collect();
                    if let Some(first) = all.first()
                        && !all.iter().any(|p| p.key == group)
                    {
                        api::note::set_field(
                            &object_id,
                            "viewGroupKey",
                            Value::string(&first.key),
                            cx,
                        )
                        .detach_and_log_err(cx);
                    }
                }
                this.prop_list_open = choosing;
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_prop(&mut self, key: String, cx: &mut Context<Self>) {
        self.prop_list_open = false;
        self.chip_open = false;
        self.new_prop = None;
        let Some(object) = self.object(cx) else {
            return;
        };
        let field = if config::view_type(&object.fields) == "calendar" {
            "viewDateKey"
        } else {
            "viewGroupKey"
        };
        api::note::set_field(&self.object_id, field, Value::string(key), cx).detach_and_log_err(cx);
        cx.notify();
    }

    /// "＋ New … property": name entry, then create the relation and pick it.
    fn begin_new_prop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let format = if config::view_type(&object.fields) == "calendar" {
            "date"
        } else {
            "tag"
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(format!("New {format} property name:"))
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let name = input.read(cx).value().trim().to_string();
                    this.new_prop = None;
                    if name.is_empty() {
                        cx.notify();
                        return;
                    }
                    let Some(object) = this.object(cx) else {
                        return;
                    };
                    let space = object_space_id(Store::global(cx).read(cx), &object.fields);
                    let task = create_relation(&name, format, &[], &space, cx);
                    cx.spawn(async move |this, cx| {
                        if let Ok(rel) = task.await {
                            let _ = this.update(cx, |this, cx| this.pick_prop(rel.key, cx));
                        }
                    })
                    .detach();
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.new_prop = Some(input);
        cx.notify();
    }

    // ── New record ───────────────────────────────────────────────

    /// Create a record inheriting the view: filters seed fields, calendar
    /// seeds the date, the page's space is stamped. `template`: `None` =
    /// the type's default, `Some(None)` = Blank, `Some(Some(id))` = that one.
    pub(crate) fn create_record(
        &mut self,
        name: String,
        template: Option<Option<String>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.creating {
            return Task::ready(());
        }
        let Some(object) = self.object(cx) else {
            return Task::ready(());
        };
        self.creating = true;
        cx.notify();
        let relations = common::scoped_relations(&object, cx);
        let channel = field_str(&object.fields, "channel").to_string();
        let sources = config::sources(&object.fields);
        let type_key = if self.kind == DataViewKind::Type {
            Some(field_str(&object.fields, "key"))
                .filter(|k| !k.is_empty())
                .unwrap_or("note")
                .to_string()
        } else {
            sources.first().cloned().unwrap_or_else(|| "note".into())
        };
        let view_type = config::view_type(&object.fields).to_string();
        let date_key = config::date_key(&object.fields).to_string();
        let seeds = roostr_core::view_seeds::seeds_from_view(&object.fields, &relations);
        let kind = self.kind;
        let default_tpl = if kind == DataViewKind::Type {
            field_str(&object.fields, "default_template_id").to_string()
        } else {
            Store::global(cx)
                .read(cx)
                .types
                .iter()
                .find(|t| t.key == type_key)
                .map(|t| t.default_template_id.clone())
                .unwrap_or_default()
        };
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<Option<String>> = async {
                // An agent named in the entry row is a row like any other.
                if type_key == "agent" && !name.trim().is_empty() {
                    cx.update(|cx| {
                        roostr_core::create::new_agent(&channel, &name, seeds.clone(), cx)
                    })
                    .await?;
                    return Ok(None);
                }
                // Infra types are never bare records.
                if matches!(type_key.as_str(), "machine" | "agent" | "file") {
                    cx.update(|cx| {
                        roostr_core::create::create_typed(&type_key, &channel, &name, cx)
                    })
                    .await?;
                    return Ok(None);
                }
                let mut fields = seeds;
                if !channel.is_empty() {
                    fields.insert("channel".into(), Value::string(&channel));
                }
                if view_type == "calendar" && !date_key.is_empty() {
                    fields.insert(date_key.clone(), Value::int(config::now_ms()));
                }
                let id = cx
                    .update(|cx| api::note::create(&name, Some(&type_key), Some(fields), cx))
                    .await?;
                let tpl = match template {
                    Some(explicit) => explicit.unwrap_or_default(),
                    None => default_tpl,
                };
                if !tpl.is_empty() {
                    let _ = cx
                        .update(|cx| roostr_core::create::apply_template(&id, &tpl, cx))
                        .await;
                }
                Ok(Some(id))
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                cx.notify();
                match result {
                    // Anytype's table New edits the record in place; other views open it.
                    Ok(Some(id)) if view_type == "table" => cx.emit(ControlsEvent::Created(id)),
                    Ok(Some(id)) => common::open_object(&id, cx),
                    Ok(None) => {}
                    Err(error) => log::error!("create record: {error:#}"),
                }
            });
        })
    }

    fn new_record(&mut self, cx: &mut Context<Self>) {
        let table = self
            .object(cx)
            .is_some_and(|o| config::view_type(&o.fields) == "table");
        if table {
            cx.emit(ControlsEvent::NewInline);
        } else {
            self.create_record(String::new(), None, cx).detach();
        }
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = !self.search_open;
        if self.search_open {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
            cx.emit(ControlsEvent::Search(String::new()));
        }
        cx.notify();
    }

    fn sort_reorder(&mut self, cx: &mut Context<Self>) {
        let (Some(from), Some(to)) = (self.sort_drag.take(), self.sort_over.take()) else {
            self.sort_drag = None;
            self.sort_over = None;
            cx.notify();
            return;
        };
        if from == to {
            cx.notify();
            return;
        }
        let Some(object) = self.object(cx) else {
            return;
        };
        let mut next = config::parse_sorts(&object.fields);
        if from >= next.len() || to >= next.len() {
            return;
        }
        let moved = next.remove(from);
        next.insert(to, moved);
        self.save_sorts(next, cx);
    }

    // ── Rendering ────────────────────────────────────────────────

    /// Whether the open rules panel gained rules since its last frame: those
    /// were just added and enter (opening the panel or re-keying a rule does
    /// not animate).
    fn rules_entering(&self, panel: Panel, len: usize) -> bool {
        matches!(self.rules_seen, Some((open, seen)) if open == panel && len > seen)
    }

    /// Record the rules `panel` showed and forget removed ones (a removal is
    /// immediate).
    fn rules_settled(&mut self, panel: Panel, ids: &[SharedString]) {
        self.rules_seen = Some((panel, ids.len()));
        self.rule_motions.retain(|id| ids.contains(id));
    }

    /// Rule `id`'s row: a rule just added enters (opacity 0 → 1 while it drops
    /// 4px, 150ms), and every rule slides to its new place when the rules
    /// reorder or a sibling comes or goes.
    fn rule_row(
        &mut self,
        id: &SharedString,
        entering: bool,
        row: impl IntoElement + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut enter =
            motion::Motion::new(motion::Target::default()).transition(motion::CONTROL.transition());
        if entering && !self.rule_motions.contains_key(id) {
            enter = enter.initial(motion::Target::default().y(-4.).opacity(0.));
        }
        let frame = self.rule_motions.frame_one(id.clone(), enter, window, cx);
        // Wrapped: the row keeps its own opacity (a dragged rule is dimmed).
        self.rules_layout
            .position(id.clone(), frame.apply(div().child(row)))
            .transition(motion::CONTROL.transition())
            .into_any_element()
    }

    /// The rules panel. Its rules measure their slides from it, so the page
    /// moving the panel (the content above it growing) slides nothing.
    fn rules_panel(&self, panel: Div) -> AnyElement {
        self.rules_layout
            .position("rules-panel", panel)
            .enabled(false)
            .into_any_element()
    }

    /// A controls-row toggle (`.cbtn`): the kit toggle, on = the accent glyph.
    fn cbtn(&self, id: &'static str, glyph: Glyph, on: bool) -> Toggle {
        Toggle::new(id)
            .checked(on)
            .icon(Icon::new(glyph).size(px(20.)))
            .flex_none()
            .size(px(28.))
            .min_w(px(28.))
            .p_0()
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(if on { theme::accent() } else { theme::muted() })
            // On reads as the accent glyph alone, not the kit's pressed fill.
            .when(on, |t| t.bg(transparent_black()))
    }

    fn prop_caption(&self, current: Option<&ViewProp>, current_key: &str) -> Div {
        match current {
            Some(p) => div()
                .flex()
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .min_w(px(18.))
                        .mr(px(6.))
                        .flex()
                        .justify_center()
                        .child(p.emoji.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(p.name.clone()),
                ),
            None => div().child(if current_key.is_empty() {
                "Select".to_string()
            } else {
                current_key.to_string()
            }),
        }
    }

    /// The board/calendar property list: used properties, a divider, the rest, "＋ New … property".
    fn prop_list(
        &self,
        title: &'static str,
        calendar: bool,
        used: &[ViewProp],
        rest: &[ViewProp],
        current_key: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let row = |p: &ViewProp, cx: &mut Context<Self>| {
            let key = p.key.clone();
            vrow(SharedString::from(format!("plist-{}", p.key)), false)
                .child(vrow_content(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(18.))
                                .mr(px(6.))
                                .flex()
                                .justify_center()
                                .child(p.emoji.clone()),
                        )
                        .child(p.name.clone()),
                    (p.key == current_key).then(|| {
                        div()
                            .text_color(theme::accent())
                            .child("✓")
                            .into_any_element()
                    }),
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.pick_prop(key.clone(), cx)))
        };
        let mut scroll = div()
            .id("plist-scroll")
            .flex()
            .flex_col()
            .gap(px(2.))
            .max_h(px(320.))
            .overflow_y_scroll();
        for p in used {
            scroll = scroll.child(row(p, cx));
        }
        if !used.is_empty() && !rest.is_empty() {
            scroll = scroll.child(separator());
        }
        for p in rest {
            scroll = scroll.child(row(p, cx));
        }
        let new_entry: AnyElement = match &self.new_prop {
            Some(input) => div()
                .px(px(4.))
                .child(Input::new(input).text_size(px(13.)))
                .into_any_element(),
            None => vrow("plist-new", false)
                .child(format!(
                    "＋ New {} property…",
                    if calendar { "date" } else { "tag" }
                ))
                .on_click(cx.listener(|this, _, window, cx| this.begin_new_prop(window, cx)))
                .into_any_element(),
        };
        menu_surface()
            .relative()
            .w(px(274.))
            .occlude()
            .child(track_bounds(&self.plist_bounds))
            .child(menu_title(title))
            .child(scroll)
            .child(separator())
            .child(new_entry)
    }

    fn render_filter_panel(
        &mut self,
        object: &Object,
        relations: &Rc<Vec<RelationDef>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filters = config::parse_filters(&object.fields);
        let keys = config::filter_keys(relations);
        let ids = rule_ids("filter", filters.iter().map(|f| f.key.as_str()));
        let entering = self.rules_entering(Panel::Filter, filters.len());
        let mut panel = panel_box();
        for ((i, f), id) in filters.iter().enumerate().zip(&ids) {
            let rels = relations.clone();
            let label: KeyLabel = Rc::new(move |k, _| config::option_label(&rels, k).into());
            let mut rule = div().flex().flex_wrap().items_center().gap(px(8.)).child(
                KeyPicker::new(("fkey", i), f.key.clone(), keys.clone(), label).on_pick(
                    cx.listener(move |this, key: &str, _, cx| {
                        this.update_filter(i, Some(key.to_string()), None, None, cx)
                    }),
                ),
            );
            rule = rule.child(self.condition_select(i, f, relations, window, cx));
            if config::needs_value(relations, f) {
                let options = config::options_for(relations, &f.key);
                if !options.is_empty() {
                    let mut tags = div().flex().flex_wrap().gap(px(6.));
                    for opt in options {
                        let on = f.value.contains(&opt);
                        let color = config::option_color(relations, &f.key, &opt);
                        let current = f.value.clone();
                        let o = opt.clone();
                        tags = tags.child(
                            Toggle::new(SharedString::from(format!("ftag-{i}-{opt}")))
                                .checked(on)
                                .p_0()
                                .h_auto()
                                .min_w_0()
                                .rounded(px(999.))
                                .border_1()
                                .border_color(if on {
                                    theme::accent()
                                } else {
                                    transparent_black()
                                })
                                .bg(transparent_black())
                                .opacity(if on { 1. } else { 0.45 })
                                .cursor_pointer()
                                .child(
                                    tag_pill(opt.clone(), &color, false, theme::fg())
                                        .px(px(10.))
                                        .rounded(px(999.)),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let next = if current.contains(&o) {
                                        current.iter().filter(|v| **v != o).cloned().collect()
                                    } else {
                                        let mut n = current.clone();
                                        n.push(o.clone());
                                        n
                                    };
                                    this.update_filter(i, None, None, Some(next), cx)
                                })),
                        );
                    }
                    rule = rule.child(tags);
                } else {
                    let inputs = self.rule_inputs(i, f, window, cx);
                    if config::multi_value(&f.condition) {
                        // Several values: every one is shown and removable.
                        let mut multi = div().flex().flex_wrap().items_center().gap(px(6.));
                        for (j, v) in f.value.iter().enumerate() {
                            let current = f.value.clone();
                            multi = multi.child(
                                Tag::custom(theme::border(), theme::fg(), theme::border())
                                    .rounded_full()
                                    .gap(px(4.))
                                    .pl(px(10.))
                                    .pr(px(4.))
                                    .py(px(2.))
                                    .text_size(px(12.))
                                    .child(v.clone())
                                    .child(
                                        x_button(("fval-x", i * 1000 + j), "Remove value")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let next = current
                                                    .iter()
                                                    .enumerate()
                                                    .filter(|(k, _)| *k != j)
                                                    .map(|(_, v)| v.clone())
                                                    .collect();
                                                this.update_filter(i, None, None, Some(next), cx)
                                            })),
                                    ),
                            );
                        }
                        let placeholder = if f.value.is_empty() {
                            "value"
                        } else {
                            "add another"
                        };
                        inputs
                            .multi
                            .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
                        multi = multi.child(div().w(px(140.)).child(field_input(&inputs.multi)));
                        rule = rule.child(multi);
                    } else {
                        rule = rule.child(div().w(px(160.)).child(field_input(&inputs.single)));
                    }
                }
            }
            let all = filters.clone();
            rule = rule.child(x_button(("fx", i), "Remove filter").on_click(cx.listener(
                move |this, _, _, cx| {
                    let next = all
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, f)| f.clone())
                        .collect();
                    this.rule_inputs.clear();
                    this.save_filters(next, cx);
                },
            )));
            panel = panel.child(self.rule_row(id, entering, rule, window, cx));
        }
        self.rules_settled(Panel::Filter, &ids);
        let all = filters.clone();
        let panel = panel.child(add_row(
            "add-filter",
            "+ Add filter",
            cx.listener(move |this, _, _, cx| {
                let mut next = all.clone();
                next.push(FilterRule {
                    key: "name".into(),
                    condition: "like".into(),
                    value: vec![],
                });
                this.save_filters(next, cx);
            }),
        ));
        self.rules_panel(panel)
    }

    /// The condition `<select>`: the current label, a dropdown of the key's catalog.
    fn condition_select(
        &self,
        i: usize,
        f: &FilterRule,
        relations: &[RelationDef],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let conds = config::conditions_for(relations, &f.key);
        let label = conds
            .iter()
            .find(|c| c.id == f.condition)
            .map_or(f.condition.as_str(), |c| c.label)
            .to_string();
        let open = self.cond_open == Some(i);
        let mut host = div().relative().child(
            Button::new(("fcond", i))
                .outline()
                .selected(open)
                .dropdown_caret(true)
                .h_auto()
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .bg(theme::panel())
                .border_color(if open {
                    theme::accent()
                } else {
                    theme::border()
                })
                .text_color(theme::fg())
                .cursor_pointer()
                .child(div().text_size(px(13.)).child(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cond_open = if this.cond_open == Some(i) {
                        None
                    } else {
                        Some(i)
                    };
                    cx.notify();
                })),
        );
        if open {
            let key = (
                i,
                f.condition.clone(),
                conds.iter().map(|c| c.id).collect::<Vec<_>>(),
            );
            let this = cx.weak_entity();
            let current = f.condition.clone();
            let dismiss = this.clone();
            let menu = kit_menu(
                ("fcond-kit", i),
                key,
                window,
                cx,
                |menu, _, _| {
                    conds.iter().fold(menu.min_w(px(160.)), |menu, c| {
                        let id = c.id.to_string();
                        let this = this.clone();
                        menu.item(kit_row(c.label, c.id == current).on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.cond_open = None;
                                this.update_filter(i, None, Some(id.clone()), None, cx)
                            });
                        }))
                    })
                },
                move |_, cx| {
                    let _ = dismiss.update(cx, |this, cx| {
                        if this.cond_open == Some(i) {
                            this.cond_open = None;
                            cx.notify();
                        }
                    });
                },
            );
            let menu = appear(("fcond-menu", i), div().child(menu), window, cx);
            host = host.child(floating(Place::BelowLeft, 4., menu));
        }
        host
    }

    /// The value inputs of rule `i`, created on first use and kept in sync with the stored value.
    fn rule_inputs(
        &mut self,
        i: usize,
        f: &FilterRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &RuleInputs {
        if !self.rule_inputs.contains_key(&i) {
            let first = f.value.first().cloned().unwrap_or_default();
            let single = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("value")
                    .default_value(first)
            });
            let multi = cx.new(|cx| InputState::new(window, cx).placeholder("value"));
            let s1 = cx.subscribe_in(
                &single,
                window,
                move |this, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let text = input.read(cx).value().to_string();
                        let Some(object) = this.object(cx) else {
                            return;
                        };
                        let current = config::parse_filters(&object.fields)
                            .get(i)
                            .map(|f| f.value.clone())
                            .unwrap_or_default();
                        let next = if text.is_empty() { vec![] } else { vec![text] };
                        if next != current {
                            this.update_filter(i, None, None, Some(next), cx);
                        }
                    }
                },
            );
            let s2 = cx.subscribe_in(
                &multi,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    let raw = input.read(cx).value().to_string();
                    let commit = match event {
                        InputEvent::PressEnter { .. } | InputEvent::Blur => true,
                        InputEvent::Change => raw.ends_with(','),
                        InputEvent::Focus => false,
                    };
                    if !commit {
                        return;
                    }
                    let add = raw.trim_end_matches(',').trim().to_string();
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                    if add.is_empty() {
                        return;
                    }
                    let Some(object) = this.object(cx) else {
                        return;
                    };
                    let mut current = config::parse_filters(&object.fields)
                        .get(i)
                        .map(|f| f.value.clone())
                        .unwrap_or_default();
                    if !current.contains(&add) {
                        current.push(add);
                        this.update_filter(i, None, None, Some(current), cx);
                    }
                },
            );
            self._subs.push(s1);
            self._subs.push(s2);
            self.rule_inputs.insert(i, RuleInputs { single, multi });
        }
        let inputs = &self.rule_inputs[&i];
        let stored = f.value.first().cloned().unwrap_or_default();
        let focused = inputs.single.read(cx).focus_handle(cx).is_focused(window);
        if !focused && inputs.single.read(cx).value() != stored.as_str() {
            inputs
                .single
                .update(cx, |s, cx| s.set_value(stored, window, cx));
        }
        &self.rule_inputs[&i]
    }

    fn render_sort_panel(
        &mut self,
        object: &Object,
        relations: &Rc<Vec<RelationDef>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sorts = config::parse_sorts(&object.fields);
        let keys = config::filter_keys(relations);
        let ids = rule_ids("sort", sorts.iter().map(|s| s.key.as_str()));
        let entering = self.rules_entering(Panel::Sort, sorts.len());
        let mut panel = panel_box().gap(px(2.));
        if sorts.is_empty() {
            panel = panel.child(
                common::empty_state("No sorts applied", 12.)
                    .px(px(4.))
                    .py(px(2.)),
            );
        }
        for ((i, s), id) in sorts.iter().enumerate().zip(&ids) {
            let rels = relations.clone();
            let label: KeyLabel = Rc::new(move |k, _| config::option_label(&rels, k).into());
            let all = sorts.clone();
            let all2 = sorts.clone();
            let all3 = sorts.clone();
            // `.dnd` and `.row-btns` reveal on the row's hover (`opacity 0.1s`).
            let (reveal, hover) = motion::hover(
                SharedString::from(format!("{id}-hover")),
                motion::HOVER_REVEAL,
                window,
                cx,
            );
            // `.sort-arrow`: one ↑ turned 180° for Desc (`transition: transform 0.15s`).
            let turn = motion::progress(
                SharedString::from(format!("{id}-dir")),
                s.desc,
                motion::CONTROL,
                window,
                cx,
            );
            let dragging = self.sort_drag == Some(i);
            let over = self.sort_over == Some(i) && self.sort_drag.is_some_and(|d| d != i);
            let more_open = self.sort_more == Some(i);
            let mut more = div().relative().child(
                Button::new(("sort-more", i))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("More")
                    .selected(more_open)
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sort_more = if this.sort_more == Some(i) {
                            None
                        } else {
                            Some(i)
                        };
                        cx.notify();
                    })),
            );
            if more_open {
                let this = cx.weak_entity();
                let pick = |empty_start: bool| {
                    let (this, sorts) = (this.clone(), sorts.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let next = sorts
                            .iter()
                            .enumerate()
                            .map(|(j, x)| {
                                if j == i {
                                    SortRule {
                                        empty_start,
                                        ..x.clone()
                                    }
                                } else {
                                    x.clone()
                                }
                            })
                            .collect();
                        let _ = this.update(cx, |this, cx| {
                            this.sort_more = None;
                            this.save_sorts(next, cx);
                        });
                    }
                };
                let dismiss = this.clone();
                let menu = kit_menu(
                    ("sort-more-kit", i),
                    (i, sorts.clone()),
                    window,
                    cx,
                    |menu, _, _| {
                        menu.min_w(px(174.))
                            .item(kit_title("Show empty"))
                            .item(kit_row("At the top", s.empty_start).on_click(pick(true)))
                            .item(kit_row("At the bottom", !s.empty_start).on_click(pick(false)))
                    },
                    move |_, cx| {
                        let _ = dismiss.update(cx, |this, cx| {
                            if this.sort_more == Some(i) {
                                this.sort_more = None;
                                cx.notify();
                            }
                        });
                    },
                );
                more = more.child(floating(
                    Place::BelowRight,
                    4.,
                    appear(("sort-more-menu", i), div().child(menu), window, cx),
                ));
            }
            let row = div()
                .id(("sort-item", i))
                .on_hover(move |hovered, _, cx| motion::HoverMotion::set(&hover, *hovered, cx))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .py(px(3.))
                .rounded(px(8.))
                .when(dragging, |d| d.opacity(0.4))
                .when(over, |d| d.border_t_2().border_color(theme::accent()))
                .on_drag(SortDrag(i), |_, _, _, cx| cx.new(|_| DragGhost))
                .on_drag_move(
                    cx.listener(move |this, e: &DragMoveEvent<SortDrag>, _, cx| {
                        let from = e.drag(cx).0;
                        if e.bounds.contains(&e.event.position) && this.sort_over != Some(i) {
                            this.sort_drag = Some(from);
                            this.sort_over = Some(i);
                            cx.notify();
                        }
                    }),
                )
                .on_drop(cx.listener(|this, _: &SortDrag, _, cx| this.sort_reorder(cx)))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .cursor_grab()
                        .opacity(0.7 * reveal)
                        .child("⋮⋮"),
                )
                .child(
                    div()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(8.))
                        .px(px(8.))
                        .py(px(3.))
                        .child(
                            KeyPicker::new(("skey", i), s.key.clone(), keys.clone(), label)
                                .on_pick(cx.listener(move |this, key: &str, _, cx| {
                                    let next = all
                                        .iter()
                                        .enumerate()
                                        .map(|(j, x)| {
                                            if j == i {
                                                SortRule {
                                                    key: key.to_string(),
                                                    ..x.clone()
                                                }
                                            } else {
                                                x.clone()
                                            }
                                        })
                                        .collect();
                                    this.save_sorts(next, cx);
                                })),
                        ),
                )
                .child(
                    Button::new(("sort-dir", i))
                        .outline()
                        .h_auto()
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(8.))
                        .border_color(theme::border())
                        .bg(transparent_black())
                        .text_color(theme::fg())
                        .cursor_pointer()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(13.))
                                .child(
                                    svg()
                                        .path(Glyph::SortArrow.path())
                                        .size(px(16.))
                                        .flex_none()
                                        .text_color(theme::fg())
                                        .with_transformation(Transformation::rotate(radians(
                                            turn * std::f32::consts::PI,
                                        ))),
                                )
                                .child(if s.desc { "Desc" } else { "Asc" }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let next = all2
                                .iter()
                                .enumerate()
                                .map(|(j, x)| {
                                    if j == i {
                                        SortRule {
                                            desc: !x.desc,
                                            ..x.clone()
                                        }
                                    } else {
                                        x.clone()
                                    }
                                })
                                .collect();
                            this.save_sorts(next, cx);
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .ml_auto()
                        .opacity(if more_open { 1. } else { reveal })
                        .child(more)
                        .child(x_button(("sort-x", i), "Delete sort").on_click(cx.listener(
                            move |this, _, _, cx| {
                                let next = all3
                                    .iter()
                                    .enumerate()
                                    .filter(|(j, _)| *j != i)
                                    .map(|(_, x)| x.clone())
                                    .collect();
                                this.save_sorts(next, cx);
                            },
                        ))),
                );
            panel = panel.child(self.rule_row(id, entering, row, window, cx));
        }
        self.rules_settled(Panel::Sort, &ids);
        let all = sorts.clone();
        panel = panel.child(separator()).child(add_row(
            "add-sort",
            "＋ Add sort",
            cx.listener(move |this, _, _, cx| {
                let mut next = all.clone();
                next.push(SortRule {
                    key: "name".into(),
                    desc: false,
                    empty_start: false,
                });
                this.save_sorts(next, cx);
            }),
        ));
        if !sorts.is_empty() {
            panel = panel.child(add_row(
                "clear-sort",
                "Delete sort",
                cx.listener(|this, _, _, cx| this.save_sorts(vec![], cx)),
            ));
        }
        self.rules_panel(panel)
    }
}

/// Each rule's identity under `prefix`: its property key, numbered among the
/// rules sharing it. Motion follows the rule when the rules reorder (Svelte
/// keyed them by index).
fn rule_ids<'a>(prefix: &str, keys: impl IntoIterator<Item = &'a str>) -> Vec<SharedString> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    keys.into_iter()
        .map(|key| {
            let n = seen.entry(key).or_default();
            let id = format!("{prefix}-{key}#{n}");
            *n += 1;
            id.into()
        })
        .collect()
}

fn view_prop(r: &RelationDef) -> ViewProp {
    ViewProp {
        key: r.key.clone(),
        name: if r.name.is_empty() {
            r.key.clone()
        } else {
            r.name.clone()
        },
        emoji: r
            .icon_emoji
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| format_glyph(&r.format).to_string()),
    }
}

fn panel_box() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .border_1()
        .border_color(theme::border())
        .rounded(px(10.))
        .p(px(10.))
        .mb(px(12.))
        .text_size(px(13.))
}

/// A muted text action of a rules panel ("+ Add filter", "Delete sort"): a
/// kit ghost button at its own width.
fn add_row(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .self_start()
        // The kit sizes a label itself (12px at xsmall): the text sits in a child.
        .child(div().text_size(px(13.)).child(label))
        .text_color(theme::muted())
        .cursor_pointer()
        .on_click(on_click)
}

/// A muted kit ghost "×" removing a value, a rule or a sort.
fn x_button(id: impl Into<ElementId>, label: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(IconName::Close)
        .accessibility_label(label)
        .text_color(theme::muted())
        .cursor_pointer()
}

/// A bordered filter value input (`select, input` style).
fn field_input(state: &Entity<InputState>) -> Input {
    Input::new(state).text_size(px(13.))
}

impl Render for QueryControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return div();
        };
        let relations = Rc::new(common::scoped_relations(&object, cx));
        let view_type = config::view_type(&object.fields).to_string();
        let calendar = view_type == "calendar";
        let chooses_prop = view_type == "kanban" || calendar;
        let prop_title: &'static str = if calendar { "Date" } else { "Group by" };
        let (used, rest) = if calendar {
            self.date_choices(&relations)
        } else {
            self.group_choices(&relations)
        };
        let current_key = if calendar {
            config::date_key(&object.fields)
        } else {
            config::group_key(&object.fields)
        }
        .to_string();
        let current = used
            .iter()
            .chain(rest.iter())
            .find(|p| p.key == current_key)
            .cloned();
        let filters_on = !config::parse_filters(&object.fields).is_empty();
        let sorts_on = !config::parse_sorts(&object.fields).is_empty();
        let view_label = {
            let mut c = view_type.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        };

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mb(px(10.))
            .child(div().flex_1());
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(layout_icon(&view_type, 16., theme::muted()))
                .child(view_label),
        );
        if chooses_prop {
            let title = if calendar {
                "Choose the property this calendar places records by"
            } else {
                "Choose the property this board groups by"
            };
            let mut chip = div()
                .relative()
                .flex()
                .min_w_0()
                .max_w(px(260.))
                .child(track_bounds(&self.chip_bounds))
                .child(
                    Button::new("prop-chip")
                        .outline()
                        .selected(self.chip_open)
                        .dropdown_caret(true)
                        .flex_shrink(1.)
                        .min_w_0()
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(8.))
                        .border_color(theme::border())
                        .bg(if self.chip_open {
                            theme::hl_med()
                        } else {
                            transparent_black()
                        })
                        .text_color(theme::fg())
                        .cursor_pointer()
                        .tooltip(title)
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .text_size(px(12.))
                                .whitespace_nowrap()
                                // `.pchip-title`: hidden at ≤720px.
                                .when(!roostr_core::layout::is_mobile(window), |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_color(theme::muted())
                                            .mr(px(6.))
                                            .child(format!("{prop_title}:")),
                                    )
                                })
                                .child(self.prop_caption(current.as_ref(), &current_key)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.chip_open = !this.chip_open;
                            this.settings_open = false;
                            if this.chip_open {
                                this.load_used_keys(cx);
                            }
                            cx.notify();
                        })),
                );
            if self.chip_open {
                let chip_bounds = self.chip_bounds.clone();
                let plist_bounds = self.plist_bounds.clone();
                let list = self
                    .prop_list(prop_title, calendar, &used, &rest, &current_key, cx)
                    .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        if !inside_any(&[&chip_bounds, &plist_bounds], e.position) {
                            this.chip_open = false;
                            this.new_prop = None;
                            cx.notify();
                        }
                    }));
                let list = appear("prop-chip-list", list, window, cx);
                chip = chip.child(floating(Place::BelowLeft, 6., list));
            }
            row = row.child(chip);
        }
        if self.search_open {
            let search = self.search.clone();
            row = row.child(
                div()
                    .w(px(202.))
                    // The Input binds Escape itself: catch its action first.
                    .capture_action(cx.listener(|this, _: &InputEscape, window, cx| {
                        cx.stop_propagation();
                        this.toggle_search(window, cx);
                    }))
                    .child(Input::new(&search).text_size(px(13.))),
            );
        }
        row = row
            .child(
                self.cbtn("dv-search", Glyph::DvSearch, self.search_open)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_search(window, cx))),
            )
            .child(
                self.cbtn(
                    "dv-filter",
                    Glyph::DvFilter,
                    filters_on || self.open == Panel::Filter,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open = if this.open == Panel::Filter {
                        Panel::None
                    } else {
                        Panel::Filter
                    };
                    cx.notify();
                })),
            )
            .child(
                self.cbtn(
                    "dv-sort",
                    Glyph::DvSort,
                    sorts_on || self.open == Panel::Sort,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open = if this.open == Panel::Sort {
                        Panel::None
                    } else {
                        Panel::Sort
                    };
                    cx.notify();
                })),
            );

        // View settings: layout tiles, then the layout's property row.
        let mut settings = div()
            .relative()
            .child(track_bounds(&self.settings_bounds))
            .child(
                self.cbtn("dv-settings", Glyph::DvSettings, self.settings_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = !this.settings_open;
                        this.prop_list_open = false;
                        this.chip_open = false;
                        cx.notify();
                    })),
            );
        if self.settings_open {
            // The layout tiles: one kit toggle each, the current one checked.
            let current_layout = LAYOUTS.iter().position(|(v, _)| view_type == *v);
            let mut tiles = ToggleGroup::new("layouts")
                .w_full()
                .gap(px(4.))
                .on_click(cx.listener(move |this, next: &Vec<bool>, _, cx| {
                    // The tile whose state flipped is the one pressed.
                    let pressed = next
                        .iter()
                        .enumerate()
                        .position(|(ix, on)| *on != (Some(ix) == current_layout));
                    if let Some(ix) = pressed {
                        this.pick_layout(LAYOUTS[ix].0, cx);
                    }
                }));
            for (v, label) in LAYOUTS {
                let active = view_type == *v;
                let ink = if active { theme::fg() } else { theme::muted() };
                tiles = tiles.child(
                    Toggle::new(SharedString::from(format!("layout-{v}")))
                        .checked(active)
                        .flex_1()
                        .flex_col()
                        .min_w_0()
                        .h_auto()
                        .gap(px(2.))
                        .border_1()
                        .border_color(if active {
                            theme::accent()
                        } else {
                            transparent_black()
                        })
                        .rounded(px(8.))
                        .pt(px(4.))
                        .px(px(2.))
                        .pb(px(6.))
                        .text_size(px(12.))
                        .text_color(ink)
                        .cursor_pointer()
                        .when(active, |t| t.bg(transparent_black()))
                        .child(layout_icon(v, 44., ink))
                        .label(*label),
                );
            }
            let mut vmenu = menu_surface()
                .relative()
                .min_w(px(314.))
                .occlude()
                .child(track_bounds(&self.vmenu_bounds));
            if chooses_prop {
                vmenu = vmenu.child(
                    tiles
                        .pb(px(6.))
                        .mb(px(4.))
                        .border_b_1()
                        .border_color(theme::border()),
                );
                let mut sub = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(track_bounds(&self.vsub_bounds))
                    .child(
                        vrow("vsub-row", self.prop_list_open)
                            .child(vrow_content(
                                prop_title,
                                Some(
                                    div()
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.))
                                        .text_color(theme::muted())
                                        .whitespace_nowrap()
                                        .child(self.prop_caption(current.as_ref(), &current_key))
                                        .child(div().ml(px(4.)).child("›"))
                                        .into_any_element(),
                                ),
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prop_list_open = !this.prop_list_open;
                                if this.prop_list_open {
                                    this.load_used_keys(cx);
                                }
                                cx.notify();
                            })),
                    );
                if self.prop_list_open {
                    // `.plist` is 274px wide (offsetWidth).
                    let place =
                        Place::beside(self.vsub_bounds.get(), px(274.), window.viewport_size());
                    let list = self.prop_list(prop_title, calendar, &used, &rest, &current_key, cx);
                    let list = appear("vsub-list", list, window, cx);
                    sub = sub.child(floating(
                        place,
                        if place == Place::BelowLeft { 6. } else { 12. },
                        list,
                    ));
                }
                vmenu = vmenu.child(sub);
            } else {
                vmenu = vmenu.child(tiles);
            }
            let (sb, vb, pb) = (
                self.settings_bounds.clone(),
                self.vmenu_bounds.clone(),
                self.plist_bounds.clone(),
            );
            let prop_open = self.prop_list_open;
            vmenu = vmenu.on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let inside = inside_any(&[&sb, &vb], e.position)
                    || (prop_open && inside_any(&[&pb], e.position));
                if !inside {
                    this.settings_open = false;
                    this.prop_list_open = false;
                    this.new_prop = None;
                    cx.notify();
                }
            }));
            let vmenu = appear("view-menu", vmenu, window, cx);
            settings = settings.child(floating(Place::BelowRight, 6., vmenu));
        }
        row = row.child(settings);

        // New (+ template chevron on type pages).
        let split = self.kind == DataViewKind::Type && !self.type_templates.is_empty();
        let mut new_wrap = div()
            .relative()
            .flex()
            .child(track_bounds(&self.tpl_bounds))
            .child(
                new_button("new-btn", self.creating)
                    .when(split, |d| d.rounded_r(px(0.)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.creating {
                            this.new_record(cx)
                        }
                    })),
            );
        if split {
            new_wrap = new_wrap.child(
                Button::new("new-chev")
                    .primary()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .accessibility_label("New from template…")
                    .w(px(22.))
                    .h(px(28.))
                    .rounded(px(8.))
                    .rounded_l(px(0.))
                    .border_l_1()
                    .border_color(hsla(0., 0., 1., 0.25))
                    .cursor_pointer()
                    .tooltip("New from template…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tpl_pick_open = !this.tpl_pick_open;
                        cx.notify();
                    })),
            );
            if self.tpl_pick_open {
                let default_tpl = field_str(&object.fields, "default_template_id").to_string();
                let this = cx.weak_entity();
                let create = |tpl: Option<String>| {
                    let this = this.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = this.update(cx, |this, cx| {
                            this.tpl_pick_open = false;
                            this.create_record(String::new(), Some(tpl.clone()), cx)
                                .detach();
                        });
                    }
                };
                let templates = self.type_templates.clone();
                let tb = self.tpl_bounds.clone();
                let dismiss = this.clone();
                let pick = kit_menu(
                    "tpl-kit",
                    (default_tpl.clone(), templates.clone()),
                    window,
                    cx,
                    |menu, _, _| {
                        let menu = menu
                            .min_w(px(170.))
                            .item(kit_row("Blank", false).on_click(create(None)));
                        templates.iter().fold(menu, |menu, (id, name)| {
                            let label = format!(
                                "{}{}",
                                if name.is_empty() { "Untitled" } else { name },
                                if *id == default_tpl { " ★" } else { "" }
                            );
                            menu.item(kit_row(label, false).on_click(create(Some(id.clone()))))
                        })
                    },
                    move |window, cx| {
                        // A press on New/▾ is theirs: the chevron toggles the picker.
                        if !window.last_input_was_keyboard()
                            && inside_any(&[&tb], window.mouse_position())
                        {
                            return;
                        }
                        let _ = dismiss.update(cx, |this, cx| {
                            this.tpl_pick_open = false;
                            cx.notify();
                        });
                    },
                );
                let pick = appear("tpl-pick", div().child(pick), window, cx);
                new_wrap = new_wrap.child(floating(Place::BelowRight, 4., pick));
            }
        }
        row = row.child(new_wrap);

        let mut root = div().flex().flex_col().child(row);
        match self.open {
            Panel::Filter => {
                root = root.child(self.render_filter_panel(&object, &relations, window, cx))
            }
            Panel::Sort => {
                root = root.child(self.render_sort_panel(&object, &relations, window, cx))
            }
            Panel::None => {
                self.rules_seen = None;
                self.rule_motions.clear();
            }
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::rule_ids;

    #[test]
    fn rule_ids_follow_rules_through_a_reorder() {
        let ids = |keys: &[&str]| -> Vec<String> {
            rule_ids("sort", keys.iter().copied())
                .into_iter()
                .map(String::from)
                .collect()
        };
        assert_eq!(
            ids(&["name", "status", "name"]),
            ["sort-name#0", "sort-status#0", "sort-name#1"]
        );
        assert_eq!(ids(&["status", "name"]), ["sort-status#0", "sort-name#0"]);
    }
}
