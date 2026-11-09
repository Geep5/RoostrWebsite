//! The right "Chat & properties" pane (`ConversationDrawer.svelte`). On a desktop the shell
//! lays it out as two columns, Chat then Properties ([`pane_column`]), each with its name and
//! collapse button; on a phone it is one sheet ([`pane_sheet`]) with a Chat / Properties
//! switch, the collapse button and the active tab.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

use roostr_core::nav::Route;
use roostr_core::{PaneTab, State};
use roostr_rt::Task;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Alert, Button, HEADER_H, KitSize, Tab, TabBar, Theme, ty};
use rune::{Cx, El, ScrollSpec, col, el, row, text};

use crate::discussion::{Place, drop_instance, instance_key};
use crate::discussion_view::{instance, place_options, view};

/// Refetch of the open object while the pane is up (`refreshActive`).
const REFRESH_EVERY: Duration = Duration::from_secs(8);

/// Every object's pane state, by object id.
#[derive(Default)]
pub(crate) struct Drawers {
    map: HashMap<String, Drawer>,
    /// Panes a view asked for that the executor is creating.
    requested: std::cell::RefCell<std::collections::HashSet<String>>,
}

struct Drawer {
    /// False for objects without a discussion (agents, types, sets): properties only. The
    /// host says so on every draw (it knows once the object has loaded).
    chat: Cell<bool>,
    /// The sheet's picked tab.
    picked: Option<PaneTab>,
    /// Last drawn as the phone's sheet (else as desktop columns).
    in_sheet: Cell<bool>,
    refresh_error: String,
    refreshing: bool,
    _timer: Task<()>,
}

impl Drawer {
    /// The sheet's tab: Chat when the object has one and nothing else was picked; never a
    /// Chat tab without a chat.
    fn tab(&self) -> PaneTab {
        match self.picked {
            Some(PaneTab::Chat) if !self.chat.get() => PaneTab::Properties,
            Some(tab) => tab,
            None if self.chat.get() => PaneTab::Chat,
            None => PaneTab::Properties,
        }
    }
}

/// The pane of `object_id`, asking for it when it doesn't exist yet.
fn drawer<'a>(s: &'a State, object_id: &str, chat: bool) -> Option<&'a Drawer> {
    let drawers = s.get::<Drawers>();
    if let Some(d) = drawers.and_then(|d| d.map.get(object_id)) {
        return Some(d);
    }
    if drawers.is_none_or(|d| d.requested.borrow_mut().insert(object_id.to_string())) {
        let id = object_id.to_string();
        s.rt.spawn(async move |cx| {
            cx.update(|s| {
                s.slot::<Drawers>().requested.borrow_mut().remove(&id);
                if !s.slot::<Drawers>().map.contains_key(&id) {
                    create(s, id, chat);
                }
            })
        })
        .detach();
    }
    None
}

/// Makes `object_id`'s pane now, unless it exists.
pub(crate) fn open(s: &mut State, object_id: &str, chat: bool) {
    if !s.slot::<Drawers>().map.contains_key(object_id) {
        create(s, object_id.to_string(), chat);
    }
}

fn create(s: &mut State, object_id: String, chat: bool) {
    s.store.ensure_object(&object_id);
    let id = object_id.clone();
    let timer = s.rt.spawn(async move |cx| {
        loop {
            cx.timer(REFRESH_EVERY).await;
            let alive = cx.update(|s| {
                if s.get::<Drawers>().is_none_or(|d| !d.map.contains_key(&id)) {
                    return false;
                }
                refresh_active(s, &id);
                true
            });
            if !alive {
                return;
            }
        }
    });
    s.slot::<Drawers>().map.insert(
        object_id,
        Drawer {
            chat: Cell::new(chat),
            picked: None,
            in_sheet: Cell::new(false),
            refresh_error: String::new(),
            refreshing: false,
            _timer: timer,
        },
    );
}

/// Refetch the object; a failure shows above the tab with a Refresh button.
fn refresh_active(s: &mut State, object_id: &str) {
    let Some(d) = s.slot::<Drawers>().map.get_mut(object_id) else {
        return;
    };
    if d.refreshing {
        return;
    }
    d.refreshing = true;
    let task = s.store.fetch_object(object_id);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let Some(d) = s.slot::<Drawers>().map.get_mut(&id) else {
                return;
            };
            d.refreshing = false;
            match result {
                Ok(fresh) => {
                    d.refresh_error.clear();
                    let stale = s.store.object(&id).is_none_or(|cached| *cached != fresh);
                    if stale {
                        s.store.reload_object(&id);
                    }
                }
                Err(error) => d.refresh_error = error.to_string(),
            }
        });
    })
    .detach();
}

