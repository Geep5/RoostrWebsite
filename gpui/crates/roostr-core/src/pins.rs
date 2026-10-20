//! Pins (`src/lib/pins.ts`, flows from `routes/app/+layout.svelte`). Each
//! pinned object carries its own sidebar position (`pin_order`, a sortable
//! text key), so pinning, unpinning or moving writes only that object. A
//! space's pin list (`Space::pinned_ids`) is built by whoever lists spaces.
//! Whether its widget is folded is on the object too (`pin_collapsed`, a
//! bool; absent = open), so it follows the vault to every device and member.

use std::collections::HashMap;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Task};

use crate::api;
use crate::store::Store;
use crate::types::{Fields, Object, QueryRow, Space, Value, field_str};

pub const PIN_KEY: &str = "pin_order";
/// A pinned object's widget is folded in the sidebar and on the space home.
pub const COLLAPSED_KEY: &str = "pin_collapsed";

/// Whether `fields` mark a pinned widget folded (`None` = not set).
pub fn collapsed(fields: &Fields) -> Option<bool> {
    fields.get(COLLAPSED_KEY).and_then(|v| v.bool_value)
}

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn digit_index(c: u8) -> i64 {
    DIGITS.iter().position(|&d| d == c).map_or(-1, |i| i as i64)
}

/// A key strictly between `a` and `b` (`a` "" = before everything; `b`
/// `None` = after everything). Keys never end in "0", so there is always
/// room between two. Same algorithm as core/pins.odin `pin_key_between`.
pub fn key_between(a: &str, b: Option<&str>) -> String {
    let (a, b) = (a.as_bytes(), b.map(str::as_bytes));
    String::from_utf8(between(a, b)).unwrap_or_default()
}

fn between(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && *a.get(n).unwrap_or(&b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(between(a.get(n..).unwrap_or(&[]), Some(&b[n..])));
            return out;
        }
    }
    let lo = a.first().map_or(0, |&c| digit_index(c));
    let hi = match b {
        Some(b) if !b.is_empty() => digit_index(b[0]),
        _ => DIGITS.len() as i64,
    };
    if hi - lo > 1 {
        return vec![DIGITS[((lo + hi + 1) / 2) as usize]];
    }
    if let Some(b) = b
        && b.len() > 1
    {
        return b[..1].to_vec();
    }
    let mut out = vec![DIGITS[lo.max(0) as usize]];
    out.extend(between(a.get(1..).unwrap_or(&[]), None));
    out
}

/// What [`pins_by_space`] reads of an object state.
pub struct Pinnable<'a> {
    pub id: &'a str,
    pub type_key: &'a str,
    pub deleted: bool,
    pub created_at: i64,
    pub fields: &'a Fields,
}

impl<'a> From<&'a Object> for Pinnable<'a> {
    fn from(o: &'a Object) -> Self {
        Self {
            id: &o.id,
            type_key: &o.type_key,
            deleted: o.deleted,
            created_at: o.created_at,
            fields: &o.fields,
        }
    }
}

impl<'a> From<&'a QueryRow> for Pinnable<'a> {
    fn from(r: &'a QueryRow) -> Self {
        Self {
            id: &r.id,
            type_key: &r.type_key,
            deleted: r.deleted == Some(true),
            created_at: r.created_at,
            fields: &r.fields,
        }
    }
}

/// Pinned object ids per space, in sidebar order. An object with no space
/// belongs to the oldest space.
pub fn pins_by_space<'a>(
    states: impl IntoIterator<Item = Pinnable<'a>>,
) -> HashMap<String, Vec<String>> {
    let all: Vec<Pinnable> = states.into_iter().filter(|s| !s.deleted).collect();
    let fallback = all
        .iter()
        .filter(|s| s.type_key == "channel")
        .min_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(b.id)))
        .map_or("", |s| s.id);
    let mut by_space: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for s in &all {
        let key = field_str(s.fields, PIN_KEY);
        if key.is_empty() {
            continue;
        }
        let space = field_str(s.fields, "channel");
        let space = if space.is_empty() { fallback } else { space };
        by_space
            .entry(space.to_string())
            .or_default()
            .push((s.id, key));
    }
    by_space
        .into_iter()
        .map(|(space, mut list)| {
            list.sort_by(|x, y| x.1.cmp(y.1).then(x.0.cmp(y.0)));
            (
                space,
                list.into_iter().map(|(id, _)| id.to_string()).collect(),
            )
        })
        .collect()
}

