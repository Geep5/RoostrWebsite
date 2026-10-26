//! What the UI says and does on a lite replica ([`crate::lite`]): the words
//! of the "Load this space fully" fallbacks, the vault estimate, the graph's
//! shadow counts, and the actions behind their buttons. Every feature crate
//! renders the same notices from here, desktop and mobile alike.
//!
//! An opened query/set (pinned widget or page) over a space not held whole
//! downloads that space on its own ([`ensure_space`]); everything else
//! (search, backlinks, counts, agents, the graph) offers the button.

use std::collections::{HashMap, HashSet};

use gpui_kit::{AnyElement, App, ElementId, FontWeight, Global, SharedString, div, prelude::*, px};

use crate::lite::{self, VaultEstimate};
use crate::sync_status::count;
use crate::types::{LiteStatus, SpaceLoad};
use crate::{Store, io, theme, ui};

pub const LOAD_SPACE: &str = "Load this space fully";
pub const SPACE_UNREADY: &str = "This space isn't ready for lite yet — load it fully once";
pub const VAULT_UNREADY: &str = "This vault isn't ready for lite yet — load it fully once";

// ------------------------------------------------------------------ words

/// "Loading this space · 1,234 of 5,000" (or "· 1,234 received" while the
/// expected count is unknown).
pub fn loading_text(load: &SpaceLoad) -> String {
    if load.expected > 0 {
        format!(
            "Loading this space · {} of {}",
            count(load.received.min(load.expected)),
            count(load.expected)
        )
    } else {
        format!("Loading this space · {} received", count(load.received))
    }
}

/// Settings row: "This device holds about 42% of your vault", "…" while the
/// estimate is loading.
pub fn percent_text(estimate: Option<&VaultEstimate>) -> String {
    match estimate {
        Some(e) => format!("This device holds about {}% of your vault", e.percent()),
        None => "This device holds about … of your vault".into(),
    }
}

/// "12 KB", "3.4 MB", "1.2 GB" (decimal units, like a download dialog).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit + 1 < UNITS.len() {
        v /= 1000.0;
        unit += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// Rough duration: "under a minute", "about 4 minutes", "about 2 hours".
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => "under a minute".into(),
        60..=5399 => {
            let m = (seconds + 30) / 60;
            if m == 1 {
                "about a minute".into()
            } else {
                format!("about {m} minutes")
            }
        }
        _ => {
            let h = (seconds + 1800) / 3600;
            format!("about {h} hours")
        }
    }
}

/// The "Fully sync this device" sheet's lines.
pub fn estimate_lines(e: &VaultEstimate) -> Vec<String> {
    let missing_objects = e.total_objects.saturating_sub(e.held_objects);
    vec![
        format!(
            "Downloads about {} objects ({} changes, {})",
            count(missing_objects),
            count(e.missing_events),
            bytes(e.missing_bytes)
        ),
        format!("Takes {}", duration(e.seconds)),
        format!("Storage used now: {}", bytes(e.held_bytes)),
        "Works offline afterwards".into(),
        "You can switch back to Lite later in Settings".into(),
    ]
}

/// Graph shadows: per type key, objects the directory counts minus those
/// loaded, when positive; sorted by key.
pub fn shadow_counts(
    counts: &HashMap<String, u64>,
    loaded: &HashMap<String, u64>,
) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = counts
        .iter()
        .filter_map(|(key, n)| {
            let missing = n.saturating_sub(loaded.get(key).copied().unwrap_or(0));
            (missing > 0).then(|| (key.clone(), missing))
        })
        .collect();
    out.sort();
    out
}

/// "12 not loaded".
pub fn shadow_label(n: u64) -> String {
    format!("{} not loaded", count(n))
}

// ---------------------------------------------------------------- actions

/// Spaces whose download this session already asked for, so a re-render
/// never asks twice.
#[derive(Default)]
struct Requested(HashSet<String>);
impl Global for Requested {}

/// A space's data is all here: not a lite replica, or the space is whole.
pub fn space_held(space_id: &str, cx: &App) -> bool {
    lite::status(cx).is_none_or(|s| s.is_full(space_id))
}

