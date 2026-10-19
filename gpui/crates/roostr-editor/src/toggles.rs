//! Toggle-block open state (`src/lib/toggles.ts`): kept per device, never in
//! the DAG - whether a toggle is expanded is a viewing preference, not
//! content. Stored under the web's `toggles-<objectId>` key as
//! `{ blockId: true }`.

use std::collections::BTreeMap;

use gpui_kit::App;
use roostr_core::UiState;

fn key(object_id: &str) -> String {
    format!("toggles-{object_id}")
}

fn read(object_id: &str, cx: &App) -> BTreeMap<String, bool> {
    UiState::get(cx, &key(object_id)).unwrap_or_default()
}

pub fn is_toggle_open(object_id: &str, block_id: &str, cx: &App) -> bool {
    read(object_id, cx).get(block_id).copied().unwrap_or(false)
}

pub fn set_toggle_open(object_id: &str, block_id: &str, open: bool, cx: &mut App) {
    let mut m = read(object_id, cx);
    if open {
        m.insert(block_id.to_string(), true);
    } else {
        m.remove(block_id);
    }
    UiState::set(cx, &key(object_id), &m);
}