/// The space that owns objects stamped `channel_id` (the oldest space when unstamped or unknown).
pub fn owning_space(store: &Store, channel_id: &str) -> Option<Space> {
    store
        .space(channel_id)
        .or_else(|| store.channels.first())
        .cloned()
}

/// The space named by `channel_id` exactly (`channels.find`), no fallback:
/// the More menu's Pin/Unpin (`togglePin`) does nothing for an unstamped object.
pub fn space_of(store: &Store, channel_id: &str) -> Option<Space> {
    store.space(channel_id).cloned()
}

/// A pinned object's position ("" for none or no object).
fn pin_key(id: Option<String>, cx: &App) -> Task<String> {
    let Some(id) = id else {
        return Task::ready(String::new());
    };
    let fetch = Store::global(cx).read(cx).fetch_object(&id, cx);
    cx.background_spawn(async move {
        fetch
            .await
            .map(|o| o.field_str(PIN_KEY).to_string())
            .unwrap_or_default()
    })
}

fn refresh(cx: &mut App) {
    Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
}

/// Pin `object_id` after `space`'s last pin, or unpin it (`togglePin`).
pub fn set_pinned(object_id: &str, space: &Space, pinned: bool, cx: &mut App) -> Task<Result<()>> {
    let object_id = object_id.to_string();
    if !pinned {
        let task = api::note::delete_field(&object_id, PIN_KEY, cx);
        return cx.spawn(async move |cx| {
            task.await?;
            cx.update(refresh);
            Ok(())
        });
    }
    let last = pin_key(space.pinned_ids.last().cloned(), cx);
    cx.spawn(async move |cx| {
        let key = key_between(&last.await, None);
        cx.update(|cx| api::note::set_field(&object_id, PIN_KEY, Value::string(key), cx))
            .await?;
        cx.update(refresh);
        Ok(())
    })
}

/// Move pin `dragged` next to pin `target` in `space` (`below` = after it):
/// only the dragged object gets a new key between its new neighbours.
pub fn move_pin(
    space: &Space,
    dragged: &str,
    target: &str,
    below: bool,
    cx: &mut App,
) -> Task<Result<()>> {
    if dragged.is_empty() || target.is_empty() || dragged == target {
        return Task::ready(Ok(()));
    }
    let ids: Vec<String> = space
        .pinned_ids
        .iter()
        .filter(|x| *x != dragged)
        .cloned()
        .collect();
    let at = ids
        .iter()
        .position(|x| x == target)
        .map_or(0, |i| i as i64 + 1)
        - 1
        + i64::from(below);
    let before_id = if at >= 1 {
        ids.get(at as usize - 1).cloned()
    } else {
        None
    };
    let after_id = if at >= 0 {
        ids.get(at as usize).cloned()
    } else {
        None
    };
    let has_after = after_id.is_some();
    let before = pin_key(before_id, cx);
    let after = pin_key(after_id, cx);
    let dragged = dragged.to_string();
    cx.spawn(async move |cx| {
        let (before, after) = (before.await, after.await);
        let key = key_between(&before, has_after.then_some(after.as_str()));
        cx.update(|cx| api::note::set_field(&dragged, PIN_KEY, Value::string(key), cx))
            .await?;
        cx.update(refresh);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sort_between_neighbours() {
        let first = key_between("", None);
        let after = key_between(&first, None);
        assert!(first < after);
        let mid = key_between(&first, Some(&after));
        assert!(first < mid && mid < after, "{first} < {mid} < {after}");
        let before = key_between("", Some(&first));
        assert!(!before.is_empty() && before < first);
        // Adjacent digits still leave room.
        let tight = key_between("A", Some("B"));
        assert!("A" < tight.as_str() && tight.as_str() < "B" && !tight.ends_with('0'));
        let mut lo = String::new();
        let hi = "1".to_string();
        for _ in 0..20 {
            let k = key_between(&lo, Some(&hi));
            assert!(lo < k && k < hi, "{lo} < {k} < {hi}");
            lo = k;
        }
    }
}
