//! The desktop shell frame (`+layout.svelte` desk markup + CSS 1869-2840):
//! boot states, tab strip, the row of rail | sidebar | main | right pane,
//! drawer tabs, resizers, overlays, and the listeners that keep the shell's
//! state current. At a phone width [`view`] draws the mobile shell
//! (`mobile.rs`) instead.

use std::time::Duration;

use roostr_core::nav::{NavEvent, Route};
use roostr_core::pane::{Column, PaneLayout, PaneTab};
use roostr_core::store::StoreEvent;
use roostr_core::types::SyncPhase;
use roostr_core::{Connection, State};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Theme, motion, outline_button, radius, ty};
use rune::{Anim, Color, Cursor, Curve, Cx, Drag, El, Phase, ScrollSpec, Tx, col, el, row, text};

use crate::shell::{
    DwellTab, RAIL_MAX, RAIL_MIN, Resize, SIDE_MAX, SIDE_MIN, Shell, current_space, open_object_id,
    shell,
};
use crate::{Pages, PanePart, list_motion, pin_collapse, pinned};

/// Resting on a drawer tab this long toggles it.
const DWELL: Duration = Duration::from_millis(700);
/// A drawer tab's hit area: its width with a mouse, and on a touch screen.
const DRAWER_TAB_W: f32 = 16.;
const DRAWER_TAB_TOUCH_W: f32 = 24.;
/// GPUI's TitleBar strip (`TITLE_BAR_HEIGHT`).
pub(crate) const TITLEBAR_H: f32 = 34.;

/// How far a drawer tab's hit area reaches past the 16px slot it is placed in.
fn drawer_tab_spill() -> f32 {
    if roostr_core::layout::is_touch() {
        (DRAWER_TAB_TOUCH_W - DRAWER_TAB_W) / 2.
    } else {
        0.
    }
}

// ------------------------------------------------------------ listeners

pub(crate) fn register(s: &mut State, pages: Pages) {
    let mut shell = Shell::load(&s.ui, pages);
    shell.shown_route = s.nav.route().clone();
    s.put(shell);
    s.on_store(on_store);
    s.on_nav(on_nav);
    s.on_poll(on_poll);
}

fn on_store(s: &mut State, event: &StoreEvent) {
    if matches!(event, StoreEvent::Refreshed) {
        refresh_delete_blocker(s, true);
    }
}

fn on_nav(s: &mut State, event: &NavEvent) {
    match event {
        NavEvent::Navigated => on_navigated(s),
        NavEvent::SpaceChanged => {
            if crate::is_mobile(s) {
                crate::mobile::space_changed(s);
            }
        }
        NavEvent::TabsChanged => {
            close_hidden_pages(s);
        }
    }
}

/// After every poll (what GPUI's `observe(store)` did): the store-driven work
/// that views can't do themselves. True when it changed what the view shows.
fn on_poll(s: &mut State) -> bool {
    if s.get::<Shell>().is_none() {
        return false;
    }
    // A phone browses outside the tabs; the desktop in them.
    let phone = crate::is_mobile(s);
    s.nav.set_phone(phone, &mut s.ui);
    let mut changed = on_store_changed(s);
    changed |= open_shown_route(s);
    changed |= top_pin_redirect(s);
    changed |= sync_pins(s);
    if !s.nav.space().is_empty()
        && let Some(route) = s.slot::<Shell>().pending_route.take()
    {
        roostr_core::nav::navigate(route, s);
        changed = true;
    }
    changed
}

fn on_store_changed(s: &mut State) -> bool {
    let store = &s.store;
    let (loaded, has_channels, first, ready) = (
        store.loaded,
        !store.channels.is_empty(),
        store.channels.first().map(|c| c.id.clone()),
        store.connection == Connection::Ready,
    );
    if !ready || !loaded {
        return false;
    }
    let mut changed = false;
    // Boot: the remembered space if it still exists, else the first. A vault
    // still being prepared may not show the remembered one yet.
    if s.nav.space().is_empty() && has_channels {
        let remembered = roostr_core::Navigator::remembered_space(&s.ui);
        let found = remembered.clone().filter(|id| s.store.space(id).is_some());
        let wait = remembered.is_some() && found.is_none() && s.store.status.preparing.is_some();
        if let Some(id) = found.or(first).filter(|_| !wait) {
            s.nav.use_space(&id, &mut s.ui);
            changed = true;
        }
    }
    let status = s.store.status.clone();
    let shell = s.slot::<Shell>();
    if has_channels {
        shell.seeding_personal = false;
    } else if !shell.seeding_personal && status.phase == SyncPhase::Live && status.bootstrapped {
        seed_personal(s);
    }
    let shell = s.slot::<Shell>();
    if !shell.space_order_moved && status.preparing.is_none() {
        shell.space_order_moved = true;
        move_space_order(s);
    }
    changed
}

