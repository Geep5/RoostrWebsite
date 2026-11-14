//! Emoji reactions on chat messages (iMessage / Discord style). A reaction is a chat line
//! (`chat_post` with `reaction`, meta `kind` "reaction") that [`crate::chat`] folds onto the
//! message it answers; the harness wakes an agent for each. Holding a message (touch), a
//! right click or the row's 🙂 button opens the reaction bar: the space's Reaction objects'
//! emoji first, then the ones you used last, then [`DEFAULTS`].

use std::hash::{Hash, Hasher};
use std::time::Duration;

use roostr_core::State;
use roostr_core::types::field_str;
use roostr_rt::Task;
use serde_json::json;

use crate::chat::{ChatMessage, DISCUSSION, Reaction};
use crate::discussion::{Disc, with};

/// The bar's quick emoji when the space and your recent ones leave room.
pub(crate) const DEFAULTS: [&str; 6] = ["👍", "❤️", "😂", "✅", "👀", "🔥"];
/// How many quick emoji the bar shows.
const QUICK: usize = 6;
/// Your recently used reactions in the UI state, newest first.
const RECENT_KEY: &str = "chat.recentReactions";
const RECENT_MAX: usize = 12;
/// A finger held still this long on a message opens its bar (the platform's long press).
pub(crate) const HOLD: Duration = Duration::from_millis(500);
/// How far (points) the list may move under a held finger before the hold is a pan.
const HOLD_SLOP: f32 = 8.;

/// Where the reaction bar opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BarAt {
    /// A held message: above its row's start.
    Row,
    /// The row's 🙂 button: above its row's end, by the button.
    Button,
    /// A right click, at this window point.
    Point(f32, f32),
}

/// The reaction bar open on a message; `picker` shows the full emoji picker instead.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReactBar {
    pub message_id: String,
    pub at: BarAt,
    pub picker: bool,
}

/// A finger down on a message: where the message list was drawn then, and the timer that
/// opens the bar.
pub(crate) struct Hold {
    message_id: String,
    list_y: f32,
    _timer: Task<()>,
}

/// One of your reactions sent and not yet in the object.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingReaction {
    message_id: String,
    emoji: String,
    removed: bool,
}

/// The emoji of the active space's Reaction objects, as last loaded.
#[derive(Default)]
struct SpaceReactions {
    space: String,
    /// The space's Reaction summaries (ids, edit times) the emoji were loaded for.
    sig: u64,
    emojis: Vec<String>,
    loading: Option<Task<()>>,
}

/// A touch screen: holding a message opens its bar (a mouse right-clicks it).
#[cfg(not(test))]
pub(crate) fn touch_screen(_: &State) -> bool {
    roostr_core::layout::is_touch()
}

/// Tests stand in for a touch screen with [`TouchScreen`] in the state.
#[cfg(test)]
pub(crate) fn touch_screen(s: &State) -> bool {
    s.get::<TouchScreen>().is_some()
}

#[cfg(test)]
pub(crate) struct TouchScreen;

fn space_sig(s: &State, space: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for o in s
        .store
        .summaries
        .iter()
        .filter(|o| o.type_key == "reaction" && o.channel_id == space)
    {
        (&o.id, o.updated_at).hash(&mut h);
    }
    h.finish()
}

/// Loads the active space's Reaction emoji when they changed (or the space did).
pub(crate) fn load_space_reactions(s: &mut State) {
    let space = s.nav.space().to_string();
    let sig = space_sig(s, &space);
    let known = s.get::<SpaceReactions>();
    if known.is_some_and(|r| r.space == space && r.sig == sig) {
        return;
    }
    let rows = roostr_core::api::query_all(
        json!({ "type": "reaction", "filters": [{ "key": "channel", "condition": "equal", "value": space }] }),
        s,
    );
    let loaded = space.clone();
    let task = s.rt.spawn(async move |cx| {
        let Ok(rows) = rows.await else {
            return;
        };
        cx.update(|s| {
            let r = s.slot::<SpaceReactions>();
            if r.space != loaded {
                return;
            }
            r.emojis.clear();
            for row in &rows {
                let emoji = field_str(&row.fields, "emoji").trim();
                if !emoji.is_empty() && !r.emojis.iter().any(|e| e == emoji) {
                    r.emojis.push(emoji.to_string());
                }
            }
        });
    });
    let r = s.slot::<SpaceReactions>();
    if r.space != space {
        r.emojis.clear();
    }
    r.space = space;
    r.sig = sig;
    r.loading = Some(task);
}

/// The bar's quick emoji: the space's Reaction objects' first, then your recent ones, then
/// [`DEFAULTS`].
pub(crate) fn quick_emojis(s: &State) -> Vec<String> {
    let space = s.nav.space();
    let configured = s
        .get::<SpaceReactions>()
        .filter(|r| r.space == space)
        .map(|r| r.emojis.clone())
        .unwrap_or_default();
    let recent: Vec<String> = s.ui.get(RECENT_KEY).unwrap_or_default();
    let mut out: Vec<String> = Vec::with_capacity(QUICK);
    let defaults = DEFAULTS.iter().map(|e| e.to_string());
    for emoji in configured.into_iter().chain(recent).chain(defaults) {
        if out.len() == QUICK {
            break;
        }
        if !out.contains(&emoji) {
            out.push(emoji);
        }
    }
    out
}