/// `State::on_poll`: a pane goes with its page or once nothing of it shows; its chat goes
/// while its column is collapsed or the sheet shows Properties (the web unmounts it, so a
/// returning chat lands at its newest message).
pub(crate) fn poll(s: &mut State) -> bool {
    let shown_object = s.nav.route().object_id().map(str::to_string);
    let up = s.pane.chat_open || s.pane.props_open || s.pane.sheet;
    let Some(drawers) = s.get::<Drawers>() else {
        return false;
    };
    let mut gone = Vec::new();
    let mut chat_gone = Vec::new();
    for (id, d) in &drawers.map {
        if shown_object.as_deref() != Some(id.as_str()) || !up {
            gone.push(id.clone());
            chat_gone.push(id.clone());
            continue;
        }
        let chat_shown = if d.in_sheet.get() {
            s.pane.sheet && d.tab() == PaneTab::Chat
        } else {
            s.pane.chat_open
        };
        if !chat_shown {
            chat_gone.push(id.clone());
        }
    }
    let mut changed = false;
    for id in chat_gone {
        let key = instance_key(&id, Place::Pane);
        if crate::discussion::get(s, &key).is_some() {
            drop_instance(s, &key);
            changed = true;
        }
    }
    let drawers = s.slot::<Drawers>();
    for id in gone {
        changed |= drawers.map.remove(&id).is_some();
    }
    changed
}

/// Drops `object_id`'s pane.
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Drawers>().map.remove(object_id);
}

/// Escape nobody else took on `object_id`'s page: desktop collapses a column; sheet:
/// Properties goes back to Chat, else the sheet closes.
pub fn on_escape(s: &mut State, object_id: &str, mobile: bool) -> bool {
    if !matches!(s.nav.route(), Route::Object(id) if id == object_id) {
        return false;
    }
    if !mobile {
        if !(s.pane.chat_open || s.pane.props_open) {
            return false;
        }
        let pane = &mut s.pane;
        pane.collapse_last(&mut s.ui);
        return true;
    }
    let back_to_chat = s
        .get::<Drawers>()
        .and_then(|d| d.map.get(object_id))
        .is_some_and(|d| d.chat.get() && d.tab() == PaneTab::Properties);
    if back_to_chat {
        if let Some(d) = s.slot::<Drawers>().map.get_mut(object_id) {
            d.picked = Some(PaneTab::Chat);
        }
        return true;
    }
    if !s.pane.sheet {
        return false;
    }
    s.pane.set_sheet(false);
    true
}

/// A Chat / Properties tab of the sheet's switch: its icon, then its name.
fn sheet_tab(s: &State, tab: PaneTab) -> Tab<State> {
    Tab::new()
        .prefix(text(tab.icon(s)).font(ty::ui(13.)).mr(5.))
        .label(tab.name())
}

/// The `»` button: closes the sheet, or collapses a desktop column.
fn collapse(t: &Theme, column: Option<PaneTab>) -> El {
    let (key, tip) = match column {
        None => ("collapse", "Collapse"),
        Some(PaneTab::Chat) => ("pane-collapse-Chat", "Collapse chat"),
        Some(PaneTab::Properties) => ("pane-collapse-Properties", "Collapse properties"),
    };
    Button::new(key)
        .outline()
        .small()
        .square(26.)
        .p(0.)
        .radius(7.)
        .text_color(t.muted)
        .tooltip(tip)
        .icon(roostr_ui::Glyph::ChevronsRight)
        .icon_size(15.)
        .on_press(move |s: &mut State| match column {
            None => s.pane.set_sheet(false),
            Some(tab) => {
                let pane = &mut s.pane;
                pane.set_open(tab, false, &mut s.ui);
            }
        })
        .build(t)
}

/// A collapsed column folded into the open one's header: its icon and name, sliding in from
/// the right; a press expands it again.
fn folded(s: &State, t: &Theme, tab: PaneTab) -> El {
    let name = tab.name();
    let button = Button::new(format!("pane-folded-{name}"))
        .outline()
        .small()
        .h(26.)
        .px(8.)
        .radius(7.)
        .text_color(t.muted)
        .tooltip(format!("Show {}", name.to_lowercase()))
        .on_press(move |s: &mut State| {
            let pane = &mut s.pane;
            pane.set_open(tab, true, &mut s.ui);
        })
        .child(text(tab.icon(s)).font(ty::ui(12.)))
        .child(text(name).font(ty::ui(12.)).color(t.muted))
        .build(t);
    button.transition(rune::Transition {
        insertion: rune::Effect {
            offset: rune::Vec2::new(24.0, 0.0),
            ..rune::Effect::opacity(0.0)
        },
        removal: rune::Effect::IDENTITY,
        springs: rune::SpringPair::new(
            roostr_ui::motion::HANDLE.with_duration(0.22).spring(),
            rune::Spring::NONE,
        ),
        ..rune::Transition::identity()
    })
}

