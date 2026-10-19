//! Spellcheck glue (`src/lib/spelldom.ts` + the storage half of `spell.ts`):
//! the dictionary loads once on a background thread; the "Add to
//! dictionary" list lives in device UI state under the web's `spell-ignore`
//! key (Settings lists and edits the same key).

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use gpui_kit::{App, AppContext as _};
use roostr_core::UiState;

use crate::spell::{Dictionary, Misspelling};

const IGNORE_KEY: &str = "spell-ignore";

static DICTIONARY: OnceLock<Option<Arc<Dictionary>>> = OnceLock::new();
static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start loading the word lists (once per process).
pub fn ensure_loaded(cx: &mut App) {
    if DICTIONARY.get().is_some() || LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    cx.background_spawn(async move {
        let dict = match Dictionary::load() {
            Ok(d) => Some(Arc::new(d)),
            Err(error) => {
                log::warn!("spellcheck dictionary unavailable: {error:#}");
                None
            }
        };
        let _ = DICTIONARY.set(dict);
    })
    .detach();
}

/// The dictionary, once loaded (nothing is flagged before).
pub fn dictionary() -> Option<Arc<Dictionary>> {
    DICTIONARY.get().and_then(Clone::clone)
}

/// The user's ignore list, re-read every time so edits in Settings apply.
pub fn ignored(cx: &App) -> HashSet<String> {
    UiState::get::<Vec<String>>(cx, IGNORE_KEY)
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.to_lowercase())
        .collect()
}

/// "Add to dictionary".
pub fn add_to_dictionary(word: &str, cx: &mut App) {
    let mut words: Vec<String> = UiState::get(cx, IGNORE_KEY).unwrap_or_default();
    let w = word.to_lowercase();
    if !words.contains(&w) {
        words.push(w);
    }
    UiState::set(cx, IGNORE_KEY, &words);
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
