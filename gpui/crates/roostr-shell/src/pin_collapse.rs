//! Folded pinned widgets (sidebar and space-home cards): the state is the
//! pinned object's own `pin_collapsed` field, so it syncs with the vault. A
//! fold shows at once and holds until the mutation lands and the object is
//! refetched; a change from elsewhere shows when the object's commit arrives.
//! The per-device `widget-collapsed` map of earlier builds is written onto
//! the objects once, then dropped.

use std::collections::HashMap;
use std::sync::Arc;

use roostr_core::pins::{self, COLLAPSED_KEY};
use roostr_core::store::ObjectSlot;
use roostr_core::types::{Object, Value};
use roostr_core::{State, api};

/// The device-local map earlier builds kept (object id -> folded).
pub(crate) const LEGACY_KEY: &str = "widget-collapsed";

/// A fold this device asked for, shown until the object reflects it.
struct PendingFold {
    collapsed: bool,
    /// The cached object when the mutation landed; a newer one settles the fold.
    landed: Option<Option<Arc<Object>>>,
}

/// This device's folds and the legacy map still to migrate.
#[derive(Default)]
pub(crate) struct PinFolds {
    pending: HashMap<String, PendingFold>,
    /// The legacy device-local folds still to migrate (read once).
    legacy: Option<HashMap<String, bool>>,
}

fn same_object(a: &Option<Arc<Object>>, b: &Option<Arc<Object>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The object's stored fold: `None` = not loaded, `Some(None)` = never folded.
fn stored(s: &State, id: &str) -> (Option<Arc<Object>>, Option<Option<bool>>) {
    let object = s.store.object(id);
    let stored = object.as_deref().map(|o| pins::collapsed(&o.fields));
    (object, stored)
}

/// A pending fold that the object now reflects (or that a newer object replaced).
fn settled(pending: &PendingFold, s: &State, id: &str) -> bool {
    let (object, stored) = stored(s, id);
    match &pending.landed {
        None => false,
        Some(at) => {
            stored.flatten().unwrap_or(false) == pending.collapsed || !same_object(at, &object)
        }
    }
}

/// Whether pin `id`'s widget is folded: this device's unsettled fold, else the
/// object's field (absent = open; not loaded yet = folded, so a folded widget
/// never flashes open).
pub(crate) fn collapsed(s: &State, id: &str) -> bool {
    if let Some(pending) = s.get::<PinFolds>().and_then(|f| f.pending.get(id))
        && !settled(pending, s, id)
    {
        return pending.collapsed;
    }
    let (_, stored) = stored(s, id);
    stored.is_none_or(|s| s.unwrap_or(false))
}

/// This device folded or unfolded pin `id` and that hasn't settled: its fold animates.
pub(crate) fn own(s: &State, id: &str) -> bool {
    s.get::<PinFolds>()
        .is_some_and(|f| f.pending.contains_key(id))
}

/// Fold or unfold pin `id`: one `set_field` on the object.
pub(crate) fn toggle(s: &mut State, id: &str) {
    let collapsed = !collapsed(s, id);
    s.slot::<PinFolds>().pending.insert(
        id.to_string(),
        PendingFold {
            collapsed,
            landed: None,
        },
    );
    let task = api::note::set_field(id, COLLAPSED_KEY, Value::bool(collapsed), s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let at = s.store.object(&id);
            let folds = s.slot::<PinFolds>();
            let Some(pending) = folds.pending.get_mut(&id) else {
                return;
            };
            if pending.collapsed != collapsed {
                return; // A later toggle owns it.
            }
            match result {
                Ok(_) => pending.landed = Some(at),
                Err(error) => {
                    log::warn!("folding pin {id}: {error:#}");
                    folds.pending.remove(&id);
                }
            }
        });
    })
    .detach();
}

/// Keep the folds current (an `on_poll` listener's work): ask for the pinned
/// objects of `pinned` the store was never asked for, drop settled folds, and
/// write the legacy device-local folds onto objects with no `pin_collapsed`
/// yet, once each (the local key goes when all are done; ids the vault no
/// longer lists are dropped once history has synced). True when anything the
/// view reads changed.
pub(crate) fn sync(s: &mut State, pinned: &[String]) -> bool {
    let mut changed = false;
    for id in pinned {
        if s.store.object_slot(id).is_none() {
            s.store.ensure_object(id);
        }
    }
    let done: Vec<String> = s
        .get::<PinFolds>()
        .map(|f| {
            f.pending
                .iter()
                .filter(|(id, p)| settled(p, s, id))
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    if !done.is_empty() {
        let folds = s.slot::<PinFolds>();
        for id in done {
            folds.pending.remove(&id);
        }
        changed = true;
    }
    migrate(s);
    changed
}

fn migrate(s: &mut State) {
    if s.slot::<PinFolds>().legacy.is_none() {
        let legacy: HashMap<String, bool> = s.ui.get(LEGACY_KEY).unwrap_or_default();
        if legacy.is_empty() {
            s.ui.remove(LEGACY_KEY);
        }
        s.slot::<PinFolds>().legacy = Some(legacy);
    }
    let Some(mut legacy) = s.slot::<PinFolds>().legacy.take() else {
        return;
    };
    if legacy.is_empty() {
        s.slot::<PinFolds>().legacy = Some(legacy);
        return;
    }
    let mut load = Vec::new();
    let mut writes = Vec::new();
    let bootstrapped = s.store.status.bootstrapped;
    legacy.retain(|id, collapsed| {
        if s.store.summary(id).is_none() {
            return !bootstrapped;
        }
        match s.store.object_slot(id) {
            None => {
                load.push(id.clone());
                true
            }
            Some(ObjectSlot::Loading) => true,
            Some(ObjectSlot::Failed(_)) => false,
            Some(ObjectSlot::Ready(object)) => {
                // Open is the default: only a fold needs writing.
                if *collapsed && pins::collapsed(&object.fields).is_none() {
                    writes.push(id.clone());
                }
                false
            }
        }
    });
    for id in &load {
        s.store.ensure_object(id);
    }
    for id in writes {
        let task = api::note::set_field(&id, COLLAPSED_KEY, Value::bool(true), s);
        s.rt.spawn(async move |_| {
            if let Err(error) = task.await {
                log::warn!("migrating the fold of pin {id}: {error:#}");
            }
        })
        .detach();
    }
    if legacy.is_empty() {
        s.ui.remove(LEGACY_KEY);
    }
    s.slot::<PinFolds>().legacy = Some(legacy);
}