/// A header row; the divider is inset like the object header's, not edge to edge.
fn head(t: &Theme, children: Vec<El>) -> El {
    row()
        .w_full()
        .center_y()
        .gap(10.)
        .h(HEADER_H)
        .px(14.)
        .pt(18.)
        .pb(8.)
        .children(children)
        .child(
            el().pin(Some(14.), None, Some(14.), Some(0.))
                .h(1.)
                .fill(t.border),
        )
}

fn refresh_error(t: &Theme, object_id: &str, error: &str) -> Option<El> {
    if error.is_empty() {
        return None;
    }
    let id = object_id.to_string();
    Some(
        row()
            .center_y()
            .gap(4.)
            .px(14.)
            .my(12.)
            .child(
                col().grow(1.).shrink().min_w(0.).child(
                    Alert::warning("refresh-error", error.to_string())
                        .size(KitSize::XSmall)
                        .text_size(12.)
                        .text_color(t.orange)
                        .build(t),
                ),
            )
            .child(
                Button::new("refresh")
                    .outline()
                    .xsmall()
                    .label("Refresh")
                    .on_press(move |s: &mut State| refresh_active(s, &id))
                    .build(t),
            ),
    )
}

/// What a tab shows: the discussion in its own scroller, or the properties in a scroll.
fn body(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    tab: PaneTab,
    window: [f32; 2],
) -> El {
    match tab {
        PaneTab::Chat => {
            let options = place_options(Place::Pane);
            let chat = match instance(s, object_id, &options) {
                Some(d) => view(d, s, cx, t, layer, window),
                None => col(),
            };
            col().grow(1.).min_h(0.).child(chat)
        }
        PaneTab::Properties => col()
            .key("props")
            .grow(1.)
            .min_h(0.)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .w_full()
                    .px(14.)
                    .py(8.)
                    .child(roostr_properties::properties_pane(
                        s, cx, t, layer, object_id,
                    )),
            ),
    }
}

/// A desktop column of `object_id`'s pane: its icon and name, the other column folded in
/// when collapsed, its collapse button, its view. A refresh failure shows in the first open
/// column. `chat` false: the object has no discussion.
pub fn pane_column(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    chat: bool,
    tab: PaneTab,
    window: [f32; 2],
) -> El {
    let Some(d) = drawer(s, object_id, chat) else {
        return col()
            .key(format!("pane-col-{}", tab.name()))
            .w_full()
            .h_full();
    };
    d.in_sheet.set(false);
    d.chat.set(chat);
    let pane = &s.pane;
    let other = tab.other();
    let first = tab == PaneTab::Properties || !pane.is_open(PaneTab::Properties);
    let fold = pane.has(other) && !pane.is_open(other);
    let mut items = vec![
        text(tab.icon(s)).font(ty::ui(13.)),
        text(tab.name())
            .font(ty::ui(12.5).semibold())
            .color(t.fg)
            .truncate()
            .grow(1.)
            .shrink(),
    ];
    // The other column, collapsed, folds into this header: a press opens it again.
    if fold {
        items.push(folded(s, t, other));
    }
    items.push(collapse(t, Some(tab)));
    let mut column = col()
        .key(format!("pane-col-{}", tab.name()))
        .w_full()
        .h_full()
        .child(head(t, items));
    if first {
        column = column.children(refresh_error(t, object_id, &d.refresh_error));
    }
    column.child(body(s, cx, t, layer, object_id, tab, window))
}

/// The phone's sheet of `object_id`'s pane: the Properties / Chat switch (the kit's
/// segmented tab bar, in the desktop columns' order), the collapse button, the picked tab.
pub fn pane_sheet(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    chat: bool,
    window: [f32; 2],
) -> El {
    let Some(d) = drawer(s, object_id, chat) else {
        return col().key("disc-sheet").w_full().h_full();
    };
    d.in_sheet.set(true);
    d.chat.set(chat);
    let tab = d.tab();
    let shown: Vec<PaneTab> = if chat {
        vec![PaneTab::Properties, PaneTab::Chat]
    } else {
        vec![PaneTab::Properties]
    };
    let selected = shown.iter().position(|x| *x == tab).unwrap_or(0);
    let id = object_id.to_string();
    let picks = shown.clone();
    let tabs = TabBar::new("pane-tabs")
        .segmented()
        .small()
        .fill(t.panel)
        .border(1., t.border)
        .radius(9.)
        .padding(2.)
        .children(shown.iter().map(|x| sheet_tab(s, *x)))
        .selected_index(selected)
        .on_click(move |s: &mut State, ix: usize| {
            if let (Some(tab), Some(d)) = (picks.get(ix), s.slot::<Drawers>().map.get_mut(&id)) {
                d.picked = Some(*tab);
            }
        })
        .build(t);
    col()
        .key("disc-sheet")
        .w_full()
        .h_full()
        .child(head(t, vec![tabs, collapse(t, None)]))
        .children(refresh_error(t, object_id, &d.refresh_error))
        .child(body(s, cx, t, layer, object_id, tab, window))
}