/// A fresh key's empty vault gets a Personal space - but only once a
/// complete history walk proved it empty (`bootstrapped`, which only a
/// replica reports), never mid-backfill. Stays guarded until the space
/// shows up, so the refreshes its changes cause never fork a second one.
fn seed_personal(s: &mut State) {
    s.slot::<Shell>().seeding_personal = true;
    let task = roostr_core::api::space::create("Personal", None, s);
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| match result {
            Ok(id) => {
                s.store.refresh_all();
                s.nav.use_space(&id, &mut s.ui);
            }
            Err(error) => {
                log::warn!("could not create the Personal space: {error:#}");
                s.slot::<Shell>().seeding_personal = false;
            }
        });
    })
    .detach();
}

/// `moveSpaceOrder`: the legacy `order` field on owned spaces moves into
/// the preferences' space order, once per load.
fn move_space_order(s: &mut State) {
    let fetches: Vec<_> = s
        .store
        .channels
        .iter()
        .filter(|c| c.owner.is_empty())
        .map(|c| (c.id.clone(), s.store.fetch_object(&c.id)))
        .collect();
    s.rt.spawn(async move |cx| {
        let mut found: Vec<(String, f64)> = Vec::new();
        for (id, fetch) in fetches {
            if let Ok(object) = fetch.await
                && let Some(n) = object.field("order").and_then(|v| v.as_number())
            {
                found.push((id, n));
            }
        }
        if found.is_empty() {
            return;
        }
        cx.update(|s| {
            let missing: Vec<(String, f64)> = found
                .iter()
                .filter(|(id, _)| !s.store.space_order.contains_key(id))
                .cloned()
                .collect();
            if !missing.is_empty() {
                for (id, n) in missing {
                    s.store.space_order.insert(id, n);
                }
                let order = s.store.space_order.clone();
                roostr_core::api::user_prefs::set_space_order(&order, s).detach_and_log_err(&s.rt);
            }
            for (id, _) in &found {
                roostr_core::api::note::delete_field(id, "order", s).detach_and_log_err(&s.rt);
            }
        });
    })
    .detach();
}

/// The route the active tab shows has a page: it opens (and publishes its
/// pane) the first time it shows.
fn open_shown_route(s: &mut State) -> bool {
    let route = s.nav.route().clone();
    if s.slot::<Shell>().live.contains(&route) {
        return false;
    }
    show(s, route);
    true
}

/// `route` is the visible route now: reset the pane, open its page if it is
/// new, and let it publish its pane.
fn show(s: &mut State, route: Route) {
    s.pane.publish(false, false, 0, 0);
    let shell = s.slot::<Shell>();
    if !shell.live.contains(&route) {
        shell.live.push(route.clone());
    }
    let shown = shell.pages.shown;
    shown(s, &route);
}

fn on_navigated(s: &mut State) {
    let route = s.nav.route().clone();
    // While the shell waits for the remembered space, the sidebar shows
    // the first space ([`current_space`]): a page opened from it was opened
    // in that space, so that space is the one picked - the remembered one
    // arriving later must not swap its saved tabs in.
    if s.nav.space().is_empty()
        && route.is_workspace_page()
        && let Some(shown) = current_space(s).map(|c| c.id.clone())
    {
        s.nav.use_space(&shown, &mut s.ui);
        return;
    }
    close_hidden_pages(s);
    let shell = s.slot::<Shell>();
    shell.more = false;
    shell.shown_route = route.clone();
    show(s, route);
    refresh_delete_blocker(s, false);
}