/// Download `space_id` whole (once per session); the store refreshes when it lands.
pub fn load_space(space_id: &str, cx: &mut App) {
    let Some(control) = lite::lite(cx) else {
        return;
    };
    if !cx
        .default_global::<Requested>()
        .0
        .insert(space_id.to_string())
    {
        return;
    }
    let space = space_id.to_string();
    let task = io::spawn(cx, async move { control.load_space(&space).await });
    let space = space_id.to_string();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if let Err(error) = result {
                log::warn!("loading space {space} failed: {error:#}");
                // Let the button try again.
                cx.default_global::<Requested>().0.remove(&space);
            }
            refresh(cx);
        });
    })
    .detach();
}

/// An opened query/set over `space_id`: download the space when lite and not whole.
pub fn ensure_space(space_id: &str, cx: &mut App) {
    if let Some(status) = lite::status(cx)
        && !status.is_full(space_id)
        && status.loading(space_id).is_none()
    {
        load_space(space_id, cx);
    }
}

/// Make sure `ids` are held (lite only); the store refreshes after.
pub fn fetch(ids: Vec<String>, cx: &mut App) {
    if ids.is_empty() || lite::status(cx).is_none() {
        return;
    }
    let Some(control) = lite::lite(cx) else {
        return;
    };
    let task = io::spawn(cx, async move { control.fetch(ids).await });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if let Err(error) = result {
                log::warn!("fetching objects failed: {error:#}");
            }
            refresh(cx);
        });
    })
    .detach();
}

/// Settings → "Fully sync this device"; progress shows in the sync status line.
pub fn sync_fully(cx: &mut App) {
    let Some(control) = lite::lite(cx) else {
        return;
    };
    let task = io::spawn(cx, async move { control.sync_fully().await });
    cx.spawn(async move |cx| {
        if let Err(error) = task.await {
            log::warn!("full sync failed to start: {error:#}");
        }
        cx.update(refresh);
    })
    .detach();
}

/// Settings → "Switch to Lite": this device drops its copy and opens again
/// lite (the browser reloads the page; the app reconnects in place);
/// `on_error` gets the reason when the replica refuses (changes not sent yet,
/// a save in progress).
pub fn switch_to_lite(cx: &mut App, on_error: impl FnOnce(String, &mut App) + 'static) {
    let Some(control) = lite::lite(cx) else {
        return;
    };
    let task = io::spawn(cx, async move { control.switch_to_lite().await });
    cx.spawn(async move |cx| match task.await {
        Ok(()) => {
            #[cfg(target_family = "wasm")]
            if let Some(window) = web_sys::window() {
                let _ = window.location().reload();
            }
            #[cfg(not(target_family = "wasm"))]
            cx.update(|cx| {
                use gpui_kit::component::WindowExt as _;
                if let Some(window) = cx.active_window() {
                    let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
                }
                if let Some(store) = Store::try_global(cx) {
                    store.update(cx, |store, cx| store.reconnect(cx));
                }
            });
        }
        Err(error) => cx.update(|cx| on_error(format!("{error:#}"), cx)),
    })
    .detach();
}

fn refresh(cx: &mut App) {
    if let Some(store) = Store::try_global(cx) {
        store.update(cx, |store, cx| store.refresh_all(cx));
    }
}

// --------------------------------------------------------------- elements

/// The notice a view over `space_id` shows on a lite replica when it may
/// see only part of the space: not ready → load once; loading → progress;
/// not whole → "Load this space fully". `None` when the space is whole (or
/// not lite). `id` keeps the button unique per view.
pub fn space_notice(id: impl Into<SharedString>, space_id: &str, cx: &App) -> Option<AnyElement> {
    let status = lite::status(cx)?;
    space_notice_for(id.into(), space_id, &status)
}

fn space_notice_for(id: SharedString, space_id: &str, status: &LiteStatus) -> Option<AnyElement> {
    if let Some(load) = status.loading(space_id) {
        return Some(notice_row(loading_text(load), None));
    }
    if status.is_full(space_id) {
        return None;
    }
    let text = status.is_unready(space_id).then_some(SPACE_UNREADY);
    let space = space_id.to_string();
    let button = ui::outline_button(
        ElementId::Name(format!("lite-load-{id}").into()),
        LOAD_SPACE,
    )
    .on_click(move |_, _, cx| load_space(&space, cx));
    Some(notice_row(
        text.unwrap_or("Showing what this device holds").into(),
        Some(button.into_any_element()),
    ))
}

