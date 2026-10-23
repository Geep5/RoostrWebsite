//! Navigation: routes, the per-space tab strip (`src/lib/tabs.svelte.ts`) and
//! one global back/forward history (the browser history the web app uses).
//!
//! Navigating replaces the active tab's route (browser model); tabs persist
//! per space in [`UiState`] under `roostr.tabs.spaces`.

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use serde::{Deserialize, Serialize};

use crate::ui_state::UiState;

const TABS_KEY: &str = "roostr.tabs.spaces";
const ACTIVE_SPACE_KEY: &str = "glon.channel";

/// One screen of the app (`/app/...` routes) or the website (`/`, `/guide`,
/// `/privacy`, `/j#<blob>`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// `/app`: the space home.
    Home,
    /// `/app/object/:id`: any object; the object page dispatches on its type.
    Object(String),
    /// `/app/graph?focus=<id>`.
    Graph { focus: Option<String> },
    /// `/app/chat/:id`: full-screen discussion.
    Chat(String),
    /// Website pages, shown inside the app window.
    Site(SitePage),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SitePage {
    /// `/`: the landing page.
    Landing,
    /// `/guide`.
    Guide,
    /// `/privacy`.
    Privacy,
    /// `/j#<blob>`: a space join request (`r2.` + base64url JSON).
    Join(String),
}

impl Route {
    /// The web path, which is what tabs persist (the focus query is dropped,
    /// like `pathname`).
    pub fn path(&self) -> String {
        match self {
            Self::Home => "/app".into(),
            Self::Object(id) => format!("/app/object/{id}"),
            Self::Graph { .. } => "/app/graph".into(),
            Self::Chat(id) => format!("/app/chat/{id}"),
            Self::Site(SitePage::Landing) => "/".into(),
            Self::Site(SitePage::Guide) => "/guide".into(),
            Self::Site(SitePage::Privacy) => "/privacy".into(),
            Self::Site(SitePage::Join(blob)) => format!("/j#{blob}"),
        }
    }