/// Pages live while a tab of this space or the screen shows their route:
/// the others close.
fn close_hidden_pages(s: &mut State) {
    let shown = s.nav.route().clone();
    let tabs: Vec<Route> = s.nav.tabs().iter().map(|t| t.route.clone()).collect();
    let shell = s.slot::<Shell>();
    let (live, gone): (Vec<Route>, Vec<Route>) = std::mem::take(&mut shell.live)
        .into_iter()
        .partition(|r| *r == shown || tabs.contains(r));
    shell.live = live;
    let closed = shell.pages.closed;
    for r in &gone {
        closed(s, r);
    }
}

/// Nothing open (a new tab, the last one closed, a space with no saved
/// tabs, the open object binned): on a desktop the space's top pin opens
/// instead of its empty home. The home still shows in a space with no pins.
fn top_pin_redirect(s: &mut State) -> bool {
    let width = crate::window_width(s);
    if width <= 0. || roostr_ui::layout::is_mobile(width) || *s.nav.route() != Route::Home {
        return false;
    }
    let space = s.nav.space().to_string();
    let Some(pin) = roostr_core::nav::top_pin(&space, s) else {
        return false;
    };
    s.nav.replace(Route::Object(pin), &mut s.ui);
    true
}

/// The current space's pins: their objects asked for, folds settled, the
/// open ones' widgets loaded, and the card lists' orders noted.
fn sync_pins(s: &mut State) -> bool {
    let Some(space) = current_space(s) else {
        return false;
    };
    let (space, pinned) = (space.id.clone(), space.pinned_ids.clone());
    let mut changed = pin_collapse::sync(s, &pinned);
    let open: Vec<String> = pinned
        .iter()
        .filter(|id| !pin_collapse::collapsed(s, id))
        .cloned()
        .collect();
    changed |= pinned::sync(s, &open);
    changed |= list_motion::note_pins(s, "pins", &space, &pinned);
    changed |= list_motion::note_pins(s, "m-pins", &space, &pinned);
    changed
}

/// Whether the open object (a type or property) may be deleted: the More
/// menu's last row. Asked again when the object changes, or on a refresh
/// (`force`), keeping the last answer until the new one lands.
pub(crate) fn refresh_delete_blocker(s: &mut State, force: bool) {
    let Some(id) = open_object_id(s) else {
        s.slot::<Shell>().delete_blocker = (String::new(), String::new());
        return;
    };
    let is_def = s.store.relations.iter().any(|r| r.id == id)
        || s.store
            .summary(&id)
            .is_some_and(|o| o.type_key == "type" || o.type_key == "tool");
    let shell = s.slot::<Shell>();
    if !force && shell.delete_blocker.0 == id {
        return;
    }
    // A refresh of the same object keeps its last answer until the new
    // one lands: clearing it first made the More menu flip between
    // "can't delete" and "Move to bin" on every store refresh.
    if shell.delete_blocker.0 != id {
        shell.delete_blocker = (id.clone(), String::new());
    }
    if !is_def {
        shell.delete_blocker.1.clear();
        return;
    }
    let fetch = s.store.fetch_object(&id);
    s.rt.spawn(async move |cx| {
        let Ok(object) = fetch.await else { return };
        let task = cx.update(|s| roostr_core::protected::deletion_blocker(&object, s));
        let why = task.await.unwrap_or_default();
        cx.update(|s| {
            let shell = s.slot::<Shell>();
            if shell.delete_blocker.0 == id {
                shell.delete_blocker.1 = why;
            }
        });
    })
    .detach();
}

// ------------------------------------------------------------ actions

pub(crate) fn toggle_nav(s: &mut State) {
    let shell = s.slot::<Shell>();
    shell.nav_hidden = !shell.nav_hidden;
    let hidden = shell.nav_hidden;
    s.ui.set("nav-hidden", &if hidden { "1" } else { "0" });
}

/// The edge drawer tab: collapse both columns while any is expanded, else expand both.
fn toggle_pane(s: &mut State) {
    let open = !s.pane.any_open();
    s.pane.set_all(open, &mut s.ui);
}

