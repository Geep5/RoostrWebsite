//! Objects this device opened, most recent first (`src/lib/recent-opened.ts`).
//! Device-local on purpose: kept in [`UiState`], never synced.

use std::collections::HashMap;

use gpui_kit::App;

use crate::ui_state::UiState;

const KEY: &str = "roostr-recent-opened";
const LIMIT: usize = 200;

fn read(cx: &App) -> Vec<String> {
    UiState::get::<Vec<serde_json::Value>>(cx, KEY)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// Record that `id` was just opened.
pub fn mark_opened(id: &str, cx: &mut App) {
    let mut ids = vec![id.to_string()];
    ids.extend(read(cx).into_iter().filter(|x| x != id));
    ids.truncate(LIMIT);
    UiState::set(cx, KEY, &ids);
}

/// id → position (0 = most recently opened); unopened ids are absent.
pub fn opened_ranks(cx: &App) -> HashMap<String, usize> {
    read(cx)
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect()
}
