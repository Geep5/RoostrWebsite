//! Navigation: routes, the per-space tab strip (`src/lib/tabs.svelte.ts`) and
//! one global back/forward history (the browser history the web app uses).
//!
//! Navigating replaces the active tab's route (browser model). Which tabs
//! each space has, in order, is shared by this person's devices: the strips
//! live in the private preferences object's [`OPEN_TABS_FIELD`], written
//! [`TABS_WRITE_DELAY`] after the last change (when they differ from what is
//! stored) and read back at every refresh, where another device's change
//! replaces the strip in place. Which tab is selected is this device's own:
//! it persists, with a copy of the strips, in [`UiState`] under
//! `roostr.tabs.spaces`. A tab whose object is binned or vanishes closes.
//!
//! On a phone ([`Navigator::set_phone`]) moving around does not touch the
//! tabs: a screen opened from a list shows outside them, and only opening
//! or closing a tab changes the strip.
//!
//! The navigator is [`crate::State::nav`]; methods that persist take the
//! state's [`crate::State::ui`] (`s.nav.back(&mut s.ui)`), and what it emits
//! ([`NavEvent`]) reaches the [`crate::State::on_nav`] listeners at the next
//! [`crate::State::poll`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use roostr_rt::Task;
use serde::{Deserialize, Serialize};

use crate::State;
use crate::types::Value;
use crate::ui_state::UiState;

const TABS_KEY: &str = "roostr.tabs.spaces";
const ACTIVE_SPACE_KEY: &str = "glon.channel";
/// The space home's path: a strip of only this tab has nothing open.
const HOME_PATH: &str = "/app";

/// The preferences field holding the open tabs ([`OpenTabs`]).
pub const OPEN_TABS_FIELD: &str = "open_tabs";
/// A change to the tabs is written this long after the last one.
pub const TABS_WRITE_DELAY: Duration = Duration::from_secs(2);

/// The open tabs this person's devices share: space id -> its strip's route
/// paths ([`Route::path`]), in order. A space with nothing open has no entry.
pub type OpenTabs = BTreeMap<String, Vec<String>>;

/// `open_tabs` as the preferences store it: a map of string lists.
pub fn open_tabs_value(tabs: &OpenTabs) -> Value {
    Value::map(
        tabs.iter()
            .map(|(space, paths)| {
                let paths = paths.iter().map(Value::string).collect();
                (space.clone(), Value::values(paths))
            })
            .collect(),
    )
}

/// The open tabs a stored `open_tabs` holds; strips with nothing open drop out.
pub fn open_tabs_from(value: &Value) -> OpenTabs {
    let Some(entries) = value.entries() else {
        return OpenTabs::new();
    };
    entries
        .iter()
        .map(|(space, paths)| {
            let paths: Vec<String> = paths
                .items()
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            (space.clone(), paths)
        })
        .filter(|(_, paths)| !is_empty_strip(paths))
        .collect()
}

/// A strip with nothing open: no tabs, or the one space-home tab.
fn is_empty_strip(paths: &[String]) -> bool {
    paths.is_empty() || paths == [HOME_PATH]
}

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
            Self::Site(SitePage::Landing) => {
                "Roostr — the workspace people and agents share".into()
            }
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

