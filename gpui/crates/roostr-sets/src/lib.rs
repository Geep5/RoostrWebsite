//! Query/set/collection/type data views: table, kanban, calendar, gallery,
//! query controls (the `.dataview` part of the object page).

mod calendar;
mod common;
mod config;
mod context_menu;
mod controls;
mod gallery;
mod kanban;
mod layout_icon;
mod table;
mod views;

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{IconName, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::default_type_key;
use roostr_core::filters::space_filter_of;
use roostr_core::relations::object_space_id;
use roostr_core::{Object, Store, StoreEvent, Value, api, field_str, lite_ui, theme, ui};
use serde_json::json;

use crate::calendar::CalendarView;
use crate::controls::{ControlsEvent, QueryControls};
use crate::gallery::GalleryView;
use crate::kanban::KanbanView;
use crate::table::{SetTable, TableEvent};
use crate::views::{ViewInput, ViewMsg};

actions!(
    roostr_sets,
    [
        /// Escape: close the dataview's menus, the cell editor and the selection.
        Dismiss
    ]
);

const CONTEXT: &str = "DataView";

/// Bind the data view's keys (Escape dismisses menus, editors and selection).
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

/// Which kind of object a [`DataView`] shows records for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataViewKind {
    /// A query / set: records matching its sources and filters.
    Query,
    /// A collection: its hand-picked members.
    Collection,
    /// A type page: every object of the type.
    Type,
}

enum ActiveView {
    Table(Entity<SetTable>),
    Gallery(Entity<GalleryView>),
    Kanban(Entity<KanbanView>),
    Calendar(Entity<CalendarView>),
}

impl ActiveView {
    fn kind(&self) -> &'static str {
        match self {
            Self::Table(_) => "table",
            Self::Gallery(_) => "gallery",
            Self::Kanban(_) => "kanban",
            Self::Calendar(_) => "calendar",
        }
    }
}

/// Controls + active view (table/gallery/kanban/calendar) for a query/set, collection or type object.
pub struct DataView {
    object_id: String,
    kind: DataViewKind,
    focus: FocusHandle,
    controls: Entity<QueryControls>,
    view: Option<ActiveView>,
    /// Live search text from the controls row.
    search: String,
    input: ViewInput,
    picking: bool,
    pick_query: Entity<InputState>,
    candidates: Vec<(String, String, String)>,
    reload_timer: Option<Task<()>>,
    /// The controls row + panels + collection bar: presses here never arm the marquee.
    chrome_bounds: common::BoundsCell,
    /// Lite: collection members last asked of the relays, so the refresh
    /// after the fetch does not ask again.
    fetched: Vec<String>,
    _subs: Vec<Subscription>,
}

