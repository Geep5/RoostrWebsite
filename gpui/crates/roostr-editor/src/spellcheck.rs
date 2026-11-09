//! Spellcheck glue (`src/lib/spelldom.ts` + the storage half of `spell.ts`):
//! the dictionary loads once on a background thread; the "Add to
//! dictionary" list lives in device UI state under the web's `spell-ignore`
//! key (Settings lists and edits the same key).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use roostr_core::{State, UiState};

use crate::spell::{Dictionary, Misspelling};

const IGNORE_KEY: &str = "spell-ignore";

/// `None` until loaded; `Some(None)` when it could not load.
static DICTIONARY: Mutex<Option<Option<Arc<Dictionary>>>> = Mutex::new(None);
static LOADING: AtomicBool = AtomicBool::new(false);

/// Start loading the word lists (once per process).
pub fn ensure_loaded(s: &State) {
    if DICTIONARY.lock().is_some() || LOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    s.rt.background(async move {
        let dict = match Dictionary::load() {
            Ok(d) => Some(Arc::new(d)),
            Err(error) => {
                log::warn!("spellcheck dictionary unavailable: {error:#}");
                None
            }
        };
        *DICTIONARY.lock() = Some(dict);
    })
    .detach();
}

/// The dictionary, once loaded (nothing is flagged before).
pub fn dictionary() -> Option<Arc<Dictionary>> {
    DICTIONARY.lock().clone().flatten()
}

/// The user's ignore list, re-read every time so edits in Settings apply.
pub fn ignored(ui: &UiState) -> HashSet<String> {
    ui.get::<Vec<String>>(IGNORE_KEY)
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.to_lowercase())
        .collect()
}

/// "Add to dictionary".
pub fn add_to_dictionary(word: &str, ui: &mut UiState) {
    let mut words: Vec<String> = ui.get(IGNORE_KEY).unwrap_or_default();
    let w = word.to_lowercase();
    if !words.contains(&w) {
        words.push(w);
    }
    ui.set(IGNORE_KEY, &words);
}

/// Misspellings per text (blocking; run in the background).
pub fn check_all(
    dict: &Dictionary,
    texts: &[(String, String)],
    ignored: &HashSet<String>,
) -> Vec<(String, Vec<Misspelling>)> {
    texts
        .iter()
        .map(|(id, text)| (id.clone(), dict.check_text(text, ignored)))
        .collect()
}