/// A space's strip as this device keeps it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// The strip's route paths: this device's copy of the shared list.
    paths: Vec<String>,
    /// The selected tab (this device's own; never shared).
    active: usize,
    /// The selected tab's path: the selection by route, where the strip
    /// changed since.
    active_path: String,
    /// `paths` follows the shared list. A strip saved before tabs synced (or
    /// changed before this launch first read them) merges into the shared
    /// list once, at the first read.
    synced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavEvent {
    /// The visible route changed (navigation, tab switch, back/forward).
    Navigated,
    /// The active space changed.
    SpaceChanged,
    /// The tab strip changed while the visible route stayed (a tab opened in
    /// the background or closed, another device's change).
    TabsChanged,
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
    /// What the preferences hold, as last read or written; `None` until the
    /// first read of this vault.
    pub(crate) stored: Option<OpenTabs>,
    /// A strip changed since the write was last scheduled.
    pub(crate) changed: bool,
    /// The debounced write ([`TABS_WRITE_DELAY`]); dropping it cancels.
    pub(crate) write: Option<Task<()>>,
    /// Objects found binned or vanished this session: a stale list from
    /// another device does not bring their tabs back.
    binned: HashSet<String>,
    /// The phone shell shows: moving around leaves the tabs alone.
    phone: bool,
    /// On a phone, the route shown outside the tabs (a screen opened from a
    /// list, the space home); `None` while a tab shows.
    loose: Option<Route>,
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
            stored: None,
            changed: false,
            write: None,
            binned: HashSet::new(),
            phone: false,
            loose: None,
            events: Vec::new(),
        }
    }

    /// The space the app last had open (`glon.channel`).
    pub fn remembered_space(ui: &UiState) -> Option<String> {
        ui.get(ACTIVE_SPACE_KEY)
    }

    /// The route shown: the active tab's, or on a phone the screen opened
    /// outside the tabs.
    pub fn route(&self) -> &Route {
        self.loose.as_ref().unwrap_or(&self.tabs[self.active].route)
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// A tab shows (not, on a phone, a screen opened outside the tabs).
    pub fn in_tab(&self) -> bool {
        self.loose.is_none()
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

    /// Go to `route` in the active tab (`goto` + `tabs.sync`); on a phone,
    /// outside the tabs.
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
        self.reopened(&route);
        if self.phone {
            self.loose = Some(route);
        } else if let Some(open) = self.tab_showing(&route) {
            // Already open in another tab: go there rather than open it twice.
            self.active = open;
            self.save(ui);
        } else {
            self.tabs[self.active].route = route;
            self.save(ui);
        }
        self.events.push(NavEvent::Navigated);
    }

    /// The tab other than the active one that already shows `route`, for a route that gets
    /// at most one tab ([`one_tab`]).
    fn tab_showing(&self, route: &Route) -> Option<usize> {
        if !one_tab(route) {
            return None;
        }
        self.tabs
            .iter()
            .enumerate()
            .find(|(i, t)| *i != self.active && t.route == *route)
            .map(|(i, _)| i)
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
        self.reopened(&route);
        // Already open: show that tab (a background open has nothing to do).
        if one_tab(&route)
            && let Some(open) = self.tabs.iter().position(|t| t.route == route)
        {
            if !background {
                self.activate(open, ui);
            }
            return;
        }
        let uid = self.uid();
        self.tabs.push(Tab {
            uid,
            route: route.clone(),
        });
        if background {
            self.save(ui);
            self.events.push(NavEvent::TabsChanged);
        } else {
            self.active = self.tabs.len() - 1;
            self.loose = None;
            self.push_history(route);
            self.save(ui);
            self.events.push(NavEvent::Navigated);
        }
    }

    /// Show tab `index` (on a phone, also when it is the selected tab while
    /// a screen outside the tabs shows).
    pub fn activate(&mut self, index: usize, ui: &mut UiState) {
        if index >= self.tabs.len() || (index == self.active && self.loose.is_none()) {
            return;
        }
        self.active = index;
        self.loose = None;
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

    /// Close tab `index`. The selection moves to its right neighbour, else
    /// its left; closing the last tab leaves the space home. On a phone the
    /// screen stays.
    pub fn close(&mut self, index: usize, ui: &mut UiState) {
        if index >= self.tabs.len() {
            return;
        }
        let before = self.route().clone();
        if self.phone && index == self.active && self.loose.is_none() {
            self.loose = Some(before.clone());
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let uid = self.uid();
            self.tabs.push(Tab {
                uid,
                route: Route::Home,
            });
            self.active = 0;
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = self.active.min(self.tabs.len() - 1);
        }
        self.save(ui);
        self.strip_changed(before);
    }

    /// Switch to `space`, restoring its saved tab strip (`tabs.use`) and
    /// this device's selection in it. When it has none, the strip starts at
    /// the space home. A workspace page opened before any space was picked
    /// (a click in the sidebar the shell shows meanwhile) stays open, in the
    /// restored strip's active tab.
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
        let paths = unique_paths(&saved.paths);
        let mut tabs: Vec<Tab> = Vec::with_capacity(paths.len());
        for path in &paths {
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
        self.active = tabs
            .iter()
            .position(|t| !saved.active_path.is_empty() && t.route.path() == saved.active_path)
            .unwrap_or(saved.active.min(tabs.len() - 1));
        self.tabs = tabs;
        self.loose = None;
        if let Some(route) = opened {
            match self.tab_showing(&route) {
                Some(open) => self.active = open,
                None => self.tabs[self.active].route = route,
            }
            self.save(ui);
        }
        self.push_history(self.route().clone());
        self.events.push(NavEvent::SpaceChanged);
        self.events.push(NavEvent::Navigated);
    }

    /// Drop a space's saved strip (space deleted or left).
    pub fn forget(&mut self, space: &str, ui: &mut UiState) {
        if self.saved.remove(space).is_some() {
            self.changed = true;
            ui.set(TABS_KEY, &self.saved);
        }
    }

    /// The phone shell shows (`true`) or the desktop's does. Back on the
    /// desktop, a screen a phone showed outside the tabs shows in them: in
    /// the tab showing its route, else in the active tab (the space home
    /// leaves the active tab as it was).
    pub fn set_phone(&mut self, phone: bool, ui: &mut UiState) {
        if self.phone == phone {
            return;
        }
        self.phone = phone;
        let Some(route) = self.loose.take() else {
            return;
        };
        if let Some(index) = self.tabs.iter().position(|t| t.route == route) {
            self.active = index;
        } else if route.is_workspace_page() {
            self.tabs[self.active].route = route.clone();
        }
        self.save(ui);
        self.strip_changed(route);
    }

    /// The shared tab lists as this device knows them.
    pub fn open_tabs(&self) -> OpenTabs {
        self.saved
            .iter()
            .filter(|(_, saved)| saved.synced && !is_empty_strip(&saved.paths))
            .map(|(space, saved)| (space.clone(), saved.paths.clone()))
            .collect()
    }

    /// A new vault opened: its preferences are unread.
    pub(crate) fn vault_opened(&mut self) {
        self.stored = None;
        self.changed = false;
        self.write = None;
        self.binned.clear();
    }

    /// The preferences' open tabs as a refresh read them (`None`: they could
    /// not be read). The first read merges in, once, the strips this device
    /// saved before tabs synced; later, another device's change replaces
    /// the strips in place - except the shown one while a change of this
    /// device waits for its write.
    pub(crate) fn landed(&mut self, remote: Option<OpenTabs>, ui: &mut UiState) {
        let mut remote = match (remote, &self.stored) {
            (Some(remote), _) => remote,
            // No preferences yet: nothing is shared so far.
            (None, None) => OpenTabs::new(),
            // A failed read changes nothing.
            (None, Some(_)) => return,
        };
        if !self.binned.is_empty() {
            for paths in remote.values_mut() {
                paths.retain(|p| !shows_any(p, &self.binned));
            }
            remote.retain(|_, paths| !is_empty_strip(paths));
        }
        match &self.stored {
            Some(stored) if *stored == remote => return,
            Some(_) => {
                let keep = self.write.is_some().then(|| self.space.clone());
                let spaces: HashSet<String> =
                    self.saved.keys().chain(remote.keys()).cloned().collect();
                for space in spaces {
                    if keep.as_ref() == Some(&space) {
                        continue;
                    }
                    let paths = remote.get(&space).cloned().unwrap_or_default();
                    let saved = self.saved.entry(space).or_default();
                    saved.paths = paths;
                    saved.synced = true;
                }
            }
            None => {
                for (space, saved) in &mut self.saved {
                    let mut paths = remote.get(space).cloned().unwrap_or_default();
                    if !saved.synced {
                        for path in &saved.paths {
                            if path != HOME_PATH && !paths.contains(path) {
                                paths.push(path.clone());
                            }
                        }
                    }
                    saved.paths = paths;
                    saved.synced = true;
                }
                for (space, paths) in &remote {
                    self.saved.entry(space.clone()).or_insert_with(|| Saved {
                        paths: paths.clone(),
                        synced: true,
                        ..Saved::default()
                    });
                }
            }
        }
        if self.open_tabs() != remote {
            self.changed = true;
        }
        self.stored = Some(remote);
        self.restore_strip();
        ui.set(TABS_KEY, &self.saved);
    }

    /// Whether a tab of any space shows object `id`.
    pub(crate) fn has_object_tab(&self, id: &str) -> bool {
        let ids = HashSet::from([id.to_string()]);
        self.tabs.iter().any(|t| shows_any(&t.route.path(), &ids))
            || self
                .saved
                .values()
                .any(|saved| saved.paths.iter().any(|p| shows_any(p, &ids)))
    }

    /// Objects `ids` were binned or vanished: their tabs close, in every space.
    pub(crate) fn close_objects(&mut self, ids: &[String], ui: &mut UiState) {
        let ids: HashSet<String> = ids.iter().cloned().collect();
        self.binned.extend(ids.iter().cloned());
        let mut closed = false;
        for saved in self.saved.values_mut() {
            let before = saved.paths.len();
            saved.paths.retain(|p| !shows_any(p, &ids));
            closed |= saved.paths.len() != before;
        }
        if !closed {
            return;
        }
        self.changed = true;
        self.restore_strip();
        ui.set(TABS_KEY, &self.saved);
    }

    /// An object opened again is no longer held closed.
    fn reopened(&mut self, route: &Route) {
        if let Route::Object(id) | Route::Chat(id) = route {
            self.binned.remove(id);
        }
    }

    /// Show the active space's saved strip, keeping each tab still in it
    /// (by route) and the selection: the selected tab, else its nearest
    /// neighbour still open (right first), else the same place. On a phone
    /// the screen stays.
    fn restore_strip(&mut self) {
        if self.space.is_empty() {
            return;
        }
        let paths: Vec<String> = match self.saved.get(&self.space) {
            Some(saved) if !saved.paths.is_empty() => unique_paths(&saved.paths),
            _ => vec![HOME_PATH.to_string()],
        };
        let old: Vec<String> = self.tabs.iter().map(|t| t.route.path()).collect();
        if old == paths {
            return;
        }
        let before = self.route().clone();
        // Each old tab claims its route's first free place, the selected tab first.
        let mut slots: Vec<Option<Tab>> = vec![None; paths.len()];
        let mut kept_at: Vec<Option<usize>> = vec![None; old.len()];
        let others = (0..old.len()).filter(|i| *i != self.active);
        for i in std::iter::once(self.active).chain(others) {
            let free = (0..paths.len()).find(|j| slots[*j].is_none() && paths[*j] == old[i]);
            if let Some(j) = free {
                slots[j] = Some(self.tabs[i].clone());
                kept_at[i] = Some(j);
            }
        }
        let a = self.active;
        let active = (0..old.len())
            .flat_map(|d| [a.checked_add(d), a.checked_sub(d)])
            .flatten()
            .find_map(|i| kept_at.get(i).copied().flatten())
            .unwrap_or(a.min(paths.len() - 1));
        let mut tabs = Vec::with_capacity(paths.len());
        for (slot, path) in slots.into_iter().zip(&paths) {
            let tab = match slot {
                Some(tab) => tab,
                None => Tab {
                    uid: self.uid(),
                    route: Route::parse(path),
                },
            };
            tabs.push(tab);
        }
        if self.phone && kept_at[a].is_none() && self.loose.is_none() {
            self.loose = Some(before.clone());
        }
        self.tabs = tabs;
        self.active = active;
        if let Some(saved) = self.saved.get_mut(&self.space) {
            saved.active = active;
            saved.active_path = self.tabs[active].route.path();
        }
        self.strip_changed(before);
    }

    /// The strip changed; `before` is the route shown until now.
    fn strip_changed(&mut self, before: Route) {
        if *self.route() != before {
            self.push_history(self.route().clone());
            self.events.push(NavEvent::Navigated);
        } else {
            self.events.push(NavEvent::TabsChanged);
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

    /// Keep the shown strip and its selection. A changed strip is shared
    /// ([`Self::changed`]); changed before this launch read the shared
    /// lists, it merges into them at the first read.
    fn save(&mut self, ui: &mut UiState) {
        if self.space.is_empty() {
            return;
        }
        let paths: Vec<String> = self.tabs.iter().map(|t| t.route.path()).collect();
        let read = self.stored.is_some();
        let saved = self.saved.entry(self.space.clone()).or_default();
        if saved.paths != paths {
            saved.paths = paths;
            saved.synced = read;
            self.changed = true;
        }
        saved.active = self.active;
        saved.active_path = self.tabs[self.active].route.path();
        ui.set(TABS_KEY, &self.saved);
    }
}

/// Whether `path` shows one of the objects `ids` (an object page or its chat).
fn shows_any(path: &str, ids: &HashSet<String>) -> bool {
    matches!(Route::parse(path), Route::Object(id) | Route::Chat(id) if ids.contains(&id))
}

/// Hand the navigator what the store learned - the preferences' open tabs,
/// objects gone from the collections or found binned - and (re)start the
/// debounced write of changed strips. [`State::poll`] runs it.
pub(crate) fn sync_tabs(s: &mut State) {
    if let Some(remote) = s.store.open_tabs.take() {
        s.nav.landed(remote, &mut s.ui);
    }
    for id in std::mem::take(&mut s.store.missing) {
        if !s.nav.has_object_tab(&id) {
            continue;
        }
        // Gone from the collections: binned, or vanished for good.
        let fetch = s.store.fetch_object(&id);
        s.rt.spawn(async move |cx| {
            if fetch.await.map_or(true, |object| object.deleted) {
                cx.update(|s| s.store.binned.push(id));
            }
        })
        .detach();
    }
    let binned = std::mem::take(&mut s.store.binned);
    if !binned.is_empty() {
        s.nav.close_objects(&binned, &mut s.ui);
    }
    if std::mem::take(&mut s.nav.changed) && s.nav.stored.is_some() {
        // Timed from now, not from when the task first runs.
        let timer = s.rt.timer(TABS_WRITE_DELAY);
        s.nav.write = Some(s.rt.spawn(async move |cx| {
            timer.await;
            cx.update(write_tabs);
        }));
    }
}

/// Write the shared tab lists, unless the preferences hold them already.
fn write_tabs(s: &mut State) {
    s.nav.write = None;
    let tabs = s.nav.open_tabs();
    if s.nav.stored.as_ref() == Some(&tabs) {
        return;
    }
    let previous = s.nav.stored.replace(tabs.clone());
    s.store.prefs_epoch += 1;
    let task = crate::api::user_prefs::set_open_tabs(&tabs, s);
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            s.store.prefs_epoch += 1;
            if let Err(error) = result {
                log::warn!("saving the open tabs failed: {error:#}");
                // The next change writes them again; a read meanwhile keeps the strips.
                if s.nav.stored.as_ref() == Some(&tabs) {
                    s.nav.stored = previous;
                }
            }
        });
    })
    .detach();
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

