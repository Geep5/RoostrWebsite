//! Folded pinned widgets (sidebar and space-home cards): the state is the
//! pinned object's own `pin_collapsed` field, so it syncs with the vault. A
//! fold shows at once and holds until the mutation lands and the object is
//! refetched; a change from elsewhere shows when the object's commit arrives.
//! The per-device `widget-collapsed` map of earlier builds is written onto
//! the objects once, then dropped.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::Context;
use roostr_core::pins::{self, COLLAPSED_KEY};
use roostr_core::store::{ObjectSlot, Store};
use roostr_core::types::{Object, Value};
use roostr_core::{UiState, api};

use crate::workspace::Workspace;

/// The device-local map earlier builds kept (object id -> folded).
pub(crate) const LEGACY_KEY: &str = "widget-collapsed";

/// A fold this device asked for, shown until the object reflects it.
pub(crate) struct PendingFold {
    collapsed: bool,
    /// The cached object when the mutation landed; a newer one settles the fold.
    landed: Option<Option<Arc<Object>>>,
}

/// The legacy map to migrate (an empty one is dropped right away).
pub(crate) fn legacy_folds(cx: &mut Context<Workspace>) -> HashMap<String, bool> {
    let legacy: HashMap<String, bool> = UiState::get(cx, LEGACY_KEY).unwrap_or_default();
    if legacy.is_empty() {
        UiState::remove(cx, LEGACY_KEY);
    }
    legacy
}

impl Workspace {
    /// Whether pin `id`'s widget is folded: this device's unsettled fold,
    /// else the object's field (absent = open; not loaded yet = folded, so
    /// a folded widget never flashes open).
    pub fn pin_collapsed(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let object = Store::global(cx).update(cx, |store, cx| {
            store.ensure_object(id, cx);
            store.object(id)
        });
        let stored = object.as_deref().map(|o| pins::collapsed(&o.fields));
        if let Some(pending) = self.pin_folds.get(id) {
            let settled = match &pending.landed {
                None => false,
                Some(at) => {
                    stored.flatten().unwrap_or(false) == pending.collapsed
                        || !same_object(at, &object)
                }
            };
            if !settled {
                return pending.collapsed;
            }
            self.pin_folds.remove(id);
        }
        stored.is_none_or(|s| s.unwrap_or(false))
    }

    /// Fold or unfold pin `id`: one `set_field` on the object.
    pub fn toggle_pin_collapsed(&mut self, id: &str, cx: &mut Context<Self>) {
        let collapsed = !self.pin_collapsed(id, cx);
        self.pin_folds.insert(
            id.to_string(),
            PendingFold {
                collapsed,
                landed: None,
            },
        );
        let task = api::note::set_field(id, COLLAPSED_KEY, Value::bool(collapsed), cx);
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let at = Store::global(cx).read(cx).object(&id);
                let Some(pending) = this.pin_folds.get_mut(&id) else {
                    return;
                };
                if pending.collapsed != collapsed {
                    return; // A later toggle owns it.
                }
                match result {
                    Ok(_) => pending.landed = Some(at),
                    Err(error) => {
                        log::warn!("folding pin {id}: {error:#}");
                        this.pin_folds.remove(&id);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Write the legacy device-local folds onto objects that have no
    /// `pin_collapsed` yet, once each; the local key goes when all are done.
    /// Ids the vault no longer lists are dropped once history has synced.
    pub(crate) fn migrate_pin_folds(&mut self, cx: &mut Context<Self>) {
        if self.legacy_folds.is_empty() {
            return;
        }
        let store = Store::global(cx);
        let mut load = Vec::new();
        let mut writes = Vec::new();
        {
            let store = store.read(cx);
            let bootstrapped = store.status.bootstrapped;
            self.legacy_folds.retain(|id, collapsed| {
                if store.summary(id).is_none() {
                    return !bootstrapped;
                }
                match store.object_slot(id) {
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
        }
        store.update(cx, |store, cx| {
            for id in &load {
                store.ensure_object(id, cx);
            }
        });
        for id in writes {
            let task = api::note::set_field(&id, COLLAPSED_KEY, Value::bool(true), cx);
            cx.spawn(async move |_, _| {
                if let Err(error) = task.await {
                    log::warn!("migrating the fold of pin {id}: {error:#}");
                }
            })
            .detach();
        }
        if self.legacy_folds.is_empty() {
            UiState::remove(cx, LEGACY_KEY);
        }
    }
}

fn same_object(a: &Option<Arc<Object>>, b: &Option<Arc<Object>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