    /// The route for a web path. The join page takes its blob from the
    /// fragment (`location.hash`), whatever the query.
    pub fn parse(path: &str) -> Self {
        let (path, hash) = path.split_once('#').unwrap_or((path, ""));
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        match path {
            "/" => return Self::Site(SitePage::Landing),
            "/guide" => return Self::Site(SitePage::Guide),
            "/privacy" => return Self::Site(SitePage::Privacy),
            "/j" => return Self::Site(SitePage::Join(hash.to_string())),
            _ => {}
        }
        if let Some(id) = path.strip_prefix("/app/object/").filter(|s| !s.is_empty()) {
            return Self::Object(id.to_string());
        }
        if let Some(id) = path.strip_prefix("/app/chat/").filter(|s| !s.is_empty()) {
            return Self::Chat(id.to_string());
        }
        if path == "/app/graph" {
            let focus = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("focus="))
                .map(str::to_string);
            return Self::Graph { focus };
        }
        Self::Home
    }

    /// The object this route shows, if any.
    pub fn object_id(&self) -> Option<&str> {
        match self {
            Self::Object(id) | Self::Chat(id) => Some(id),
            Self::Graph { focus } => focus.as_deref(),
            Self::Home | Self::Site(_) => None,
        }
    }

    /// A page of a space's workspace (an object, a chat, the graph): not
    /// the space home or a website page.
    pub fn is_workspace_page(&self) -> bool {
        matches!(self, Self::Object(_) | Self::Chat(_) | Self::Graph { .. })
    }

    /// Whether the address bar's `url` already names this route: the same
    /// pathname, parsing to the same web path. Its query and any parameter
    /// [`Self::path`] drops stay, as the Svelte pages never rewrote their
    /// URL (`/j?utm=x#r2.…`, `/app/graph?focus=<id>`).
    pub fn shown_by(&self, url: &str) -> bool {
        fn pathname(url: &str) -> &str {
            url.split(['?', '#']).next().unwrap_or_default()
        }
        let path = self.path();
        pathname(url) == pathname(&path) && Self::parse(url).path() == path
    }

    /// The browser tab's title, the Svelte pages' `<title>`s. An object page
    /// names its object (`object_name`: `None` until it has loaded); pages
    /// that set none (the space home, a chat) show the app shell's `Roostr`.
    pub fn title(&self, object_name: Option<&str>) -> String {
        match self {
            Self::Object(_) => {
                let name = match object_name {
                    None => "Loading…",
                    Some("") => "Untitled",
                    Some(name) => name,
                };
                format!("{name} — glon")
            }
            Self::Graph { .. } => "Graph — Roostr".into(),
            Self::Home | Self::Chat(_) => "Roostr".into(),
            Self::Site(SitePage::Landing) => "Roostr — your things speak".into(),
            Self::Site(SitePage::Guide) => "Guide — Roostr".into(),
            Self::Site(SitePage::Privacy) => "Privacy — Roostr".into(),
            Self::Site(SitePage::Join(_)) => "Join a space — Roostr".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// Process-local identity (survives reordering).
    pub uid: u64,
    pub route: Route,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Saved {
    paths: Vec<String>,
    active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavEvent {
    /// The visible route changed (navigation, tab switch, back/forward).
    Navigated,
    /// The active space changed.
    SpaceChanged,
}

pub struct Navigator {
    tabs: Vec<Tab>,
    active: usize,
    /// Active space id; "" until the store knows the spaces.
    space: String,
    saved: HashMap<String, Saved>,
    history: Vec<Route>,
    history_ix: usize,
    next_uid: u64,
}

struct GlobalNavigator(Entity<Navigator>);

impl Global for GlobalNavigator {}

impl EventEmitter<NavEvent> for Navigator {}

impl Navigator {
    /// The global navigator, created once: a browser tab that showed a
    /// website page before its vault opened keeps that page's navigator.
    pub fn init(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx.try_global::<GlobalNavigator>() {
            return existing.0.clone();
        }
        let saved: HashMap<String, Saved> = UiState::get(cx, TABS_KEY).unwrap_or_default();
        let navigator = cx.new(|_| Self {
            tabs: vec![Tab {
                uid: 1,
                route: Route::Home,
            }],
            active: 0,
            space: String::new(),
            saved,
            history: vec![Route::Home],
            history_ix: 0,
            next_uid: 2,
        });
        cx.set_global(GlobalNavigator(navigator.clone()));
        navigator
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalNavigator>().0.clone()
    }

    /// The space the app last had open (`glon.channel`).
    pub fn remembered_space(cx: &App) -> Option<String> {
        UiState::get(cx, ACTIVE_SPACE_KEY)
    }

    pub fn route(&self) -> &Route {
        &self.tabs[self.active].route
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn space(&self) -> &str {
        &self.space
    }

    pub fn can_go_back(&self) -> bool {
        self.history_ix > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_ix + 1 < self.history.len()
    }

    fn uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    /// Go to `route` in the active tab (`goto` + `tabs.sync`).
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if *self.route() == route {
            return;
        }
        self.history.truncate(self.history_ix + 1);
        self.history.push(route.clone());
        self.history_ix = self.history.len() - 1;
        self.show(route, cx);
    }

    fn show(&mut self, route: Route, cx: &mut Context<Self>) {
        self.tabs[self.active].route = route;
        self.save(cx);
        cx.emit(NavEvent::Navigated);
        cx.notify();
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.can_go_back() {
            self.history_ix -= 1;
            self.show(self.history[self.history_ix].clone(), cx);
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.can_go_forward() {
            self.history_ix += 1;
            self.show(self.history[self.history_ix].clone(), cx);
        }
    }

    /// `tabs.open(path, background)`.
    pub fn open_tab(&mut self, route: Route, background: bool, cx: &mut Context<Self>) {
        let uid = self.uid();
        self.tabs.push(Tab {
            uid,
            route: route.clone(),
        });
        if background {
            self.save(cx);
            cx.notify();
        } else {
            self.active = self.tabs.len() - 1;
            self.push_history(route);
            self.save(cx);
            cx.emit(NavEvent::Navigated);
            cx.notify();
        }
    }

    pub fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.active = index;
        self.push_history(self.route().clone());
        self.save(cx);
        cx.emit(NavEvent::Navigated);
        cx.notify();
    }

    /// Reorder: `to` is the index in the list without the moved tab.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from >= self.tabs.len() {
            return;
        }
        let active_uid = self.tabs[self.active].uid;
        let tab = self.tabs.remove(from);
        let to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        self.active = self
            .tabs
            .iter()
            .position(|t| t.uid == active_uid)
            .unwrap_or(0);
        self.save(cx);
        cx.notify();
    }

    pub fn close(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let uid = self.uid();
            self.tabs.push(Tab {
                uid,
                route: Route::Home,
            });
            self.active = 0;
            self.push_history(Route::Home);
            cx.emit(NavEvent::Navigated);
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = self.active.min(self.tabs.len() - 1);
            self.push_history(self.route().clone());
            cx.emit(NavEvent::Navigated);
        }
        self.save(cx);
        cx.notify();
    }

    /// Switch to `space`, restoring its saved tab strip (`tabs.use`). When it
    /// has none, the strip starts at the space home. A workspace page opened
    /// before any space was picked (a click in the sidebar the shell shows
    /// meanwhile) stays open, in the restored strip's active tab.
    pub fn use_space(&mut self, space: &str, cx: &mut Context<Self>) {
        if self.space == space {
            return;
        }
        let opened = (self.space.is_empty() && self.route().is_workspace_page())
            .then(|| self.route().clone());
        self.save(cx);
        self.space = space.to_string();
        UiState::set(cx, ACTIVE_SPACE_KEY, &space);
        let saved = self.saved.get(space).cloned().unwrap_or_default();
        let mut tabs: Vec<Tab> = Vec::with_capacity(saved.paths.len());
        for path in &saved.paths {
            let uid = self.uid();
            tabs.push(Tab {
                uid,
                route: Route::parse(path),
            });
        }
        if tabs.is_empty() {
            let uid = self.uid();
            tabs.push(Tab {
                uid,
                route: Route::Home,
            });
        }
        self.active = saved.active.min(tabs.len() - 1);
        self.tabs = tabs;
        if let Some(route) = opened {
            self.tabs[self.active].route = route;
            self.save(cx);
        }
        self.push_history(self.route().clone());
        cx.emit(NavEvent::SpaceChanged);
        cx.emit(NavEvent::Navigated);
        cx.notify();
    }

    /// Drop a space's saved strip (space deleted or left).
    pub fn forget(&mut self, space: &str, cx: &mut Context<Self>) {
        if self.saved.remove(space).is_some() {
            UiState::set(cx, TABS_KEY, &self.saved);
        }
    }

    fn push_history(&mut self, route: Route) {
        if self.history.get(self.history_ix) == Some(&route) {
            return;
        }
        self.history.truncate(self.history_ix + 1);
        self.history.push(route);
        self.history_ix = self.history.len() - 1;
    }

    fn save(&mut self, cx: &mut App) {
        if self.space.is_empty() {
            return;
        }
        let saved = Saved {
            paths: self.tabs.iter().map(|t| t.route.path()).collect(),
            active: self.active,
        };
        self.saved.insert(self.space.clone(), saved);
        UiState::set(cx, TABS_KEY, &self.saved);
    }
}