/// The whole-vault notice: `checked && !ready`.
pub fn vault_notice(cx: &App) -> Option<AnyElement> {
    let status = lite::status(cx)?;
    (status.checked && !status.ready).then(|| {
        let button = ui::outline_button("lite-sync-fully", "Fully sync this device")
            .on_click(|_, _, cx| sync_fully(cx));
        notice_row(VAULT_UNREADY.into(), Some(button.into_any_element()))
    })
}

/// The sidebar/home notice: the vault not ready ([`vault_notice`]), else
/// `space_id` not ready ("load it fully once"), else its download's
/// progress. `None` otherwise: lite is quiet where it has what it needs.
pub fn ready_notice(space_id: &str, cx: &App) -> Option<AnyElement> {
    if let Some(notice) = vault_notice(cx) {
        return Some(notice);
    }
    let status = lite::status(cx)?;
    if status.loading(space_id).is_some() || status.is_unready(space_id) {
        return space_notice_for("ready".into(), space_id, &status);
    }
    None
}

/// A progress line alone ("Loading this space · N of M") for compact spots
/// such as pinned widgets; `None` when nothing loads.
pub fn loading_line(space_id: &str, cx: &App) -> Option<AnyElement> {
    let status = lite::status(cx)?;
    let load = status.loading(space_id)?;
    Some(notice_row(loading_text(load), None))
}

fn notice_row(text: String, button: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::NORMAL)
        .text_color(theme::muted())
        .child(text)
        .children(button)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_words() {
        let l = SpaceLoad {
            space_id: "s".into(),
            received: 1234,
            expected: 5000,
        };
        assert_eq!(loading_text(&l), "Loading this space · 1,234 of 5,000");
        let l = SpaceLoad {
            space_id: "s".into(),
            received: 7,
            expected: 0,
        };
        assert_eq!(loading_text(&l), "Loading this space · 7 received");
    }

    #[test]
    fn percent_words() {
        assert_eq!(
            percent_text(None),
            "This device holds about … of your vault"
        );
        let full = VaultEstimate {
            full: true,
            ..Default::default()
        };
        assert_eq!(
            percent_text(Some(&full)),
            "This device holds about 100% of your vault"
        );
        let part = VaultEstimate {
            held_events: 1,
            missing_events: 3,
            ..Default::default()
        };
        assert_eq!(
            percent_text(Some(&part)),
            "This device holds about 25% of your vault"
        );
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(3_400_000), "3.4 MB");
        assert_eq!(bytes(25_000_000_000), "25 GB");
        assert_eq!(duration(10), "under a minute");
        assert_eq!(duration(70), "about a minute");
        assert_eq!(duration(600), "about 10 minutes");
        assert_eq!(duration(7200), "about 2 hours");
    }

    #[test]
    fn estimate_sheet() {
        let e = VaultEstimate {
            held_objects: 10,
            total_objects: 1010,
            missing_events: 5000,
            missing_bytes: 2_000_000,
            held_bytes: 40_000,
            seconds: 120,
            ..Default::default()
        };
        let lines = estimate_lines(&e);
        assert_eq!(
            lines[0],
            "Downloads about 1,000 objects (5,000 changes, 2.0 MB)"
        );
        assert_eq!(lines[1], "Takes about 2 minutes");
        assert_eq!(lines[2], "Storage used now: 40 KB");
    }

    #[test]
    fn shadows() {
        let counts = HashMap::from([
            ("page".to_string(), 10),
            ("task".to_string(), 2),
            ("note".to_string(), 1),
        ]);
        let loaded = HashMap::from([("page".to_string(), 4), ("task".to_string(), 3)]);
        assert_eq!(
            shadow_counts(&counts, &loaded),
            vec![("note".to_string(), 1), ("page".to_string(), 6)]
        );
        assert_eq!(shadow_label(1200), "1,200 not loaded");
    }
}
