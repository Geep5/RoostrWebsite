//! Chat drafts that outlive the composer (`src/lib/drafts.ts`): switching the
//! pane to Properties (or to another object) drops the chat's state, and a
//! half-typed message must be there when you come back. Kept per object +
//! conversation thread for the life of the app (the web's sessionStorage
//! scope: one tab session).

use std::collections::HashMap;

use roostr_core::State;

/// The slot holding every draft, by `object|thread`.
#[derive(Default)]
struct Drafts(HashMap<String, String>);

fn key_of(object_id: &str, thread_id: &str) -> String {
    format!("{object_id}|{thread_id}")
}

pub(crate) fn load_draft(object_id: &str, thread_id: &str, s: &State) -> String {
    s.get::<Drafts>()
        .and_then(|d| d.0.get(&key_of(object_id, thread_id)).cloned())
        .unwrap_or_default()
}

pub(crate) fn save_draft(object_id: &str, thread_id: &str, text: &str, s: &mut State) {
    let drafts = s.slot::<Drafts>();
    let key = key_of(object_id, thread_id);
    if text.is_empty() {
        drafts.0.remove(&key);
    } else {
        drafts.0.insert(key, text.to_string());
    }
}