impl DataView {
    /// The data view for `object_id` (loaded into the store cache on demand).
    pub fn new(
        object_id: String,
        kind: DataViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let controls = cx.new(|cx| QueryControls::new(object_id.clone(), kind, window, cx));
        let pick_query = cx.new(|cx| InputState::new(window, cx).placeholder("Search objects…"));
        let subs = vec![
            cx.observe_in(&store, window, |this, _, window, cx| this.sync(window, cx)),
            cx.subscribe_in(&store, window, |this, _, event: &StoreEvent, _, cx| {
                this.on_store_event(event, cx)
            }),
            cx.subscribe_in(
                &controls,
                window,
                |this, _, event: &ControlsEvent, window, cx| this.on_controls(event, window, cx),
            ),
            cx.observe(&pick_query, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            object_id,
            kind,
            focus: cx.focus_handle(),
            controls,
            view: None,
            search: String::new(),
            input: ViewInput {
                body: None,
                sorts: Vec::new(),
                updated_at: 0,
            },
            picking: false,
            pick_query,
            candidates: Vec::new(),
            reload_timer: None,
            chrome_bounds: common::bounds_cell(),
            fetched: Vec::new(),
            _subs: subs,
        };
        this.sync(window, cx);
        this
    }

    /// Forward a press on the page around the data view (its margins, the
    /// space below it) so the table's rubber band can start there, like the
    /// web's page-wide selection provider. Call it from the object page's
    /// `on_mouse_down(MouseButton::Left, ..)`. Presses with cmd/ctrl/shift,
    /// other buttons, non-table views and the view's own controls are
    /// ignored, and so are presses that focused something on their way up
    /// (the title, a property field): the web provider leaves inputs alone,
    /// and taking focus here would take it from the field just tapped. A
    /// plain click (no drag) clears the selection. The view takes keyboard
    /// focus so Escape clears the selection after.
    pub fn page_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = event.modifiers;
        if event.button != MouseButton::Left
            || m.platform
            || m.control
            || m.shift
            || self.chrome_bounds.get().contains(&event.position)
            || window.default_prevented()
        {
            return;
        }
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus, cx);
        }
        if let Some(ActiveView::Table(t)) = &self.view {
            t.update(cx, |t, cx| t.start_marquee(event.position, cx));
        }
    }

    fn collection_id(&self) -> Option<String> {
        (self.kind == DataViewKind::Collection).then(|| self.object_id.clone())
    }

    /// Recompute the record query and hand it to the controls and the active
    /// view; swap the view when `viewType` changed.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = common::cached_object(&self.object_id, cx) else {
            return;
        };
        self.lite(&object, cx);
        let relations = common::scoped_relations(&object, cx);
        let body = common::table_body(&object, self.kind, &self.search, &relations, cx);
        let input = ViewInput {
            body: body.clone(),
            sorts: config::view_sorts(&object.fields),
            updated_at: object.updated_at,
        };
        self.controls.update(cx, |c, _| c.set_body(body));
        let view_type = config::view_type(&object.fields).to_string();
        if self.view.as_ref().map(ActiveView::kind) != Some(view_type.as_str()) {
            self.view = Some(self.make_view(&view_type, input.clone(), window, cx));
        } else if input != self.input {
            match &self.view {
                Some(ActiveView::Table(v)) => v.update(cx, |v, cx| v.set_input(input.clone(), cx)),
                Some(ActiveView::Gallery(v)) => {
                    v.update(cx, |v, cx| v.set_input(input.clone(), cx))
                }
                Some(ActiveView::Kanban(v)) => v.update(cx, |v, cx| v.set_input(input.clone(), cx)),
                Some(ActiveView::Calendar(v)) => {
                    v.update(cx, |v, cx| v.set_input(input.clone(), cx))
                }
                None => {}
            }
        }
        self.input = input;
        cx.notify();
    }

    /// Lite: an opened query/set or type page over a space not held whole
    /// downloads that space (like its pinned widget; the refresh when it
    /// lands reloads the rows); a collection fetches its members.
    fn lite(&mut self, object: &Object, cx: &mut Context<Self>) {
        if roostr_core::lite::status(cx).is_none() {
            return;
        }
        if self.kind == DataViewKind::Collection {
            let ids = config::member_ids(&object.fields);
            if !ids.is_empty() && ids != self.fetched {
                self.fetched = ids.clone();
                lite_ui::fetch(ids, cx);
            }
            return;
        }
        let space = object_space_id(Store::global(cx).read(cx), &object.fields);
        lite_ui::ensure_space(&space, cx);
    }

    fn make_view(
        &mut self,
        view_type: &str,
        input: ViewInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ActiveView {
        let id = self.object_id.clone();
        let collection = self.collection_id();
        match view_type {
            "gallery" => {
                ActiveView::Gallery(cx.new(|cx| GalleryView::new(id, collection, input, cx)))
            }
            "kanban" => {
                let v = cx.new(|cx| KanbanView::new(id, collection, input, cx));
                self._subs
                    .push(cx.subscribe(&v, |_, _, _: &ViewMsg, cx| cx.notify()));
                ActiveView::Kanban(v)
            }
            "calendar" => ActiveView::Calendar(cx.new(|cx| CalendarView::new(id, input, cx))),
            _ => {
                let v = cx.new(|cx| SetTable::new(id, collection, input, window, cx));
                self._subs.push(cx.subscribe(
                    &v,
                    |this, table, event: &TableEvent, cx| match event {
                        TableEvent::Create(name) => {
                            table.update(cx, |t, _| t.set_committing(true));
                            let task = this
                                .controls
                                .update(cx, |c, cx| c.create_record(name.clone(), None, cx));
                            cx.spawn(async move |_, cx| {
                                task.await;
                                table.update(cx, |t, cx| t.created(cx));
                            })
                            .detach();
                        }
                    },
                ));
                ActiveView::Table(v)
            }
        }
    }

    /// The page's commit listener: this object's changes (and a reconnect)
    /// refresh the records; so do changes to records on screen.
    fn on_store_event(&mut self, event: &StoreEvent, cx: &mut Context<Self>) {
        let relevant = match event {
            StoreEvent::Committed(id) => *id == self.object_id || self.shows(id, cx),
            StoreEvent::Reconnected => true,
            StoreEvent::Refreshed => false,
        };
        if !relevant {
            return;
        }
        // Bursts collapse into one reload.
        self.reload_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| this.reload(cx));
        }));
    }

    fn shows(&self, id: &str, cx: &App) -> bool {
        match &self.view {
            Some(ActiveView::Table(v)) => v.read(cx).row_ids().any(|r| r == id),
            Some(ActiveView::Gallery(v)) => v.read(cx).row_ids().any(|r| r == id),
            Some(ActiveView::Kanban(v)) => v.read(cx).row_ids().any(|r| r == id),
            Some(ActiveView::Calendar(v)) => v.read(cx).row_ids().any(|r| r == id),
            None => false,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_timer = None;
        match &self.view {
            Some(ActiveView::Table(v)) => v.update(cx, |v, cx| v.reload(cx)),
            Some(ActiveView::Gallery(v)) => v.update(cx, |v, cx| v.load(cx)),
            Some(ActiveView::Kanban(v)) => v.update(cx, |v, cx| v.load(cx)),
            Some(ActiveView::Calendar(v)) => v.update(cx, |v, cx| v.load(cx)),
            None => {}
        }
    }

    fn on_controls(&mut self, event: &ControlsEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ControlsEvent::Search(q) => {
                self.search = q.clone();
                self.sync(window, cx);
            }
            ControlsEvent::NewInline => {
                if let Some(ActiveView::Table(t)) = &self.view {
                    t.update(cx, |t, cx| t.begin_new(window, cx));
                }
            }
            ControlsEvent::Created(id) => self.on_record_created(id.clone(), cx),
        }
    }

    /// Table-view New: the record joins the view (collection membership included).
    fn on_record_created(&mut self, id: String, cx: &mut Context<Self>) {
        let table = match &self.view {
            Some(ActiveView::Table(t)) => Some(t.clone()),
            _ => None,
        };
        let join = (self.kind == DataViewKind::Collection).then(|| {
            let mut members = common::cached_object(&self.object_id, cx)
                .map(|o| config::member_ids(&o.fields))
                .unwrap_or_default();
            members.push(id);
            common::set_members(&self.object_id, &members, cx)
        });
        cx.spawn(async move |_, cx| {
            if let Some(join) = join {
                let _ = join.await;
            }
            if let Some(table) = table {
                table.update(cx, |t, cx| t.reload(cx));
            }
        })
        .detach();
    }

    /// Every window-level Escape handler of the web page runs: the collection
    /// picker, the controls' menus, the table's editor/selection/menu.
    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.picking = false;
        self.controls.update(cx, |c, cx| c.dismiss(cx));
        match &self.view {
            Some(ActiveView::Table(t)) => t.update(cx, |t, cx| {
                t.dismiss(cx);
            }),
            Some(ActiveView::Gallery(v)) => v.update(cx, |v, cx| v.dismiss(cx)),
            Some(ActiveView::Kanban(v)) => v.update(cx, |v, cx| v.dismiss(cx)),
            _ => {}
        }
        cx.notify();
    }

    // ── Collection bar ───────────────────────────────────────────

    fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking {
            self.picking = false;
            cx.notify();
            return;
        }
        let Some(object) = common::cached_object(&self.object_id, cx) else {
            return;
        };
        self.pick_query
            .update(cx, |s, cx| s.set_value("", window, cx));
        let default = Store::global(cx)
            .read(cx)
            .channels
            .first()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        // Self-contained spaces: only this space's objects are addable.
        let task = api::query(
            json!({"filters": [space_filter_of(&object.fields, &default)], "limit": 200}),
            cx,
        );
        let members = config::member_ids(&object.fields);
        let current = object.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(res) = task.await else { return };
            const HIDDEN: &[&str] = &[
                "program",
                "typescript",
                "json",
                "proto",
                "relation",
                "collection",
                "query",
                "set",
                "type",
                "template",
                "agent",
                "skill",
            ];
            let candidates = res
                .records
                .iter()
                .filter(|r| {
                    r.id != current
                        && !members.contains(&r.id)
                        && !HIDDEN.contains(&r.type_key.as_str())
                })
                .map(|r| {
                    let name = field_str(&r.fields, "name");
                    let name = if name.is_empty() {
                        r.id.chars().take(8).collect()
                    } else {
                        name.to_string()
                    };
                    (r.id.clone(), name, r.type_key.clone())
                })
                .collect();
            let _ = this.update_in(cx, |this, window, cx| {
                this.candidates = candidates;
                this.picking = true;
                this.pick_query.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Anytype collection New: create an object of the space's Default type and link it.
    fn new_in_collection(&mut self, cx: &mut Context<Self>) {
        let Some(object) = common::cached_object(&self.object_id, cx) else {
            return;
        };
        let ch = field_str(&object.fields, "channel").to_string();
        let members = config::member_ids(&object.fields);
        let type_task = default_type_key(&ch, cx);
        let collection = self.object_id.clone();
        cx.spawn(async move |_, cx| {
            let type_key = type_task.await;
            let mut fields = roostr_core::Fields::new();
            if !ch.is_empty() {
                fields.insert("channel".into(), Value::string(&ch));
            }
            let create = cx.update(|cx| roostr_core::create::from_type("", &type_key, fields, cx));
            let Ok(id) = create.await else { return };
            let mut next = members;
            next.push(id.clone());
            let _ = cx
                .update(|cx| common::set_members(&collection, &next, cx))
                .await;
            cx.update(|cx| common::open_object(&id, cx));
        })
        .detach();
    }

    fn add_member(&mut self, id: String, cx: &mut Context<Self>) {
        self.picking = false;
        let mut members = common::cached_object(&self.object_id, cx)
            .map(|o| config::member_ids(&o.fields))
            .unwrap_or_default();
        members.push(id);
        common::set_members(&self.object_id, &members, cx).detach_and_log_err(cx);
        cx.notify();
    }

    fn render_collection_bar(&mut self, members: usize, cx: &mut Context<Self>) -> Div {
        let mut out = div().flex().flex_col();
        out = out.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .mb(px(12.))
                .child(
                    common::new_button("col-new", false)
                        .on_click(cx.listener(|this, _, _, cx| this.new_in_collection(cx))),
                )
                .child(
                    ui::outline_button("col-add", "+ Add object")
                        .bg(theme::panel())
                        .px(px(12.))
                        .py(px(6.))
                        .on_click(cx.listener(|this, _, window, cx| this.open_picker(window, cx))),
                )
                .when(members > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(format!("{members} object(s)")),
                    )
                }),
        );
        if self.picking {
            let q = self.pick_query.read(cx).value().trim().to_lowercase();
            let shown: Vec<&(String, String, String)> = self
                .candidates
                .iter()
                .filter(|c| q.is_empty() || c.1.to_lowercase().contains(&q))
                .collect();
            let mut list = div()
                .id("picker-list")
                .flex()
                .flex_col()
                .gap(px(4.))
                .max_h(px(260.))
                .overflow_y_scroll();
            for (id, name, type_key) in &shown {
                let pick = id.clone();
                list = list.child(
                    common::vrow(SharedString::from(format!("pick-{id}")), false)
                        .bg(theme::panel())
                        .rounded(px(8.))
                        .px(px(12.))
                        .child(
                            div().flex().gap(px(4.)).child(name.clone()).child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child(type_key.clone()),
                            ),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.add_member(pick.clone(), cx)),
                        ),
                );
            }
            if shown.is_empty() {
                let text = if self.candidates.is_empty() {
                    "Nothing to add."
                } else {
                    "No matches."
                };
                list = list.child(common::empty_state(text, 12.));
            }
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(10.))
                    .p(px(10.))
                    .mb(px(14.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.pick_query).text_size(px(13.))),
                            )
                            .child(
                                Button::new("picker-close")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .tooltip("Close (Esc)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.picking = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(list),
            );
        }
        out
    }
}