fn dwell_start(s: &mut State, which: DwellTab) {
    let timer = s.rt.timer(DWELL);
    let task = s.rt.spawn(async move |cx| {
        timer.await;
        cx.update(|s| {
            let shell = s.slot::<Shell>();
            if shell.dwell.as_ref().is_some_and(|d| d.which == which) {
                shell.dwell = None;
                match which {
                    DwellTab::Nav => toggle_nav(s),
                    DwellTab::Pane => toggle_pane(s),
                }
            }
        });
    });
    s.slot::<Shell>().dwell = Some(crate::shell::Dwell {
        which,
        _timer: task,
    });
}

fn dwell_cancel(s: &mut State) {
    s.slot::<Shell>().dwell = None;
}

pub(crate) fn rail_persist(s: &mut State) {
    let width = s.slot::<Shell>().rail_width.round();
    s.ui.set("rail-width", &width.to_string());
    if width > 100. {
        s.ui.set("rail-width-wide", &width.to_string());
    }
}

pub(crate) fn rail_toggle(s: &mut State) {
    let shell = s.slot::<Shell>();
    shell.rail_width = if shell.rail_wide() {
        RAIL_MIN
    } else {
        shell.rail_wide_width.max(160.)
    };
    rail_persist(s);
}

/// A resizer's drag: the column follows the pointer 1:1 (drag moves are
/// instant), and the width persists when it lets go.
pub(crate) fn resize_drag(s: &mut State, d: &Drag, start: Resize) {
    match d.phase {
        Phase::Down => s.slot::<Shell>().resize = Some(start),
        Phase::Move => {
            let Some(resize) = s.slot::<Shell>().resize else {
                return;
            };
            match resize {
                Resize::Rail { start, moved } => {
                    let shell = s.slot::<Shell>();
                    shell.resize = Some(Resize::Rail {
                        start,
                        moved: moved || d.dx.abs() > 4.,
                    });
                    shell.rail_width = (start + d.dx).clamp(RAIL_MIN, RAIL_MAX);
                }
                Resize::Side { start } => {
                    s.slot::<Shell>().side_width = (start + d.dx).clamp(SIDE_MIN, SIDE_MAX);
                }
                Resize::Pane { tab, start } => {
                    // As wide as the window allows: the page beside the columns keeps its minimum.
                    let other = pane_layout(s).get(tab.other()).width();
                    let shell = s.slot::<Shell>();
                    let nav = if shell.nav_hidden {
                        0.
                    } else {
                        shell.rail_width + shell.side_width
                    };
                    let room = shell.width - nav - other;
                    let width = (start - d.dx).clamp(
                        roostr_core::pane::MIN_WIDTH,
                        roostr_core::pane::max_width(room),
                    );
                    s.pane.set_width(tab, width);
                }
            }
        }
        Phase::Up => match s.slot::<Shell>().resize.take() {
            Some(Resize::Rail { moved, .. }) => {
                if !moved {
                    rail_toggle(s);
                    return;
                }
                let shell = s.slot::<Shell>();
                if shell.rail_width < 140. {
                    shell.rail_width = RAIL_MIN;
                } else {
                    shell.rail_wide_width = shell.rail_width;
                }
                rail_persist(s);
            }
            Some(Resize::Side { .. }) => {
                let width = s.slot::<Shell>().side_width.round();
                s.ui.set("side-width", &width.to_string());
            }
            Some(Resize::Pane { tab, .. }) => s.pane.save_width(tab, &mut s.ui),
            None => {}
        },
    }
}

// ------------------------------------------------------------ rendering

