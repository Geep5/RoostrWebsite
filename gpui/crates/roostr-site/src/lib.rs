//! Website pages shown inside the app window: the landing (`/`), the guide (`/guide`), the
//! privacy policy (`/privacy`) and the space join request landing (`/j#r2.<blob>`), ported
//! from `src/routes/{+page,guide,privacy,j}`.
//!
//! [`register`] once at startup, [`view`] draws the page a [`SitePage`] route shows, and
//! [`on_key`] scrolls the scrolled pages from the keyboard as a browser does.

mod assets;
mod doc;
mod guide;
mod join;
mod landing;
mod privacy;
mod text;

use std::cell::Cell;
use std::rc::Rc;

use roostr_core::State;
use roostr_core::nav::{NavEvent, Route, SitePage};
use roostr_ui::ScrollArea;
use rune::{Cx, El, Key, KeyEvent, ScrollPos, Tx};

/// The scroll viewport of the guide and privacy pages.
const DOC_SCROLL: &str = "site-doc-scroll";
/// The scroll viewport of the join page.
const JOIN_SCROLL: &str = "site-join-scroll";

/// Up and Down scroll by this much (a browser's arrow step).
const LINE: f32 = 40.0;
/// Page Up and Page Down scroll by this share of the viewport (Chrome's 87.5%).
const PAGE: f32 = 0.875;

/// Where the shown scrolled site page is, as its viewport last reported it.
#[derive(Default)]
struct SiteScroll(Rc<Cell<ScrollPos>>);

/// Set up the site pages' state once at startup (replaces GPUI's `init(cx)`): the keyboard
/// scroll position, the landing hero and join page slots, and a navigation listener that stops
/// the hero animation once the landing is left.
pub fn register(s: &mut State) {
    s.slot::<SiteScroll>();
    s.slot::<assets::Hero>();
    s.slot::<join::JoinPages>();
    s.on_nav(left_landing);
}

fn left_landing(s: &mut State, event: &NavEvent) {
    if *event == NavEvent::Navigated && *s.nav.route() != Route::Site(SitePage::Landing) {
        assets::stop_hero(s);
    }
}

/// One website page, filling its parent and owning its scroll (replaces `SiteView`).
pub fn view(s: &State, cx: &mut Cx<State>, page: &SitePage) -> El {
    match page {
        SitePage::Landing => landing::view(s, cx),
        SitePage::Guide => doc::view(s, guide::BLOCKS, &guide::style()),
        SitePage::Privacy => doc::view(s, privacy::BLOCKS, &privacy::style()),
        SitePage::Join(blob) => join::view(s, cx, blob),
    }
}

/// Browser-style keyboard scrolling of the site page `s.nav.route()` shows: Up/Down a line,
/// Page Up/Page Down, Space/Shift-Space and Option-Up/Option-Down a page, Home/End and
/// Command-Up/Command-Down (Ctrl off macOS) to the ends. The scrolled pages call it while focus
/// is inside them; a window showing a site page calls it from its `on_key` so the keys work
/// with nothing focused. True when it took the key.
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let key = match s.nav.route() {
        Route::Site(SitePage::Guide | SitePage::Privacy) => DOC_SCROLL,
        Route::Site(SitePage::Join(_)) => JOIN_SCROLL,
        _ => return false,
    };
    let Some(pos) = s.get::<SiteScroll>().map(|p| p.0.get()) else {
        return false;
    };
    let Some(y) = scroll_target(ev, pos) else {
        return false;
    };
    tx.scroll_to_y(key, y);
    true
}

/// Where `ev` scrolls a viewport at `pos`, if it is one of the scroll keys.
fn scroll_target(ev: &KeyEvent, pos: ScrollPos) -> Option<f32> {
    let m = ev.mods;
    let plain = !(m.shift || m.cmd || m.alt || m.ctrl || m.logo);
    let alt_only = m.alt && !(m.shift || m.cmd || m.ctrl || m.logo);
    let cmd_only = m.cmd && !(m.shift || m.alt);
    let shift_only = m.shift && !(m.cmd || m.alt || m.ctrl || m.logo);
    let page = pos.viewport[1] * PAGE;
    let dy = match &ev.key {
        Key::Up if plain => -LINE,
        Key::Down if plain => LINE,
        Key::PageUp if plain => -page,
        Key::PageDown if plain => page,
        Key::Up if alt_only => -page,
        Key::Down if alt_only => page,
        Key::Text(t) if t == " " && shift_only => -page,
        Key::Text(t) if t == " " && plain => page,
        Key::Home if plain => f32::NEG_INFINITY,
        Key::End if plain => f32::INFINITY,
        Key::Up if cmd_only => f32::NEG_INFINITY,
        Key::Down if cmd_only => f32::INFINITY,
        _ => return None,
    };
    Some((pos.offset[1] + dy).clamp(0.0, pos.max[1].max(0.0)))
}

/// A scrolled page's viewport reports where it is to [`on_key`].
fn track_scroll(s: &State, area: ScrollArea) -> ScrollArea {
    match s.get::<SiteScroll>() {
        Some(SiteScroll(pos)) => {
            let pos = pos.clone();
            area.on_scroll_offset(move |at| pos.set(at))
        }
        None => area,
    }
}

/// A scrolled page's root: takes focus on a click inside it and routes the scroll keys.
fn scroll_root(root: El) -> El {
    root.focusable(true)
        .no_focus_ring()
        .on_key(|s: &mut State, ev: &KeyEvent, tx: &mut Tx| on_key(s, ev, tx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn key(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods }
    }

    #[test]
    fn scroll_keys_step_like_a_browser() {
        let mut pos = ScrollPos::default();
        pos.offset = [0.0, 100.0];
        pos.max = [0.0, 1000.0];
        pos.viewport = [800.0, 400.0];
        let none = Mods::default();
        let alt = Mods { alt: true, ..none };
        let shift = Mods {
            shift: true,
            ..none
        };
        let cmd = Mods {
            cmd: true,
            logo: true,
            ..none
        };
        let at = |k, m| scroll_target(&key(k, m), pos);
        assert_eq!(at(Key::Down, none), Some(140.0));
        assert_eq!(at(Key::Up, none), Some(60.0));
        assert_eq!(at(Key::PageDown, none), Some(450.0));
        assert_eq!(at(Key::PageUp, none), Some(0.0));
        assert_eq!(at(Key::Down, alt), Some(450.0));
        assert_eq!(at(Key::Text(" ".into()), none), Some(450.0));
        assert_eq!(at(Key::Text(" ".into()), shift), Some(0.0));
        assert_eq!(at(Key::End, none), Some(1000.0));
        assert_eq!(at(Key::Home, none), Some(0.0));
        assert_eq!(at(Key::Down, cmd), Some(1000.0));
        assert_eq!(at(Key::Up, cmd), Some(0.0));
        assert_eq!(at(Key::Text("a".into()), none), None);
        assert_eq!(at(Key::Down, shift), None);
    }
}
