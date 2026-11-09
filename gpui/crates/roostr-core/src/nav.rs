//! Navigation: routes, the per-space tab strip (`src/lib/tabs.svelte.ts`) and
//! one global back/forward history (the browser history the web app uses).
//!
//! Navigating replaces the active tab's route (browser model); tabs persist
//! per space in [`UiState`] under `roostr.tabs.spaces`. The navigator is
//! [`crate::State::nav`]; methods that persist take the state's
//! [`crate::State::ui`] (`s.nav.back(&mut s.ui)`), and what it emits
//! ([`NavEvent`]) reaches the [`crate::State::on_nav`] listeners at the next
//! [`crate::State::poll`].

use std::collections::HashMap;

use crate::State;
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
            Self::Site(SitePage::Landing) => "Roostr — your team, human and not".into(),
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
    /// Emitted, not yet handed to the listeners ([`State::poll`]).
    pub(crate) events: Vec<NavEvent>,
}

impl Navigator {
    /// A navigator at the space home with the tab strips `ui` saved. The
    /// [`State`] makes its own; a browser tab that shows a website page
    /// before its vault opens makes one first and moves it into the state
    /// (`s.nav = nav`) so the page's history stays.
    pub fn new(ui: &UiState) -> Self {
        Self {
            tabs: vec![Tab {
                uid: 1,
                route: Route::Home,
            }],
            active: 0,
            space: String::new(),
            saved: ui.get(TABS_KEY).unwrap_or_default(),
            history: vec![Route::Home],
            history_ix: 0,
            next_uid: 2,
            events: Vec::new(),
        }
    }

    /// The space the app last had open (`glon.channel`).
    pub fn remembered_space(ui: &UiState) -> Option<String> {
        ui.get(ACTIVE_SPACE_KEY)
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
    pub fn navigate(&mut self, route: Route, ui: &mut UiState) {
        if *self.route() == route {
            return;
        }
        self.history.truncate(self.history_ix + 1);
        self.history.push(route.clone());
        self.history_ix = self.history.len() - 1;
        self.show(route, ui);
    }

    fn show(&mut self, route: Route, ui: &mut UiState) {
        self.tabs[self.active].route = route;
        self.save(ui);
        self.events.push(NavEvent::Navigated);
    }

    /// Show `route` in the active tab in place of the current route, in
    /// history too: Back does not return to what it replaced.
    pub fn replace(&mut self, route: Route, ui: &mut UiState) {
        if *self.route() == route {
            return;
        }
        if let Some(entry) = self.history.get_mut(self.history_ix) {
            *entry = route.clone();
        }
        self.show(route, ui);
    }

    pub fn back(&mut self, ui: &mut UiState) {
        if self.can_go_back() {
            self.history_ix -= 1;
            self.show(self.history[self.history_ix].clone(), ui);
        }
    }

    pub fn forward(&mut self, ui: &mut UiState) {
        if self.can_go_forward() {
            self.history_ix += 1;
            self.show(self.history[self.history_ix].clone(), ui);
        }
    }

    /// `tabs.open(path, background)`.
    pub fn open_tab(&mut self, route: Route, background: bool, ui: &mut UiState) {
        let uid = self.uid();
        self.tabs.push(Tab {
            uid,
            route: route.clone(),
        });
        if background {
            self.save(ui);
        } else {
            self.active = self.tabs.len() - 1;
            self.push_history(route);
            self.save(ui);
            self.events.push(NavEvent::Navigated);
        }
    }

    pub fn activate(&mut self, index: usize, ui: &mut UiState) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.active = index;
        self.push_history(self.route().clone());
        self.save(ui);
        self.events.push(NavEvent::Navigated);
    }

    /// Reorder: `to` is the index in the list without the moved tab.
    pub fn move_tab(&mut self, from: usize, to: usize, ui: &mut UiState) {
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
        self.save(ui);
    }

    pub fn close(&mut self, index: usize, ui: &mut UiState) {
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
            self.events.push(NavEvent::Navigated);
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = self.active.min(self.tabs.len() - 1);
            self.push_history(self.route().clone());
            self.events.push(NavEvent::Navigated);
        }
        self.save(ui);
    }

    /// Switch to `space`, restoring its saved tab strip (`tabs.use`). When it
    /// has none, the strip starts at the space home. A workspace page opened
    /// before any space was picked (a click in the sidebar the shell shows
    /// meanwhile) stays open, in the restored strip's active tab.
    pub fn use_space(&mut self, space: &str, ui: &mut UiState) {
        if self.space == space {
            return;
        }
        let opened = (self.space.is_empty() && self.route().is_workspace_page())
            .then(|| self.route().clone());
        self.save(ui);
        self.space = space.to_string();
        ui.set(ACTIVE_SPACE_KEY, &space);
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
            self.save(ui);
        }
        self.push_history(self.route().clone());
        self.events.push(NavEvent::SpaceChanged);
        self.events.push(NavEvent::Navigated);
    }

    /// Drop a space's saved strip (space deleted or left).
    pub fn forget(&mut self, space: &str, ui: &mut UiState) {
        if self.saved.remove(space).is_some() {
            ui.set(TABS_KEY, &self.saved);
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

    fn save(&mut self, ui: &mut UiState) {
        if self.space.is_empty() {
            return;
        }
        let saved = Saved {
            paths: self.tabs.iter().map(|t| t.route.path()).collect(),
            active: self.active,
        };
        self.saved.insert(self.space.clone(), saved);
        ui.set(TABS_KEY, &self.saved);
    }
}

/// Navigate the active tab from anywhere (`goto`).
pub fn navigate(route: Route, s: &mut State) {
    s.nav.navigate(route, &mut s.ui);
}

/// Open `route` in a new tab (`tabs.open`).
pub fn open_tab(route: Route, background: bool, s: &mut State) {
    s.nav.open_tab(route, background, &mut s.ui);
}

/// A space's top pinned object, as its sidebar lists them: what it shows
/// when nothing is open. Pins with no object here (a section such as All
/// Objects, or one not synced yet) are skipped, as the sidebar skips them.
pub fn top_pin(space: &str, s: &State) -> Option<String> {
    let store = &s.store;
    store
        .space(space)?
        .pinned_ids
        .iter()
        .find(|id| store.summary(id).is_some())
        .cloned()
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
        assert_eq!(site(SitePage::Landing), "Roostr — your team, human and not");
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