/// The whole window (see [`crate::view`]).
pub(crate) fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = crate::theme(s, cx);
    let Some(sh) = shell(s) else {
        return roostr_ui::root(col().w_full().h_full().fill(t.gutter), []);
    };
    let pages = sh.pages;
    let mut layer = Vec::new();
    let mobile = cx
        .layout_with("shell-root", |r| roostr_ui::layout::is_mobile(r[2]))
        .unwrap_or(false);
    let route = s.nav.route().clone();
    let content = match boot(s, cx, t, &pages, &mut layer) {
        Some(boot) => boot,
        None if mobile && !matches!(route, Route::Site(_)) => {
            crate::mobile::view(s, cx, t, &mut layer)
        }
        None if matches!(route, Route::Site(_) | Route::Chat(_)) => col()
            .key("full-page")
            .w_full()
            .grow(1.)
            .min_h(0.)
            .fill(t.bg)
            .child((pages.page)(s, cx, &route, &mut layer)),
        None => desk(s, cx, t, sh, mobile, &mut layer),
    };
    overlays(s, cx, t, mobile, &mut layer);
    (pages.overlays)(s, cx, &mut layer);
    let mut root = col()
        .key("shell-root")
        .w_full()
        .h_full()
        .fill(t.gutter)
        .on_layout(|s: &mut State, r: [f32; 4]| {
            let shell = s.slot::<Shell>();
            if (shell.width - r[2]).abs() > 0.5 {
                shell.width = r[2];
            }
            if (shell.height - r[3]).abs() > 0.5 {
                shell.height = r[3];
            }
        });
    // A desktop window's strip: a browser tab has none, and the iOS window's content starts
    // below the status bar (the host keeps it inside the safe area).
    if cfg!(not(any(target_family = "wasm", target_os = "ios"))) {
        root = root.child(titlebar(t));
    }
    roostr_ui::root(root.child(content), layer)
}

/// gpui-kit's TitleBar strip: 34 pt, a vertical gradient from the title bar
/// colour mixed 55/45 with the background to the title bar colour, a bottom
/// border, the traffic lights over its left end, and a window drag region.
fn titlebar(t: &Theme) -> El {
    let k = &t.kit;
    let top = roostr_ui::color::mix(k.background, k.title_bar, 0.55);
    el().key("titlebar")
        .w_full()
        .h(TITLEBAR_H)
        .gradient(top, k.title_bar)
        .border_sides([0., 0., 1., 0.], k.title_bar_border)
        .window_drag()
}

/// The boot states: connecting, a failed connection (with Retry), or the
/// identity gate while the daemon has no key. `None` once ready.
fn boot(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    pages: &Pages,
    layer: &mut Vec<El>,
) -> Option<El> {
    let frame = || col().key("boot").w_full().grow(1.).min_h(0.).fill(t.bg);
    let status = |line: String, color: Color| {
        row()
            .w_full()
            .justify_center()
            .my(16.)
            .px(24.)
            .child(text(line).font(ty::ui(16.)).color(color).wrap().shrink())
    };
    match &s.store.connection {
        Connection::Ready => None,
        Connection::Connecting => {
            // The loading card the phone's first load uses, centred: a sweeping bar until the
            // replica knows how far it has to go, then the percentage.
            let status = &s.store.status;
            let pct = roostr_core::sync_status::sync_progress(status).and_then(|p| p.pct);
            let detail = roostr_core::sync_status::boot_progress(status)
                .unwrap_or_else(|| "Connecting…".to_string());
            Some(
                frame().justify_center().items_center().padding(16.).child(
                    crate::mobile::progress_card(
                        s,
                        t,
                        "boot-progress",
                        "Opening your vault",
                        pct,
                        detail,
                    )
                    .w_full()
                    .max_w(380.),
                ),
            )
        }
        Connection::Failed(error) => {
            if s.store.settings.as_ref().map(|s| s.has_key) == Some(false) {
                return Some(frame().child((pages.identity_gate)(s, cx, layer)));
            }
            Some(
                frame()
                    .child(
                        col()
                            .items_center()
                            .padding(48.)
                            .gap(16.)
                            .child(
                                text("Your identity and local changes remain on this device.")
                                    .font(ty::ui(16.))
                                    .color(t.fg),
                            )
                            .child(
                                outline_button("retry", "Retry opening vault")
                                    .on_press(|s: &mut State| s.store.reconnect())
                                    .build(t),
                            ),
                    )
                    .child(status(error.clone(), t.fg)),
            )
        }
    }
}