impl Focusable for DataView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DataView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = common::cached_object(&self.object_id, cx) else {
            return div().track_focus(&self.focus);
        };
        let members = config::member_ids(&object.fields).len();
        let mut root = div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                    // Anytype's rubber band arms anywhere in the view except its
                    // chrome (controls row, panels, collection bar); interactive
                    // table parts (header, entry row, buttons) stop the press first.
                    let m = e.modifiers;
                    if m.platform
                        || m.control
                        || m.shift
                        || this.chrome_bounds.get().contains(&e.position)
                    {
                        return;
                    }
                    if let Some(ActiveView::Table(t)) = &this.view {
                        t.update(cx, |t, cx| t.start_marquee(e.position, cx));
                    }
                }),
            )
            .flex()
            // Children go records → chrome → overlay, displayed bottom-up: the
            // chrome's menus and the row menu paint above the records.
            .flex_col_reverse()
            .text_color(theme::fg())
            // CSS `line-height: normal` for the system font.
            .line_height(relative(1.2));
        if self.input.body.is_some() {
            root = match &self.view {
                Some(ActiveView::Table(v)) => root.child(v.clone()),
                Some(ActiveView::Gallery(v)) => root.child(v.clone()),
                Some(ActiveView::Kanban(v)) => root.child(v.clone()),
                Some(ActiveView::Calendar(v)) => root.child(v.clone()),
                None => root,
            };
        } else if self.kind == DataViewKind::Collection {
            root = root.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("Empty collection — add objects."),
            );
        }
        let mut chrome = div()
            .relative()
            .flex()
            .flex_col()
            .child(common::track_bounds(&self.chrome_bounds));
        if self.kind == DataViewKind::Collection {
            chrome = chrome.child(self.render_collection_bar(members, cx));
        }
        // Lite: the space downloading (or its button, after a failed try).
        if self.kind != DataViewKind::Collection {
            let space = object_space_id(Store::global(cx).read(cx), &object.fields);
            chrome = chrome.children(lite_ui::space_notice(
                format!("view-{}", self.object_id),
                &space,
                cx,
            ));
        }
        root = root.child(chrome.child(self.controls.clone()));
        let menu = match &self.view {
            Some(ActiveView::Table(v)) => v.read(cx).ctx_menu(),
            Some(ActiveView::Gallery(v)) => v.read(cx).ctx_menu(),
            Some(ActiveView::Kanban(v)) => v.read(cx).ctx_menu(),
            _ => None,
        };
        let _ = window;
        // An absolutely positioned overlay: its place in the column is irrelevant.
        root.when_some(menu, |d, m| {
            d.child(div().absolute().top_0().left_0().child(m))
        })
    }
}