/// Navigate the active tab from anywhere (`goto`).
pub fn navigate(route: Route, cx: &mut App) {
    Navigator::global(cx).update(cx, |nav, cx| nav.navigate(route, cx));
}

/// Open `route` in a new tab (`tabs.open`).
pub fn open_tab(route: Route, background: bool, cx: &mut App) {
    Navigator::global(cx).update(cx, |nav, cx| nav.open_tab(route, background, cx));
}

/// A space picked in the rail: its last-opened page, or - when it has none
/// yet (the space home) - its first pinned object. Pinned sections without
/// an object (`any-…`: All Objects, Favorites) are skipped.
pub fn open_space(space: &str, cx: &mut App) {
    Navigator::global(cx).update(cx, |nav, cx| nav.use_space(space, cx));
    if *Navigator::global(cx).read(cx).route() != Route::Home {
        return;
    }
    let first = {
        let store = crate::Store::global(cx).read(cx);
        store.space(space).and_then(|s| {
            let ids = s.pinned_ids.iter().filter(|id| !id.starts_with("any-"));
            ids.clone()
                .find(|id| store.summary(id).is_some())
                .or_else(|| ids.clone().next())
                .cloned()
        })
    };
    if let Some(id) = first {
        navigate(Route::Object(id), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip_through_paths() {
        for route in [
            Route::Home,
            Route::Object("a-b".into()),
            Route::Chat("c".into()),
            Route::Graph { focus: None },
        ] {
            assert_eq!(Route::parse(&route.path()), route);
        }
        assert_eq!(
            Route::parse("/app/graph?focus=x"),
            Route::Graph {
                focus: Some("x".into())
            }
        );
        assert_eq!(Route::parse("/somewhere"), Route::Home);
    }

    #[test]
    fn site_pages_parse_from_their_web_paths() {
        for route in [
            Route::Site(SitePage::Landing),
            Route::Site(SitePage::Guide),
            Route::Site(SitePage::Privacy),
            Route::Site(SitePage::Join("r2.abc".into())),
        ] {
            assert_eq!(Route::parse(&route.path()), route);
        }
        // An invite link keeps its fragment past any query; a bare /j is an
        // invalid link, not the workspace.
        assert_eq!(
            Route::parse("/j?utm=x#r2.abc"),
            Route::Site(SitePage::Join("r2.abc".into()))
        );
        assert_eq!(
            Route::parse("/j"),
            Route::Site(SitePage::Join(String::new()))
        );
        assert_eq!(Route::parse("/guide?x=1"), Route::Site(SitePage::Guide));
    }

    #[test]
    fn the_address_bar_keeps_a_query_naming_the_same_route() {
        // The join link's query survives the app settling on its route.
        let url = "/j?x=1#r2.test";
        let route = Route::parse(url);
        assert_eq!(route, Route::Site(SitePage::Join("r2.test".into())));
        assert_eq!(route.path(), "/j#r2.test");
        assert!(route.shown_by(url));
        assert!(route.shown_by(&route.path()));
        assert!(!Route::Site(SitePage::Join("r2.other".into())).shown_by(url));
        // The graph keeps its focus; any other route replaces the URL.
        let graph = Route::parse("/app/graph?focus=x");
        assert!(graph.shown_by("/app/graph?focus=x"));
        assert!(graph.shown_by("/app/graph"));
        assert!(!Route::Home.shown_by("/app/graph?focus=x"));
        assert!(Route::Site(SitePage::Guide).shown_by("/guide?x=1"));
        // An unknown path parses to the space home but is still rewritten.
        assert!(!Route::Home.shown_by("/somewhere"));
        assert!(Route::Home.shown_by("/app"));
    }

    #[test]
    fn titles_match_the_svelte_pages() {
        let site = |page| Route::Site(page).title(None);
        assert_eq!(site(SitePage::Landing), "Roostr — your things speak");
        assert_eq!(site(SitePage::Guide), "Guide — Roostr");
        assert_eq!(site(SitePage::Privacy), "Privacy — Roostr");
        assert_eq!(site(SitePage::Join("r2.x".into())), "Join a space — Roostr");
        assert_eq!(Route::Graph { focus: None }.title(None), "Graph — Roostr");
        assert_eq!(Route::Home.title(None), "Roostr");
        assert_eq!(Route::Chat("c".into()).title(Some("Plans")), "Roostr");
        let object = Route::Object("o".into());
        assert_eq!(object.title(None), "Loading… — glon");
        assert_eq!(object.title(Some("")), "Untitled — glon");
        assert_eq!(object.title(Some("Plans")), "Plans — glon");
    }
}