/// The desktop shell: the tab strip (2+ tabs), then rail | sidebar | main |
/// Properties | Chat on the gutter, with the resizers and drawer tabs over the
/// gaps. Columns slide over [`motion::COLUMNS`]; a resizer drags them 1:1.
fn desk(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    sh: &Shell,
    _mobile: bool,
    layer: &mut Vec<El>,
) -> El {
    let route = s.nav.route().clone();
    let rail_col = if sh.nav_hidden { 0. } else { sh.rail_width };
    let side_col = if sh.nav_hidden { 0. } else { sh.side_width };
    // A narrower window narrows the columns to fit (or shows one and a
    // strip); the chosen widths come back when there is room.
    // The window's width as laid out (settles on the first frame; the slot's
    // copy lags a frame).
    let window_w = cx
        .layout_size("shell-root")
        .map_or(sh.width, |size| size[0]);
    let room = window_w - rail_col - side_col;
    let pane_here = matches!(route, Route::Object(_)) && s.pane.available;
    let layout = if pane_here {
        s.pane.layout(room)
    } else {
        PaneLayout::HIDDEN
    };
    let pane_tab_shown =
        pane_here && (s.pane.has(PaneTab::Chat) || s.pane.has(PaneTab::Properties));
    let pane_open = s.pane.any_open();
    let chat_col = layout.chat.width();
    let props_col = layout.properties.width();
    let wide = open_object_id(s)
        .and_then(|id| s.store.summary(&id).map(|o| o.type_key.clone()))
        .is_some_and(|k| matches!(k.as_str(), "query" | "set" | "collection"));
    let tabs_shown = s.nav.tabs().len() > 1;
    let nav_tab_left = if sh.nav_hidden {
        0.
    } else {
        rail_col + side_col + 3. - 8. - drawer_tab_spill()
    };
    let columns = motion::COLUMNS.spring();

    let mut shell = row()
        .key("desk")
        .w_full()
        .grow(1.)
        .min_h(0.)
        .fill(t.gutter)
        // Rail column.
        .child(
            col()
                .key("rail-col")
                .w(rail_col)
                .h_full()
                .py(6.)
                .clip_rounded(0.)
                .motion(columns)
                .children(
                    (!sh.nav_hidden && rail_col > 6.)
                        .then(|| crate::rail::view(s, cx, t, sh, rail_col - 6.)),
                ),
        )
        // Sidebar column.
        .child(
            col()
                .key("side-col")
                .w(side_col)
                .h_full()
                .py(6.)
                .clip_rounded(0.)
                .motion(columns)
                .children(
                    (!sh.nav_hidden && side_col > 6.)
                        .then(|| crate::sidebar::view(s, cx, t, sh, side_col - 6., layer)),
                ),
        )
        .child(main_column(s, cx, t, &route, wide, layer))
        // The right pane's columns: Properties, then Chat.
        .child(pane_column(
            s,
            cx,
            t,
            PaneTab::Properties,
            layout.properties,
            layer,
        ))
        .child(pane_column(s, cx, t, PaneTab::Chat, layout.chat, layer));
    // Resizers (in the gaps between panes).
    if !sh.nav_hidden {
        shell = shell
            .child(crate::rail::resize_handle(t, sh, rail_col))
            .child(
                el().key("side-resize")
                    .pin(Some(rail_col + side_col - 8.), Some(6.), None, Some(6.))
                    .w(8.)
                    .cursor(Cursor::ResizeColumn)
                    .on_drag(|s: &mut State, d: &mut Drag, _: &mut Tx| {
                        let start = s.slot::<Shell>().side_width;
                        resize_drag(s, d, Resize::Side { start });
                    }),
            );
    }
    for (tab, column, edge) in [
        (PaneTab::Properties, layout.properties, props_col + chat_col),
        (PaneTab::Chat, layout.chat, chat_col),
    ] {
        if column.is_open() {
            shell = shell.child(pane_resize(tab, edge));
        }
    }
    // Drawer tabs.
    shell = shell.child(drawer_tab(
        s,
        t,
        sh,
        DwellTab::Nav,
        !sh.nav_hidden,
        Some(nav_tab_left),
        None,
    ));
    if pane_tab_shown {
        let right = (chat_col + props_col + 3. - 8. - drawer_tab_spill()).max(0.);
        shell = shell.child(drawer_tab(
            s,
            t,
            sh,
            DwellTab::Pane,
            pane_open,
            None,
            Some(right),
        ));
    }
    let mut desk = col().key("desk-frame").w_full().grow(1.).min_h(0.);
    if tabs_shown {
        desk = desk.child(crate::tabs::view(s, cx, t));
    }
    desk.child(shell)
}

