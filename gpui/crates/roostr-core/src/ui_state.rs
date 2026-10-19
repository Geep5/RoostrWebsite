//! Device-local UI state: the native counterpart of the web app's
//! `localStorage` keys (`rail-width`, `side-width`, `nav-hidden`,
//! `widget-collapsed`, `section-collapsed`, `glon.channel`,
//! `roostr.tabs.spaces`, `disc-drawer-w`, ...). One JSON object in the
//! platform config dir, rewritten on every change. Never synced.
//!
//! In the browser the object lives in `localStorage` under its own key
//! ([`WEB_KEY`]), apart from the Svelte app's keys on the same origin.

#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;

use gpui_kit::{App, Global};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value as Json};

/// The `localStorage` key of the browser build's UI state.
#[cfg(target_family = "wasm")]
pub const WEB_KEY: &str = "roostr-gpui.ui-state";

pub struct UiState {
    #[cfg(not(target_family = "wasm"))]
    path: Option<PathBuf>,
    values: Map<String, Json>,
}

impl Global for UiState {}

impl UiState {
    /// Load [`WEB_KEY`] from `localStorage` as a global.
    #[cfg(target_family = "wasm")]
    pub fn init(cx: &mut App) {
        let values = web_storage()
            .and_then(|s| s.get_item(WEB_KEY).ok().flatten())
            .and_then(|raw| serde_json::from_str::<Map<String, Json>>(&raw).ok())
            .unwrap_or_default();
        cx.set_global(Self { values });
    }

    /// Load `<config dir>/Roostr/ui-state.json` (or `ROOSTR_UI_STATE`) as a global.
    #[cfg(not(target_family = "wasm"))]
    pub fn init(cx: &mut App) {
        let path = std::env::var_os("ROOSTR_UI_STATE")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|d| d.join("Roostr").join("ui-state.json")));
        let values = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Map<String, Json>>(&bytes).ok())
            .unwrap_or_default();
        cx.set_global(Self { path, values });
    }

    /// The stored value for `key`, when present and of the expected shape.
    pub fn get<T: DeserializeOwned>(cx: &App, key: &str) -> Option<T> {
        let value = cx.try_global::<Self>()?.values.get(key)?.clone();
        serde_json::from_value(value).ok()
    }

    pub fn set<T: Serialize>(cx: &mut App, key: &str, value: &T) {
        let Ok(value) = serde_json::to_value(value) else {
            return;
        };
        let state = cx.global_mut::<Self>();
        if state.values.get(key) == Some(&value) {
            return;
        }
        state.values.insert(key.to_string(), value);
        state.save();
    }

    pub fn remove(cx: &mut App, key: &str) {
        let state = cx.global_mut::<Self>();
        if state.values.remove(key).is_some() {
            state.save();
        }
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
