//! Device-local UI state: the native counterpart of the web app's
//! `localStorage` keys (`rail-width`, `side-width`, `nav-hidden`,
//! `section-collapsed`, `glon.channel`, `roostr.tabs.spaces`, `disc-drawer-w`,
//! `pane-props-w`, `pane-chat-open`, ...). One JSON object in the platform
//! config dir, rewritten on every change. Never synced.
//!
//! In the browser the object lives in `localStorage` under its own key
//! ([`WEB_KEY`]), apart from the Svelte app's keys on the same origin.

#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value as Json};

/// The `localStorage` key of the browser build's UI state.
#[cfg(target_family = "wasm")]
pub const WEB_KEY: &str = "roostr-gpui.ui-state";

/// The key of the colour theme preference ([`UiState::theme_name`]).
pub const THEME_KEY: &str = "theme";

/// The stored UI state ([`crate::State::ui`]).
pub struct UiState {
    #[cfg(not(target_family = "wasm"))]
    path: Option<PathBuf>,
    values: Map<String, Json>,
}

impl UiState {
    /// Load [`WEB_KEY`] from `localStorage`.
    #[cfg(target_family = "wasm")]
    pub fn load() -> Self {
        let values = web_storage()
            .and_then(|s| s.get_item(WEB_KEY).ok().flatten())
            .and_then(|raw| serde_json::from_str::<Map<String, Json>>(&raw).ok())
            .unwrap_or_default();
        Self { values }
    }

    /// Load `<config dir>/Roostr/ui-state.json` (or `ROOSTR_UI_STATE`).
    #[cfg(not(target_family = "wasm"))]
    pub fn load() -> Self {
        let path = std::env::var_os("ROOSTR_UI_STATE")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|d| d.join("Roostr").join("ui-state.json")));
        Self::load_from(path)
    }

    /// Load the state kept at `path` (`None`: nothing is kept).
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn load_from(path: Option<PathBuf>) -> Self {
        let values = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Map<String, Json>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, values }
    }

    /// The stored value for `key`, when present and of the expected shape.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let value = self.values.get(key)?.clone();
        serde_json::from_value(value).ok()
    }

    pub fn set<T: Serialize>(&mut self, key: &str, value: &T) {
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        if self.values.get(key) == Some(&value) {
            return;
        }
        self.values.insert(key.to_string(), value);
        self.save();
    }

    pub fn remove(&mut self, key: &str) {
        if self.values.remove(key).is_some() {
            self.save();
        }
    }

    /// The colour theme the person picked in Settings (`"dark"`, `"light"` or `"system"`;
    /// roostr-ui's `ThemePref::from_name` reads it). `"dark"`, today's look, until one is picked.
    pub fn theme_name(&self) -> &str {
        self.values
            .get(THEME_KEY)
            .and_then(Json::as_str)
            .unwrap_or("dark")
    }

    /// Remember the picked colour theme (see [`Self::theme_name`]).
    pub fn set_theme_name(&mut self, name: &str) {
        self.set(THEME_KEY, &name);
    }

    #[cfg(target_family = "wasm")]
    fn save(&self) {
        let Some(storage) = web_storage() else { return };
        match serde_json::to_string(&self.values) {
            Ok(raw) => {
                if storage.set_item(WEB_KEY, &raw).is_err() {
                    log::warn!("could not save UI state to localStorage");
                }
            }
            Err(error) => log::warn!("could not encode UI state: {error}"),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_vec_pretty(&self.values) {
            Ok(bytes) => {
                // Write-then-rename so a crash never leaves half a file.
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes)
                    .and_then(|()| std::fs::rename(&tmp, path))
                    .is_err()
                {
                    log::warn!("could not save UI state to {}", path.display());
                }
            }
            Err(error) => log::warn!("could not encode UI state: {error}"),
        }
    }
}

#[cfg(target_family = "wasm")]
fn web_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}