/// The middle column: the page card, with the header over the scrolling
/// page (wide pages run the header's inset; others a 920 pt column).
fn main_column(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    route: &Route,
    wide: bool,
    layer: &mut Vec<El>,
) -> El {
    let pages = shell(s).map(|sh| sh.pages).unwrap_or_default();
    let page = (pages.page)(s, cx, route, layer);
    let body = col().w_full().pt(24.).pb(80.);
    let body = if wide {
        // Wide pages (queries, sets, collections): the header's inset, so
        // they start under the back/forward arrows.
        body.px(32.).child(page)
    } else {
        body.items_center()
            .child(col().w_full().max_w(920.).child(page))
    };
    col()
        .key("main-col")
        .grow(1.)
        .min_w(0.)
        .min_h(0.)
        .mx(6.)
        .my(6.)
        .radius(radius::PAGE)
        .fill(t.bg)
        .clip_rounded(radius::PAGE)
        .child(
            col()
                .w_full()
                .px(32.)
                .items_center()
                .child(crate::header::view(s, cx, t, wide, layer)),
        )
        .child(
            col()
                .key(format!("main-scroll:{}", route.path()))
                .grow(1.)
                .min_h(0.)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .px(32.)
                .child(body),
        )
}

/// A pane column `column` wide (sliding), its 6 pt gap on the right: the
/// page's column body while expanded, else its strip.
fn pane_column(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    tab: PaneTab,
    column: Column,
    layer: &mut Vec<El>,
) -> El {
    let width = column.width();
    let inner = match column {
        Column::Open(_) => open_object_id(s).map(|id| {
            let pane = shell(s)
                .map(|sh| sh.pages.pane)
                .unwrap_or(Pages::default().pane);
            pane(s, cx, &id, PanePart::Column(tab), layer)
        }),
        Column::Strip => Some(pane_strip(s, t, tab)),
        Column::Hidden => None,
    };
    let mut outer = col()
        .key(format!("pane-col-{}", tab.name()))
        .w(width)
        .h_full()
        .py(6.)
        .clip_rounded(0.)
        .motion(motion::COLUMNS.spring());
    if let Some(inner) = inner {
        outer = outer.child(
            col()
                .key(format!("pane-card-{}", tab.name()))
                .w((width - 6.).max(0.))
                .h_full()
                .radius(radius::PAGE)
                .fill(t.bg)
                .clip_rounded(radius::PAGE)
                .child(inner),
        );
    }
    outer
}

/// A collapsed column: its icon over its name, letter by letter; a press expands it.
fn pane_strip(s: &State, t: &Theme, tab: PaneTab) -> El {
    let name = tab.name();
    let tip = match tab {
        PaneTab::Chat => "Show chat",
        PaneTab::Properties => "Show properties",
    };
    let letters = name.chars().fold(col().items_center(), |c, ch| {
        c.child(crate::shell::hover_fg(
            text(ch.to_string()).font(ty::ui(11.5).semibold().lh(13.)),
            t,
        ))
    });
    let strip = col()
        .key(format!("pane-strip-{name}"))
        .w_full()
        .h_full()
        .items_center()
        .gap(8.)
        .pt(14.)
        .cursor(Cursor::Pointer)
        .hover(t.hover)
        .hover_group()
        .on_press(move |s: &mut State| s.pane.set_open(tab, true, &mut s.ui))
        .child(crate::shell::hover_fg(
            text(tab.icon(s)).font(ty::ui(13.)),
            t,
        ))
        .child(letters);
    roostr_ui::tooltip(strip, tip)
}

/// The resizer on an expanded column's left edge, `edge` pt from the window's right.
fn pane_resize(tab: PaneTab, edge: f32) -> El {
    el().key(format!("pane-resize-{}", tab.name()))
        .pin(None, Some(6.), Some(edge - 6.), Some(6.))
        .w(12.)
        .cursor(Cursor::ResizeColumn)
        .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| {
            let start = pane_layout(s).get(tab).width();
            resize_drag(s, d, Resize::Pane { tab, start });
        })
}