/// `paths` with a repeated page's later tabs dropped (a list saved before tabs were one per
/// page may repeat one).
fn unique_paths(paths: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    paths
        .iter()
        .filter(|p| !one_tab(&Route::parse(p)) || seen.insert(p.as_str()))
        .cloned()
        .collect()
}

/// Whether `route` gets at most one tab: an object, a chat or the graph. The space home and
/// the website pages can repeat (New tab opens a home).
fn one_tab(route: &Route) -> bool {
    !matches!(route, Route::Home | Route::Site(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A navigator over in-memory UI state (nothing on disk), showing space `sp`.
    fn navigator() -> (Navigator, crate::UiState) {
        let mut ui = crate::UiState::load_from(None);
        let mut nav = Navigator::new(&ui);
        nav.use_space("sp", &mut ui);
        (nav, ui)
    }

    fn routes(nav: &Navigator) -> Vec<String> {
        nav.tabs().iter().map(|t| t.route.path()).collect()
    }

    #[test]
    fn a_page_already_open_in_a_tab_is_shown_there_not_opened_twice() {
        let (mut nav, mut ui) = navigator();
        let a = Route::Object("a".into());
        let b = Route::Object("b".into());
        nav.navigate(a.clone(), &mut ui);
        nav.open_tab(b.clone(), false, &mut ui);
        assert_eq!(routes(&nav), ["/app/object/a", "/app/object/b"]);
        // Following a link to `a` from `b`'s tab goes to `a`'s tab; `b`'s tab stays `b`.
        nav.navigate(a.clone(), &mut ui);
        assert_eq!(routes(&nav), ["/app/object/a", "/app/object/b"]);
        assert_eq!(nav.active_index(), 0);
        // Opening `b` in a new tab shows its tab; in the background, nothing changes.
        nav.open_tab(b.clone(), false, &mut ui);
        assert_eq!((routes(&nav).len(), nav.active_index()), (2, 1));
        nav.open_tab(a.clone(), true, &mut ui);
        assert_eq!((routes(&nav).len(), nav.active_index()), (2, 1));
        // Back returns to `a`'s tab rather than turning `b`'s tab into a second `a`.
        nav.back(&mut ui);
        assert_eq!(routes(&nav), ["/app/object/a", "/app/object/b"]);
        assert_eq!(nav.active_index(), 0);
        // The space home can repeat: New tab opens another.
        nav.open_tab(Route::Home, false, &mut ui);
        nav.open_tab(Route::Home, false, &mut ui);
        assert_eq!(routes(&nav).iter().filter(|p| *p == "/app").count(), 2);
    }

    #[test]
    fn a_saved_list_with_a_repeated_page_keeps_its_first() {
        let mut ui = crate::UiState::load_from(None);
        ui.set(
            TABS_KEY,
            &serde_json::json!({ "sp": { "paths": ["/app/object/a", "/app/object/b", "/app/object/a", "/app", "/app"], "active": 2 } }),
        );
        let mut nav = Navigator::new(&ui);
        nav.use_space("sp", &mut ui);
        assert_eq!(
            routes(&nav),
            ["/app/object/a", "/app/object/b", "/app", "/app"]
        );
    }

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
        assert_eq!(
            site(SitePage::Landing),
            "Roostr — the workspace people and agents share"
        );
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