/// `emoji` leads your recent reactions.
fn remember(s: &mut State, emoji: &str) {
    let mut recent: Vec<String> = s.ui.get(RECENT_KEY).unwrap_or_default();
    recent.retain(|e| e != emoji);
    recent.insert(0, emoji.to_string());
    recent.truncate(RECENT_MAX);
    s.ui.set(RECENT_KEY, &recent);
}

impl Disc {
    /// Reactions go on the human thread's lines (not exchanges, not read-only history), once
    /// you are known.
    pub(crate) fn can_react(&self) -> bool {
        !self.read_only && !self.is_exchange && !self.me.is_empty()
    }

    /// You reacted to message `message_id` with `emoji` (as shown, pending ones included).
    pub(crate) fn reacted(&self, message_id: &str, emoji: &str) -> bool {
        self.message(message_id).is_some_and(|m| {
            m.reactions
                .iter()
                .any(|r| r.emoji == emoji && r.authors.contains(&self.me))
        })
    }

    pub(crate) fn open_react_bar(&mut self, message_id: &str, at: BarAt, s: &mut State) {
        self.hold = None;
        // A line still sending has no id to react to yet.
        if !self.can_react() || self.outgoing.as_ref().is_some_and(|o| o.id == message_id) {
            return;
        }
        self.react_bar = Some(ReactBar {
            message_id: message_id.to_string(),
            at,
            picker: false,
        });
        load_space_reactions(s);
    }

    pub(crate) fn close_react_bar(&mut self) {
        self.react_bar = None;
    }

    /// Puts your `emoji` on message `message_id`, or takes it back when it is there: shown at
    /// once, posted as a reaction line.
    pub(crate) fn toggle_reaction(&mut self, message_id: &str, emoji: &str, s: &mut State) {
        if !self.can_react() || emoji.is_empty() || self.message(message_id).is_none() {
            return;
        }
        let removed = self.reacted(message_id, emoji);
        if !removed {
            remember(s, emoji);
        }
        let pending = PendingReaction {
            message_id: message_id.to_string(),
            emoji: emoji.to_string(),
            removed,
        };
        self.pending_reactions
            .retain(|p| !(p.message_id == message_id && p.emoji == emoji));
        self.pending_reactions.push(pending.clone());
        let thread = if self.thread_id == DISCUSSION {
            ""
        } else {
            self.thread_id.as_str()
        };
        let task =
            roostr_core::api::chat::react(&self.object_id, thread, message_id, emoji, removed, s);
        self.derive(s);
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let result = task.await;
            cx.update(|s| {
                with(s, &key, |d, s| {
                    if let Err(error) = result {
                        // Not sent: the chip goes back to what the object says.
                        d.pending_reactions.retain(|p| *p != pending);
                        d.send_error = error.to_string();
                        d.derive(s);
                    }
                })
            });
        })
        .detach();
    }

    /// Shows your pending reactions on `messages`; each goes once the object carries it.
    pub(crate) fn show_pending_reactions(&mut self, messages: &mut [ChatMessage]) {
        let me = self.me.clone();
        self.pending_reactions.retain(|p| {
            let Some(m) = messages.iter_mut().find(|m| m.id == p.message_id) else {
                return false;
            };
            let at = m.reactions.iter().position(|r| r.emoji == p.emoji);
            let carried = at.is_some_and(|i| m.reactions[i].authors.contains(&me));
            if carried != p.removed {
                return false;
            }
            match (at, p.removed) {
                (Some(i), true) => {
                    m.reactions[i].authors.retain(|a| *a != me);
                    if m.reactions[i].authors.is_empty() {
                        m.reactions.remove(i);
                    }
                }
                (Some(i), false) => m.reactions[i].authors.push(me.clone()),
                (None, false) => m.reactions.push(Reaction {
                    emoji: p.emoji.clone(),
                    authors: vec![me.clone()],
                }),
                (None, true) => {}
            }
            true
        });
    }

    /// A finger went down on message `message_id`: held still for [`HOLD`] it opens the bar.
    pub(crate) fn hold_start(&mut self, message_id: &str, s: &mut State) {
        if !self.can_react() || self.react_bar.is_some() {
            return;
        }
        let timer = s.rt.timer(HOLD);
        let (key, id) = (self.key.clone(), message_id.to_string());
        let task = s.rt.spawn(async move |cx| {
            timer.await;
            cx.update(|s| {
                with(s, &key, |d, s| d.held(&id, s));
            });
        });
        self.hold = Some(Hold {
            message_id: message_id.to_string(),
            list_y: self.list_rect.get()[1],
            _timer: task,
        });
    }

    /// The finger left message `message_id` (lifted, or the press ended) before the hold.
    pub(crate) fn hold_end(&mut self, message_id: &str) {
        if self
            .hold
            .as_ref()
            .is_some_and(|h| h.message_id == message_id)
        {
            self.hold = None;
        }
    }

    /// The hold's time passed: the bar opens unless the list moved under the finger (a pan).
    fn held(&mut self, message_id: &str, s: &mut State) {
        let Some(hold) = self.hold.take() else {
            return;
        };
        let still = (self.list_rect.get()[1] - hold.list_y).abs() <= HOLD_SLOP;
        if hold.message_id == message_id && still {
            self.open_react_bar(message_id, BarAt::Row, s);
        }
    }
}