/// A drawer tab: a slim pill on an invisible hit area, a hairline at rest
/// that thickens and darkens on hover (`transition: 0.12s ease`); resting on
/// it fills it with accent over [`DWELL`], then it toggles. A press toggles at once.
#[allow(clippy::too_many_arguments)]
fn drawer_tab(
    s: &State,
    t: &Theme,
    sh: &Shell,
    which: DwellTab,
    open: bool,
    left: Option<f32>,
    right: Option<f32>,
) -> El {
    let _ = s;
    let dwelling = sh.dwell.as_ref().is_some_and(|d| d.which == which);
    let hovered = sh.dwell.as_ref().is_some_and(|d| d.which == which);
    let tip = match (which, open) {
        (DwellTab::Nav, true) => "Hide sidebar",
        (DwellTab::Nav, false) => "Show sidebar",
        (DwellTab::Pane, true) => "Hide chat & properties",
        (DwellTab::Pane, false) => "Show chat & properties",
    };
    let key = match which {
        DwellTab::Nav => "nav-drawer",
        DwellTab::Pane => "pane-drawer",
    };
    let touch = roostr_core::layout::is_touch();
    let (hit_w, hit_h) = if touch {
        (DRAWER_TAB_TOUCH_W, 96.)
    } else {
        (DRAWER_TAB_W, 72.)
    };
    let pill_h = if hovered { 40. } else { 32. };
    let fill = el()
        .key(format!("{key}-fill"))
        .pin(Some(0.), Some(0.), Some(0.), Some(0.))
        .fill(t.accent)
        .offset(0., if dwelling { 0. } else { pill_h })
        .offset_motion(if dwelling {
            Anim::new(Curve::linear(DWELL.as_secs_f32()))
        } else {
            Anim::INSTANT
        });
    let pill = el()
        .key(format!("{key}-pill"))
        .w(if hovered { 4. } else { 3. })
        .h(pill_h)
        .radius(2.)
        .clip_rounded(2.)
        .fill(if hovered { t.muted } else { t.border })
        .motion(motion::FADE.spring())
        .child(fill);
    let mut hit = row()
        .key(key)
        .pin(left, None, right, None)
        .w(hit_w)
        .h(hit_h)
        .items_center()
        .cursor(Cursor::Pointer)
        .on_hover_change(move |s: &mut State, hovered: bool, _: &mut Tx| {
            if hovered {
                dwell_start(s, which);
            } else {
                dwell_cancel(s);
            }
        })
        .on_press(move |s: &mut State| {
            dwell_cancel(s);
            match which {
                DwellTab::Nav => toggle_nav(s),
                DwellTab::Pane => toggle_pane(s),
            }
        })
        .child(pill);
    // Shut, the nav pill sits a few pt in from the window edge; the pane's
    // always sits in the gap left of its columns or strips.
    hit = match (which, open) {
        (DwellTab::Nav, false) => hit.justify_start().pl(4.),
        _ => hit.justify_center(),
    };
    roostr_ui::tooltip(hit.self_center(), tip)
}

/// The shell's overlays, last in the window: drag ghosts, the New menu, the
/// More menu, the New property suggest, the delete-property confirmation,
/// search, and the prompt.
fn overlays(s: &State, cx: &mut Cx<State>, t: &Theme, mobile: bool, layer: &mut Vec<El>) {
    crate::rail::space_ghost(s, t, layer);
    crate::tabs::tab_ghost(s, t, layer);
    crate::create_menu::overlay(s, cx, t, layer);
    if shell(s).is_some_and(|sh| sh.more) {
        crate::header::more_overlay(s, cx, t, mobile, layer);
    }
    crate::sidebar::new_prop_overlay(s, cx, t, layer);
    crate::header::delete_overlay(s, t, layer);
    crate::search::overlay(s, cx, t, layer);
    crate::modal::overlay(s, cx, t, layer);
}

/// The pane columns the desk lays out now (what a resizer drags against).
fn pane_layout(s: &State) -> PaneLayout {
    let Some(sh) = shell(s) else {
        return PaneLayout::HIDDEN;
    };
    if !matches!(s.nav.route(), Route::Object(_)) || !s.pane.available {
        return PaneLayout::HIDDEN;
    }
    let nav = if sh.nav_hidden {
        0.
    } else {
        sh.rail_width + sh.side_width
    };
    s.pane.layout(sh.width - nav)
}
