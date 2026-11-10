//! An object's discussion (`src/lib/components/Discussion.svelte`): its state and
//! behaviour. Human and private chat stays local; exchanges send immutable mailbox
//! envelopes. Each shown discussion is an instance in the [`Discussions`] slot, keyed by
//! object and place; [`crate::discussion_view`] draws it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::{
    AgentEndpoint, AgentMessage, Conversation, Object, State, Store, StoreEvent, guest_agents,
};
use roostr_rt::Task;
use roostr_ui::Theme;
use roostr_ui::tags::{TagPaint, object_pill_paint, tint_paint};
use rune::{KeyEvent, TextEdit};
use serde::Deserialize;

use crate::chat::{
    ChatMessage, DISCUSSION, chat_messages, is_legacy_exchange, object_chat_messages,
    reply_recipients, unique_endpoints,
};
use crate::drafts::{load_draft, save_draft};
use crate::threads::{ObjectAgentOption, endpoint_name, load_object_agents, seed_tags};

/// The green that object links and `!object` tags are tinted with.
pub(crate) const OBJECT_GREEN: u32 = 0x2ea043;

/// An `@Agent` tag in that agent's own chat colour (the one its replies use), neutral grey
/// when it has none. `name` is matched against the agents of `space` first, then any space.
pub(crate) fn agent_tag_paint(t: &Theme, store: &Store, name: &str, space: &str) -> TagPaint {
    let color = store
        .agents
        .iter()
        .filter(|a| a.name == name)
        .max_by_key(|a| a.channel == space)
        .and_then(|a| roostr_ui::color::hex(&a.chat_color));
    match color {
        Some(c) => tint_paint(t, c),
        None => object_pill_paint(t, t.fg),
    }
}

/// How a discussion is shown.
#[derive(Clone, Debug)]
pub struct DiscussionOptions {
    /// Always open, filling its parent (the drawer, a chat object's page).
    pub full: bool,
    /// The full-screen chat page: messages fill the screen, composer pinned.
    pub page_mode: bool,
    /// Which conversation of the object; `"__discussion__"` is the human thread (the
    /// object's one merged chat).
    pub thread_id: String,
    /// Inside the conversation drawer: messages in their own scroller, no outer margins,
    /// 14px sides.
    pub embedded: bool,
}

impl Default for DiscussionOptions {
    fn default() -> Self {
        Self {
            full: false,
            page_mode: false,
            thread_id: DISCUSSION.into(),
            embedded: false,
        }
    }
}

/// Where an instance is shown: one instance per object and place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Place {
    /// The conversation drawer's chat (`embedded`).
    Pane,
    /// The full-screen chat page (`page_mode`).
    Page,
    /// In the object's page (collapsed opener, or a chat object's full body).
    Inline,
}

impl Place {
    pub(crate) fn of(options: &DiscussionOptions) -> Place {
        if options.embedded {
            Place::Pane
        } else if options.page_mode {
            Place::Page
        } else {
            Place::Inline
        }
    }

    fn name(self) -> &'static str {
        match self {
            Place::Pane => "pane",
            Place::Page => "page",
            Place::Inline => "inline",
        }
    }
}

/// The instance key of `object_id`'s discussion at `place`; it also scopes the element keys.
pub(crate) fn instance_key(object_id: &str, place: Place) -> String {
    format!("disc-{}-{object_id}", place.name())
}

/// The paired harness's live turn state for one agent (`/agent/status`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct AgentPresence {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// "idle" | "working" | "error"
    pub state: String,
    pub surface: String,
    pub detail: String,
    pub ts: i64,
}

/// Whether an agent's live state belongs in the chat of `object_id`.
fn presence_belongs(p: &AgentPresence, object_id: &str, guests: &[String]) -> bool {
    !object_id.is_empty() && (p.surface == object_id || p.id == object_id || guests.contains(&p.id))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StatusBody {
    agents: Option<Vec<AgentPresence>>,
}

/// An object linked into the next message with `!Name`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectTag {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub type_key: String,
}

/// An open `@…` / `!…` menu: the byte offset of its sigil and the text typed after it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TagQuery {
    pub start: usize,
    pub query: String,
}

const PRESENCE_POLL: Duration = Duration::from_millis(2500);
/// How long a sent @-mention shows its agent typing without a reply.
const EXPECT_REPLY: Duration = Duration::from_secs(10 * 60);

/// How an arriving reply's words fade in (`El::reveal`).
pub(crate) const REVEAL: f32 = rune::edge::WORD_FADE.word_fade;

/// How long `text` takes to reveal: its words start one after another, 20 ms apart down to
/// 5.5 ms as the backlog reaches 24 (as Rune staggers them), each fading over [`REVEAL`],
/// with the growth spring's time on top for the row to land.
fn reveal_time(text: &str) -> Duration {
    let words = text.split_whitespace().count() as f32;
    let k = (words / 24.0).min(1.0);
    let stagger = 0.020 + (0.0055 - 0.020) * k;
    Duration::from_secs_f32(words * stagger + REVEAL + rune::GROWTH.duration)
}

pub(crate) const AGENT_TOKEN: &str = "agent:";
pub(crate) const OBJECT_TOKEN: &str = "object:";

/// Every discussion instance, by [`instance_key`].
#[derive(Default)]
pub(crate) struct Discussions {
    pub(crate) map: HashMap<String, Disc>,
    /// Instances a view asked for that the executor is creating.
    requested: RefCell<HashSet<String>>,
    /// The model a composer edits once its instance has gone (a key that lands in the frame
    /// after): nothing reads it.
    spare: TextEdit,
}

/// A scroll the message list asks for once laid out (handlers own scrolling).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ScrollAsk {
    /// The thread just opened: land at the end at once.
    Land,
    /// A new message to follow: glide to the end.
    Follow,
}

/// One shown discussion.
pub(crate) struct Disc {
    pub key: String,
    pub object_id: String,
    pub full: bool,
    pub page_mode: bool,
    pub thread_id: String,
    pub embedded: bool,

    pub object: Option<Arc<Object>>,
    pub messages: Rc<Vec<ChatMessage>>,
    pub conversation: Option<Conversation>,
    pub legacy: bool,
    pub is_exchange: bool,
    pub read_only: bool,
    pub members: Vec<AgentEndpoint>,
    pub exchange_title: String,

    pub open: bool,
    /// Each message's rendering inputs, hashed: the row's memo version.
    pub fingerprints: Vec<u64>,
    /// Each shown origin object's tag name (`None`: not listed, no tag), as the rows were
    /// last fingerprinted.
    origins: HashMap<String, Option<String>>,
    landed_on: String,
    /// The `object:thread` whose messages are shown (an arrival needs a thread already on
    /// screen).
    arrival_key: String,
    /// Someone else's message that just arrived at the end: its words fade in
    /// (`El::reveal`).
    pub arrived: Option<String>,
    /// `arrived` is still revealing: its row streams in the message list's growth stack
    /// until [`reveal_time`] has passed.
    pub arriving: bool,
    arrive_timer: Option<Task<()>>,
    /// The placeholder id (`sending-…`) each landed message replaced, by real id: its row
    /// keeps the placeholder's element keys, so landing neither closes one row nor grows
    /// another.
    pub row_ids: HashMap<String, String>,
    /// Bumped when the shown rows are replaced wholesale (another thread, a first load of
    /// several messages): the message list's growth stack takes a new key, so they land
    /// at once rather than growing in.
    pub rows_epoch: u64,
    seen_count: usize,
    focused_once: bool,
    seeded: bool,
    /// The message list shows its end (its viewport's `on_at_bottom`).
    pub at_end: bool,
    /// The message list's viewport height, for the tall-reply check.
    pub viewport_h: f32,
    pub scroll_ask: Option<ScrollAsk>,
    /// A newly followed message to check once laid out (taller than the pane: show its top).
    pub tall_check: Option<String>,
    /// The list was asked to follow: the tall check may run.
    pub tall_armed: bool,

    /// The composer's model: the draft with each live tag as a pill.
    pub edit: TextEdit,
    /// The draft as text (`@Name` / `!Name` for the pills), as it is saved and sent.
    pub draft: String,
    /// The caret in `draft`, as last seen.
    caret: usize,
    pub reply_to: String,
    pub sending: bool,
    pub send_error: String,
    pub me: String,
    pub identity_error: String,
    pub private_recipient: String,
    pub request_reply: bool,
    pub retry_error: String,
    pub retrying: Vec<String>,
    pending_send: Option<AgentMessage>,
    /// A chat line just sent, shown at once (marked "Sending…") until the object carries
    /// the real one or the write fails.
    pub outgoing: Option<ChatMessage>,

    pub tag_options: Option<Vec<ObjectAgentOption>>,
    tag_loading: Option<Task<()>>,
    pub mention: Option<TagQuery>,
    pub mention_index: usize,
    pub tagged: Vec<ObjectAgentOption>,
    pub object_tag: Option<TagQuery>,
    pub object_tag_index: usize,
    pub tagged_objects: Vec<ObjectTag>,
    pub opened_rank: HashMap<String, usize>,
    /// The reply-audience menu is open.
    pub audience_open: bool,

    pub presence: Vec<AgentPresence>,
    /// The agent this device just @-mentioned and the send time: shown typing until it posts
    /// here, its harness reports an error, or [`EXPECT_REPLY`] passes. Client-side only.
    expecting: Option<(AgentPresence, i64)>,
    expect_timer: Option<Task<()>>,
    pub presence_error: String,
    poll: Option<Task<()>>,
}

/// Runs `f` on instance `key` with the state: the instance leaves its slot meanwhile, so `f`
/// may use all of `s`. `None` when the instance is gone (async work outliving its view).
pub(crate) fn with<R>(
    s: &mut State,
    key: &str,
    f: impl FnOnce(&mut Disc, &mut State) -> R,
) -> Option<R> {
    let mut d = s.slot::<Discussions>().map.remove(key)?;
    let out = f(&mut d, s);
    s.slot::<Discussions>().map.insert(key.to_string(), d);
    Some(out)
}

/// The instance `key`, if it exists.
pub(crate) fn get<'a>(s: &'a State, key: &str) -> Option<&'a Disc> {
    s.get::<Discussions>()?.map.get(key)
}

/// The composer model of instance `key` (a spare one once it has gone).
pub(crate) fn composer_edit<'a>(s: &'a mut State, key: &str) -> &'a mut TextEdit {
    let discussions = s.slot::<Discussions>();
    match discussions.map.get_mut(key) {
        Some(d) => &mut d.edit,
        None => &mut discussions.spare,
    }
}

/// A view needs instance `key`: create it on the executor's next run (once). The view draws
/// nothing for it meanwhile; creating it re-runs the view.
pub(crate) fn request(s: &State, key: &str, object_id: &str, options: &DiscussionOptions) {
    let Some(discussions) = s.get::<Discussions>() else {
        // Not registered: create the slot along with the instance.
        spawn_create(s, key, object_id, options);
        return;
    };
    if discussions.map.contains_key(key) || !discussions.requested.borrow_mut().insert(key.into()) {
        return;
    }
    spawn_create(s, key, object_id, options);
}

fn spawn_create(s: &State, key: &str, object_id: &str, options: &DiscussionOptions) {
    let (key, object_id, options) = (key.to_string(), object_id.to_string(), options.clone());
    s.rt.spawn(async move |cx| {
        cx.update(|s| {
            s.slot::<Discussions>().requested.borrow_mut().remove(&key);
            if !s.slot::<Discussions>().map.contains_key(&key) {
                create(s, key, object_id, options);
            }
        })
    })
    .detach();
}

/// Makes instance `key` of `object_id`'s discussion (GPUI's `Discussion::new`): loads the
/// object into the store cache, restores the thread's draft, asks who you are.
pub(crate) fn create(s: &mut State, key: String, object_id: String, options: DiscussionOptions) {
    s.store.ensure_object(&object_id);
    let thread_id = if options.thread_id.is_empty() {
        DISCUSSION.to_string()
    } else {
        options.thread_id
    };
    let draft = load_draft(&object_id, &thread_id, s);
    // A restored draft is picked up where it ends.
    let mut edit = TextEdit::new(draft.clone());
    edit.set_caret(draft.len());
    let d = Disc {
        key: key.clone(),
        object_id,
        full: options.full,
        page_mode: options.page_mode,
        thread_id,
        embedded: options.embedded,
        object: None,
        messages: Rc::new(Vec::new()),
        conversation: None,
        legacy: false,
        is_exchange: false,
        read_only: false,
        members: Vec::new(),
        exchange_title: "Exchange".into(),
        open: false,
        fingerprints: Vec::new(),
        origins: HashMap::new(),
        landed_on: String::new(),
        arrival_key: String::new(),
        arrived: None,
        arriving: false,
        arrive_timer: None,
        row_ids: HashMap::new(),
        rows_epoch: 0,
        seen_count: 0,
        focused_once: false,
        seeded: false,
        at_end: true,
        viewport_h: 0.0,
        scroll_ask: None,
        tall_check: None,
        tall_armed: false,
        edit,
        caret: draft.len(),
        draft,
        reply_to: String::new(),
        sending: false,
        send_error: String::new(),
        me: String::new(),
        identity_error: String::new(),
        private_recipient: String::new(),
        request_reply: true,
        retry_error: String::new(),
        retrying: Vec::new(),
        pending_send: None,
        outgoing: None,
        tag_options: None,
        tag_loading: None,
        mention: None,
        mention_index: 0,
        tagged: Vec::new(),
        object_tag: None,
        object_tag_index: 0,
        tagged_objects: Vec::new(),
        opened_rank: HashMap::new(),
        audience_open: false,
        presence: Vec::new(),
        expecting: None,
        expect_timer: None,
        presence_error: String::new(),
        poll: None,
    };
    s.slot::<Discussions>().map.insert(key.clone(), d);
    with(s, &key, |d, s| {
        d.load_identity(s);
        d.sync_object(s);
    });
}

/// Drops instance `key` (its tasks with it).
pub(crate) fn drop_instance(s: &mut State, key: &str) {
    s.slot::<Discussions>().map.remove(key);
}

/// Drops every instance of `object_id`.
pub(crate) fn forget_object(s: &mut State, object_id: &str) {
    s.slot::<Discussions>()
        .map
        .retain(|_, d| d.object_id != object_id);
}

/// Drops the instances whose object no tab shows any more.
pub(crate) fn retain_objects(s: &mut State, shown: &HashSet<String>) {
    s.slot::<Discussions>()
        .map
        .retain(|_, d| shown.contains(&d.object_id));
}

/// `State::on_poll`: every instance follows the store (GPUI's `observe(store)`) and its
/// composer's caret and focus (GPUI's mouse-up / arrow key-up / blur handlers).
pub(crate) fn poll_all(s: &mut State) -> bool {
    let keys: Vec<String> = match s.get::<Discussions>() {
        Some(d) => d.map.keys().cloned().collect(),
        None => return false,
    };
    let mut changed = false;
    for key in keys {
        changed |= with(s, &key, |d, s| d.sync_object(s) | d.follow_caret(s)).unwrap_or(false);
    }
    changed
}

/// `StoreEvent::Refreshed` in lite mode: the agent list is what this device holds; a refresh
/// (the space finished loading) lists them again.
pub(crate) fn on_store(s: &mut State, event: &StoreEvent) {
    if !matches!(event, StoreEvent::Refreshed) || roostr_core::lite::status(s).is_none() {
        return;
    }
    let keys: Vec<String> = match s.get::<Discussions>() {
        Some(d) => d.map.keys().cloned().collect(),
        None => return,
    };
    for key in keys {
        with(s, &key, |d, s| {
            if d.tag_loading.is_none() {
                d.tag_options = None;
                if d.mention.is_some() {
                    d.ensure_tag_options(s);
                }
            }
        });
    }
}

impl Disc {
    pub(crate) fn is_open(&self) -> bool {
        self.full || self.page_mode || self.open
    }

    /// Messages live in their own scroller (drawer, chat page) rather than the page's.
    pub(crate) fn own_scroller(&self) -> bool {
        self.embedded || self.page_mode
    }

    // ------------------------------------------------------------ object state

    /// Re-derive everything from the cached object when it changed. True when anything the
    /// view shows changed.
    pub(crate) fn sync_object(&mut self, s: &mut State) -> bool {
        let object = s.store.object(&self.object_id);
        let changed = match (&self.object, &object) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let guests = |o: &Option<Arc<Object>>| o.as_ref().map(|o| guest_agents(&o.fields));
            let agents_changed = guests(&self.object) != guests(&object);
            self.object = object;
            // Agents added or removed on the object: the @-options and the composer's
            // default tag follow at once, no reload. Reset before `derive`, which starts the
            // load a restored draft needs.
            if agents_changed {
                self.tag_options = None;
                self.tag_loading = None;
            }
            self.derive(s);
            if agents_changed {
                if self.draft.is_empty() {
                    self.seed_default_agent(Vec::new(), s);
                } else {
                    // A kept draft: its `@Agent` names become tags once the agents load.
                    self.ensure_tag_options(s);
                }
            }
            return true;
        }
        let renamed = self
            .origins
            .iter()
            .any(|(id, label)| origin_label(id, &self.object_id, &s.store) != *label);
        if renamed {
            self.sync_list(s);
        }
        renamed
    }

    pub(crate) fn derive(&mut self, s: &mut State) {
        let Some(object) = self.object.clone() else {
            return;
        };
        let thread = self.thread_id.as_str();
        let mut messages = if thread == DISCUSSION {
            object_chat_messages(&object)
        } else {
            chat_messages(&object, thread)
        };
        // The line being sent shows until the object has it (same author and text, written
        // no earlier than it).
        if let Some(out) = &self.outgoing {
            let landed = messages
                .iter()
                .find(|m| m.author == out.author && m.text == out.text && m.ts >= out.ts - 5_000);
            if let Some(m) = landed {
                self.row_ids.insert(m.id.clone(), out.id.clone());
                self.outgoing = None;
            } else {
                messages.push(out.clone());
            }
        }
        roostr_core::sync_trace!(
            "chat view {} thread {thread}: shows {} message(s){}",
            self.object_id,
            messages.len(),
            messages
                .last()
                .map(|m| format!(
                    ", latest at {} ms by {}: {:?}",
                    m.ts,
                    m.author,
                    roostr_core::sync_trace::head(&m.text, 60)
                ))
                .unwrap_or_default()
        );
        self.conversation = object
            .conversations
            .iter()
            .flatten()
            .find(|c| c.id == thread)
            .cloned();
        self.legacy = is_legacy_exchange(&object, thread);
        // The object's one chat (__discussion__) is a discussion even though it shows
        // envelopes: tagging sends one, an untagged line stays a chat line. Only an opened
        // exchange thread is reply-to-all.
        self.is_exchange = thread != DISCUSSION
            && (self.legacy
                || self.conversation.as_ref().is_some_and(|c| c.kind == "a2a")
                || messages.iter().any(|m| m.mailbox.is_some()));
        let closed = self.conversation.as_ref().is_some_and(|c| c.closed);
        self.read_only = self.legacy
            || (self.is_exchange && closed)
            || (thread != DISCUSSION && self.conversation.is_none() && messages.is_empty());
        self.members = unique_endpoints(
            messages
                .iter()
                .filter_map(|m| m.mailbox.as_ref())
                .flat_map(|e| {
                    std::iter::once(&e.message.sender).chain(e.message.recipients.iter())
                }),
        );
        self.exchange_title = self
            .conversation
            .as_ref()
            .map(|c| c.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                messages
                    .iter()
                    .find_map(|m| m.mailbox.as_ref())
                    .map(|e| e.message.title.clone())
                    .filter(|t| !t.is_empty())
            })
            .unwrap_or_else(|| "Exchange".into());
        let previous = std::mem::replace(&mut self.messages, Rc::new(messages));
        let at_end = self.at_end;
        self.note_arrival(&previous, at_end, s);
        self.sync_list(s);
        self.follow_new_messages(at_end);
        self.update_polling(s);
        if self.is_open() && !self.focused_once {
            // The composer takes the focus once, when the open thread first shows.
            self.focused_once = true;
            self.edit.focused = true;
        }
        if !self.seeded {
            self.seeded = true;
            if self.draft.is_empty() {
                self.seed_default_agent(Vec::new(), s);
            } else {
                // A draft kept from before (reload, reopened chat): its `@Agent` names
                // become tags again once the agents are known.
                self.ensure_tag_options(s);
                self.retag_draft(s);
            }
        }
    }

    /// Tag every agent of this object whose `@Name` is written out in the draft (ending it
    /// or followed by a non-word character), as a pick would - for drafts restored from
    /// storage, which keep only text.
    fn retag_draft(&mut self, s: &mut State) {
        let Some(options) = self.tag_options.clone() else {
            return;
        };
        let mut added = false;
        for option in options {
            if option.agent_name.is_empty() || self.tagged.iter().any(|t| t.key() == option.key()) {
                continue;
            }
            let label = format!("@{}", option.agent_name);
            let written = self.draft.match_indices(&label).any(|(at, _)| {
                self.draft[at + label.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !is_word_char(c))
            });
            if written {
                self.tagged.push(option);
                added = true;
            }
        }
        if added {
            self.repill(s);
        }
    }

    pub(crate) fn message(&self, id: &str) -> Option<&ChatMessage> {
        self.messages.iter().find(|m| m.id == id)
    }

    /// Someone else's message arriving at the end of the thread on screen streams in: its
    /// row grows on the growth stack's streaming pace while its words fade in, then lands.
    /// The thread's first load, history and your own sent messages don't, nor anything
    /// arriving while you read further up. Rows replaced wholesale bump [`Disc::rows_epoch`].
    fn note_arrival(&mut self, previous: &[ChatMessage], at_end: bool, s: &mut State) {
        let key = format!("{}:{}", self.object_id, self.thread_id);
        let shown = std::mem::replace(&mut self.arrival_key, key.clone()) == key;
        if !shown || (previous.is_empty() && self.messages.len() > 1) {
            self.rows_epoch += 1;
        }
        let Some(last) = self.messages.last() else {
            return;
        };
        if shown
            && self.is_open()
            && at_end
            && last.author != self.me
            && !previous.is_empty()
            && !previous.iter().any(|m| m.id == last.id)
        {
            let (id, wait) = (last.id.clone(), reveal_time(&last.text));
            self.arrived = Some(id.clone());
            self.arriving = true;
            let key = self.key.clone();
            self.arrive_timer = Some(s.rt.spawn(async move |cx| {
                cx.timer(wait).await;
                cx.update(|s| {
                    with(s, &key, |d, _| {
                        if d.arrived.as_deref() == Some(id.as_str()) {
                            d.arriving = false;
                        }
                    })
                });
            }));
        }
    }

    /// Each message's rendering inputs hashed (its row's memo version), and the origin tags'
    /// names they used.
    fn sync_list(&mut self, s: &State) {
        let store = &s.store;
        // A message's origin tag shows its object's name, or nothing once the object is
        // gone: a store change can redraw a row without touching it.
        let origins: HashMap<String, Option<String>> = self
            .messages
            .iter()
            .filter(|m| !m.origin.is_empty())
            .map(|m| {
                (
                    m.origin.clone(),
                    origin_label(&m.origin, &self.object_id, store),
                )
            })
            .collect();
        self.fingerprints = self
            .messages
            .iter()
            .map(|m| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (
                    &m.id,
                    &m.author,
                    m.ts,
                    &m.text,
                    &m.origin,
                    origins.get(&m.origin),
                    &self.me,
                    self.read_only,
                    self.is_exchange,
                )
                    .hash(&mut h);
                if let Some(e) = &m.mailbox {
                    (
                        e.incoming,
                        e.outgoing,
                        &e.processing.status,
                        &e.processing.error,
                    )
                        .hash(&mut h);
                    for d in &e.deliveries {
                        (&d.status, &d.error, &d.recipient.object_id).hash(&mut h);
                    }
                }
                h.finish()
            })
            .collect();
        self.origins = origins;
    }

    /// Land at the end once per opened thread; on new messages follow when the reader was at
    /// the end, and always for a reply to what you just sent.
    fn follow_new_messages(&mut self, at_end: bool) {
        if !self.own_scroller() {
            self.seen_count = self.messages.len();
            return;
        }
        let key = format!("{}:{}", self.object_id, self.thread_id);
        if !self.is_open() {
            self.landed_on.clear();
        } else if self.landed_on != key && self.object.is_some() {
            self.landed_on = key;
            self.seen_count = self.messages.len();
            self.scroll_ask = Some(ScrollAsk::Land);
            return;
        }
        let count = self.messages.len();
        if count == self.seen_count {
            return;
        }
        let fresh = count > self.seen_count;
        self.seen_count = count;
        if !fresh {
            return;
        }
        let newest = &self.messages[count - 1];
        let previous = count.checked_sub(2).map(|i| &self.messages[i]);
        let mine_before = previous.is_some_and(|p| p.author == self.me) && newest.author != self.me;
        if !(self.page_mode || at_end || mine_before || newest.author == self.me) {
            return;
        }
        self.scroll_ask = Some(ScrollAsk::Follow);
        self.tall_check = Some(newest.id.clone());
        self.tall_armed = false;
    }

    // --------------------------------------------------------------- identity

    pub(crate) fn load_identity(&mut self, s: &mut State) {
        let backend = s.store.backend();
        let settings = s.rt.background(async move { backend.settings().await });
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let result = settings.await;
            cx.update(|s| {
                with(s, &key, |d, s| {
                    match result {
                        Ok(settings) => {
                            d.me = settings.author_id;
                            d.identity_error.clear();
                        }
                        Err(error) => {
                            d.me.clear();
                            let message = error.to_string();
                            d.identity_error = if message.is_empty() {
                                "Discussion identity is unavailable while offline.".into()
                            } else {
                                message
                            };
                        }
                    }
                    d.sync_list(s);
                })
            });
        })
        .detach();
    }

    // --------------------------------------------------------------- presence

    /// Poll the paired harness while open so the user sees an agent composing (typing dots)
    /// or failing (warning row).
    fn update_polling(&mut self, s: &mut State) {
        let harness = s.harness.clone();
        let wanted = self.is_open() && harness.is_some() && !self.is_exchange;
        if !wanted {
            self.poll = None;
            self.presence.clear();
            self.presence_error.clear();
            return;
        }
        if self.poll.is_some() {
            return;
        }
        let Some(harness) = harness else { return };
        let key = self.key.clone();
        self.poll = Some(s.rt.spawn(async move |cx| {
            loop {
                let h = harness.clone();
                let result = cx
                    .background(async move { h.get_json("/agent/status").await })
                    .await;
                let alive = cx.update(|s| {
                    with(s, &key, |d, _| {
                        let (presence, error) = match result {
                            Ok(body) => (
                                serde_json::from_value::<StatusBody>(body)
                                    .unwrap_or_default()
                                    .agents
                                    .unwrap_or_default(),
                                String::new(),
                            ),
                            Err(error) => (Vec::new(), presence_failure(&error)),
                        };
                        if presence != d.presence || error != d.presence_error {
                            d.presence = presence;
                            d.presence_error = error;
                        }
                    })
                    .is_some()
                });
                if !alive {
                    return;
                }
                cx.timer(PRESENCE_POLL).await;
            }
        }));
    }

    /// The harness reports every agent; this chat shows only its own: one whose turn runs on
    /// this object, the agent whose page this is, or one on the object's Agent list (a
    /// recurring object's runs live in the agent's own transcript, so its surface is the
    /// agent, not the object).
    fn presence_here(&self) -> impl Iterator<Item = &AgentPresence> {
        let (object_id, guests) = match &self.object {
            Some(o) => (o.id.as_str(), guest_agents(&o.fields)),
            None => ("", vec![]),
        };
        self.presence
            .iter()
            .filter(move |p| presence_belongs(p, object_id, &guests))
    }

    pub(crate) fn agent_working(&self) -> Option<&AgentPresence> {
        self.presence_here()
            .find(|p| p.state == "working")
            .or_else(|| self.expected_reply())
    }

    /// The agent this device @-mentioned, while its reply is still owed.
    fn expected_reply(&self) -> Option<&AgentPresence> {
        let (agent, since) = self.expecting.as_ref()?;
        let now = chrono::Utc::now().timestamp_millis();
        let replied = self
            .messages
            .iter()
            .any(|m| m.author == agent.id && m.ts >= *since);
        let failed = self
            .presence
            .iter()
            .any(|p| p.id == agent.id && p.state == "error" && p.ts >= *since);
        let expired = now - since >= EXPECT_REPLY.as_millis() as i64;
        (!replied && !failed && !expired).then_some(agent)
    }

    /// Show `option`'s agent typing from now (its reply is assumed) and repaint when that
    /// assumption runs out.
    fn expect_reply(&mut self, option: &ObjectAgentOption, s: &mut State) {
        let id = option.endpoint.agent_id.clone();
        let icon = s
            .store
            .agent(&id)
            .map(|a| a.icon.clone())
            .unwrap_or_else(|| option.icon.clone());
        let agent = AgentPresence {
            id,
            name: option.agent_name.clone(),
            icon,
            state: "working".into(),
            ..AgentPresence::default()
        };
        self.expecting = Some((agent, chrono::Utc::now().timestamp_millis()));
        let key = self.key.clone();
        self.expect_timer = Some(s.rt.spawn(async move |cx| {
            cx.timer(EXPECT_REPLY).await;
            cx.update(|s| with(s, &key, |d, _| d.expecting = None));
        }));
    }

    pub(crate) fn agent_error(&self) -> Option<&AgentPresence> {
        let now = chrono::Utc::now().timestamp_millis();
        self.presence_here()
            .find(|p| p.state == "error" && now - p.ts < 15 * 60_000)
    }

    // ------------------------------------------------------------------ names

    pub(crate) fn who(&self, author: &str, store: &Store) -> String {
        if author == self.me {
            return "You".into();
        }
        if author == "scheduler" {
            return "Scheduler".into();
        }
        // An agent posts as its own object id, wherever it is replying. It is absent from
        // `summaries` (agents are hidden infrastructure), so the name comes from the agent
        // roster.
        if let Some(agent) = store.agent(author) {
            return if agent.name.is_empty() {
                "Agent".into()
            } else {
                agent.name.clone()
            };
        }
        if let Some(object) = &self.object {
            // A chat object's own id: the brain-chat surface.
            if author == object.id {
                return if object.name().is_empty() {
                    "Agent".into()
                } else {
                    object.name().to_string()
                };
            }
            if !author.is_empty() && guest_agents(&object.fields).iter().any(|a| a == author) {
                return "Agent".into();
            }
        }
        // A person (or any object) speaking in a recreated meeting or email thread.
        if let Some(subject) = store.summary(author) {
            return if subject.name.is_empty() {
                "Untitled".into()
            } else {
                subject.name.clone()
            };
        }
        author.chars().take(6).collect()
    }

    /// Authors that are objects (agents, people) link to themselves.
    pub(crate) fn author_links(&self, author: &str, store: &Store) -> bool {
        store.agent(author).is_some() || store.summary(author).is_some()
    }

    /// Avatar letters: a named author's initials, else the id's first two characters.
    pub(crate) fn initials(&self, author: &str, store: &Store) -> String {
        let name = self.who(author, store);
        if name == author.chars().take(6).collect::<String>() {
            return author.chars().take(2).collect();
        }
        initials_of(&name)
    }

    pub(crate) fn member_name(&self, endpoint: &AgentEndpoint, store: &Store) -> String {
        if let Some(object) = &self.object
            && endpoint.object_id == object.id
        {
            let object_name = if object.name().is_empty() {
                "This object".to_string()
            } else {
                object.name().to_string()
            };
            let agent_name = store.agent(&endpoint.agent_id).map(|a| a.name.clone());
            return match agent_name {
                Some(agent) if !agent.is_empty() && agent != object_name => {
                    format!("{object_name} · {agent}")
                }
                _ => object_name,
            };
        }
        endpoint_name(store, endpoint)
    }

    /// Emoji avatar for an author, when it has one. An agent always has one: its own icon,
    /// else its type's (🤖), as everywhere else it is shown.
    pub(crate) fn avatar_emoji(&self, author: &str, store: &Store) -> String {
        if let Some(agent) = store.agent(author) {
            if !agent.icon.is_empty() {
                return agent.icon.clone();
            }
            return object_icon(store, "", "agent", &agent.channel);
        }
        if let Some(object) = &self.object
            && author == object.id
        {
            return object.field_str("iconEmoji").to_string();
        }
        store
            .summary(author)
            .map(|s| s.icon.clone())
            .unwrap_or_default()
    }

    // ------------------------------------------------------------ exchanges

    fn reply_message(&self) -> Option<AgentMessage> {
        let message = if self.reply_to.is_empty() {
            self.messages.iter().rev().find(|m| m.mailbox.is_some())
        } else {
            self.message(&self.reply_to)
        };
        message
            .and_then(|m| m.mailbox.as_ref())
            .map(|e| e.message.clone())
    }

    pub(crate) fn reply_all(&self) -> Vec<AgentEndpoint> {
        let responder = AgentEndpoint {
            object_id: self.object_id.clone(),
            agent_id: String::new(),
        };
        self.reply_message()
            .map(|m| reply_recipients(&m, &responder))
            .unwrap_or_default()
    }

    pub(crate) fn audience(&self) -> Vec<AgentEndpoint> {
        let all = self.reply_all();
        if self.private_recipient.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|e| e.object_id == self.private_recipient)
                .collect()
        }
    }

    pub(crate) fn start_reply(&mut self, message_id: &str, privately: bool) {
        self.reply_to = message_id.to_string();
        let target = self
            .message(message_id)
            .and_then(|m| m.mailbox.as_ref())
            .map(|e| e.message.clone());
        self.private_recipient = match (privately, target) {
            (true, Some(target)) => {
                let responder = AgentEndpoint {
                    object_id: self.object_id.clone(),
                    agent_id: String::new(),
                };
                reply_recipients(&target, &responder)
                    .into_iter()
                    .find(|e| e.object_id == target.sender.object_id)
                    .map(|e| e.object_id)
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        self.edit.focused = true;
    }

    // ------------------------------------------------------------- composer

    /// The pills the composer shows for the live tags, by id.
    fn pill_tags(&self, s: &State) -> Vec<PillTag> {
        let space = s.nav.space().to_string();
        let agents = self.tagged.iter().map(|o| {
            let icon = s
                .store
                .agents
                .iter()
                // GPUI's name -> icon map: the last agent of that name wins.
                .rfind(|a| a.name == o.agent_name)
                .map(|a| a.icon.clone())
                .filter(|i| !i.is_empty() && !is_image_icon(i))
                .unwrap_or_else(|| "🤖".into());
            PillTag {
                id: format!("{AGENT_TOKEN}{}", o.key()),
                logical: format!("@{}", o.agent_name),
                label: pill_label(&icon, &o.agent_name),
            }
        });
        let objects = self.tagged_objects.iter().map(|t| {
            let icon = object_icon(&s.store, &t.icon, &t.type_key, &space);
            let icon = if is_image_icon(&icon) {
                object_icon(&s.store, "", &t.type_key, &space)
            } else {
                icon
            };
            PillTag {
                id: format!("{OBJECT_TOKEN}{}", t.id),
                logical: format!("!{}", t.name),
                label: pill_label(&icon, &t.name),
            }
        });
        agents.chain(objects).collect()
    }

    /// The field changed (typing, paste, a pill deleted): the draft follows it.
    pub(crate) fn on_change(&mut self, s: &mut State) {
        let tags = self.pill_tags(s);
        let (draft, caret) = decompose(&self.edit, &tags);
        self.draft = draft;
        self.caret = caret;
        save_draft(&self.object_id, &self.thread_id, &self.draft, s);
        self.update_mention(s);
    }

    /// The caret moved without an edit (a click, an arrow): the menus follow it; the field
    /// lost the focus: they close. True when the view must change.
    fn follow_caret(&mut self, s: &mut State) -> bool {
        let mut changed = false;
        if !self.edit.focused && (self.mention.is_some() || self.object_tag.is_some()) {
            self.mention = None;
            self.object_tag = None;
            return true;
        }
        let tags = self.pill_tags(s);
        let (draft, caret) = decompose(&self.edit, &tags);
        if draft == self.draft && caret != self.caret {
            self.caret = caret;
            let before = (self.mention.clone(), self.object_tag.clone());
            self.update_mention(s);
            changed = before != (self.mention.clone(), self.object_tag.clone());
        }
        changed
    }

    /// Replace the composer's text, re-creating the pills of live tags.
    pub(crate) fn set_draft(&mut self, text: String, caret: Option<usize>, s: &mut State) {
        let tags = self.pill_tags(s);
        let focused = self.edit.focused;
        let caret = caret.unwrap_or(text.len()).min(text.len());
        self.edit = compose(&text, caret, &tags);
        self.edit.focused = focused;
        self.caret = caret;
        self.draft = text;
        save_draft(&self.object_id, &self.thread_id, &self.draft, s);
    }

    /// A tag was made from typed text: the draft shows it as a pill now.
    fn repill(&mut self, s: &mut State) {
        let (draft, caret) = (self.draft.clone(), self.caret);
        self.set_draft(draft, Some(caret), s);
    }

    /// Open/refresh/close a menu from the caret: whichever `@…` or `!…` token ends at the
    /// caret and started last wins.
    fn update_mention(&mut self, s: &mut State) {
        // A tag deleted from the text untags immediately, so the menu offers that agent again.
        let draft = self.draft.clone();
        self.tagged
            .retain(|o| draft.contains(&format!("@{}", o.agent_name)));
        self.tagged_objects
            .retain(|t| draft.contains(&format!("!{}", t.name)));
        let caret = self.caret.min(draft.len());
        let before = &draft[..caret];
        let hit = tag_query(before, '@', &['@', '\n']);
        let obj_hit = tag_query(before, '!', &['!', '@', '\n']);
        if let Some(obj) = &obj_hit
            && hit.as_ref().is_none_or(|h| obj.query.len() < h.query.len())
        {
            self.mention = None;
            self.update_object_tag(obj.clone(), s);
            return;
        }
        self.object_tag = None;
        let Some(hit) = hit else {
            self.mention = None;
            return;
        };
        // A completed tag is only `@Name ` in the draft; once the caret moves past it the
        // token still matches, so close explicitly.
        if self
            .tagged
            .iter()
            .any(|o| hit.query.starts_with(&format!("{} ", o.agent_name)))
        {
            self.mention = None;
            return;
        }
        // `@Name ` typed out in full (not picked from the menu) tags the agent as a pick
        // would - once the agents are known (re-run when they load). Capitals don't matter:
        // the text takes the agent's own spelling.
        if hit.query.ends_with(char::is_whitespace)
            && let Some(options) = &self.tag_options
        {
            let name = hit.query.trim_end();
            let lower = name.to_lowercase();
            if let Some(option) = options.iter().find(|o| {
                o.agent_name.to_lowercase() == lower
                    && !self.tagged.iter().any(|t| t.key() == o.key())
            }) {
                let option = option.clone();
                if option.agent_name != name {
                    // `@` + the typed name -> `@` + the agent's name.
                    let at = hit.start + '@'.len_utf8();
                    self.draft
                        .replace_range(at..at + name.len(), &option.agent_name);
                    self.caret = self.caret + option.agent_name.len() - name.len();
                }
                self.tagged.push(option);
                self.mention = None;
                self.repill(s);
                return;
            }
        }
        if self.mention.as_ref().is_none_or(|m| m.query != hit.query) {
            self.mention_index = 0;
        }
        self.mention = Some(hit);
        self.ensure_tag_options(s);
    }

    pub(crate) fn ensure_tag_options(&mut self, s: &mut State) {
        if self.tag_options.is_some() || self.tag_loading.is_some() {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        let task = load_object_agents(&object, s);
        let key = self.key.clone();
        self.tag_loading = Some(s.rt.spawn(async move |cx| {
            let options = task.await.unwrap_or_default();
            cx.update(|s| {
                with(s, &key, |d, s| {
                    d.tag_options = Some(options);
                    d.tag_loading = None;
                    d.retag_draft(s);
                    d.update_mention(s);
                })
            });
        }));
    }

    pub(crate) fn mention_matches(&self) -> Vec<ObjectAgentOption> {
        let (Some(mention), Some(options)) = (&self.mention, &self.tag_options) else {
            return Vec::new();
        };
        // "@Grant Email " (a space typed after the name) still lists it.
        let q = mention.query.trim_end().to_lowercase();
        options
            .iter()
            .filter(|o| !self.tagged.iter().any(|t| t.key() == o.key()))
            .filter(|o| {
                q.is_empty()
                    || o.agent_name.to_lowercase().contains(&q)
                    || o.name.to_lowercase().contains(&q)
            })
            .take(8)
            .cloned()
            .collect()
    }

    pub(crate) fn pick_mention(&mut self, option: ObjectAgentOption, s: &mut State) {
        let Some(at) = self.mention.take() else {
            return;
        };
        let caret = self.caret.max(at.start).min(self.draft.len());
        let label = format!("@{}", option.agent_name);
        // One trailing space so the next word doesn't glue onto the tag.
        let rest = &self.draft[caret..];
        let after = if rest.starts_with(' ') {
            rest.to_string()
        } else {
            format!(" {rest}")
        };
        let text = format!("{}{label}{after}", &self.draft[..at.start]);
        if !self.tagged.iter().any(|t| t.key() == option.key()) {
            self.tagged.push(option);
        }
        let pos = at.start + label.len() + 1;
        self.edit.focused = true;
        self.set_draft(text, Some(pos), s);
    }

    fn update_object_tag(&mut self, hit: TagQuery, s: &State) {
        // A completed `!Name ` closes the menu once the caret moves on.
        if self
            .tagged_objects
            .iter()
            .any(|t| hit.query.starts_with(&format!("{} ", t.name)))
        {
            self.object_tag = None;
            return;
        }
        if self.object_tag.is_none() {
            self.opened_rank = roostr_core::recent::opened_ranks(s);
        }
        if self
            .object_tag
            .as_ref()
            .is_none_or(|t| t.query != hit.query)
        {
            self.object_tag_index = 0;
        }
        self.object_tag = Some(hit);
    }

    pub(crate) fn object_matches(&self, store: &Store) -> Vec<ObjectTag> {
        let Some(tag) = &self.object_tag else {
            return Vec::new();
        };
        let Some(object) = &self.object else {
            return Vec::new();
        };
        let q = tag.query.trim().to_lowercase();
        let fallback = store
            .channels
            .first()
            .map(|c| c.id.clone())
            .unwrap_or_default();
        let space = roostr_core::relations::object_space_id(store, &object.fields);
        let mut rows: Vec<&roostr_core::ObjectSummary> = store
            .summaries
            .iter()
            .filter(|s| {
                !s.name.trim().is_empty()
                    && (if s.channel_id.is_empty() {
                        &fallback
                    } else {
                        &s.channel_id
                    }) == &space
            })
            .filter(|s| !self.tagged_objects.iter().any(|t| t.id == s.id))
            .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q))
            .collect();
        rows.sort_by(|a, b| {
            let ra = self.opened_rank.get(&a.id).copied().unwrap_or(usize::MAX);
            let rb = self.opened_rank.get(&b.id).copied().unwrap_or(usize::MAX);
            ra.cmp(&rb).then(b.updated_at.cmp(&a.updated_at))
        });
        rows.into_iter()
            .take(50)
            .map(|s| ObjectTag {
                id: s.id.clone(),
                name: s.name.trim().to_string(),
                icon: s.icon.clone(),
                type_key: s.type_key.clone(),
            })
            .collect()
    }

    pub(crate) fn pick_object(&mut self, option: ObjectTag, s: &mut State) {
        let Some(at) = self.object_tag.take() else {
            return;
        };
        let caret = self.caret.max(at.start).min(self.draft.len());
        let label = format!("!{}", option.name);
        let rest = &self.draft[caret..];
        let after = if rest.starts_with(' ') {
            rest.to_string()
        } else {
            format!(" {rest}")
        };
        let text = format!("{}{label}{after}", &self.draft[..at.start]);
        if !self.tagged_objects.iter().any(|t| t.id == option.id) {
            self.tagged_objects.push(option);
        }
        let pos = at.start + label.len() + 1;
        self.edit.focused = true;
        self.set_draft(text, Some(pos), s);
    }

    /// The message as sent: every live `!Name` becomes a link to its object.
    fn link_object_tags(&self, text: &str) -> String {
        if self.tagged_objects.is_empty() {
            return text.to_string();
        }
        let mut names: Vec<&str> = self
            .tagged_objects
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        names.dedup();
        let mut out = String::new();
        let mut i = 0;
        let mut copied = 0;
        let bytes = text.as_bytes();
        while i < text.len() {
            let at_boundary = i == 0
                || text[..i]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            if bytes[i] == b'!' && at_boundary {
                let after = &text[i + 1..];
                if let Some(name) = names
                    .iter()
                    .find(|n| after.starts_with(**n) && !after[n.len()..].starts_with(is_word_char))
                    && let Some(tag) = self.tagged_objects.iter().find(|t| t.name == *name)
                {
                    out.push_str(&text[copied..i]);
                    out.push_str(&format!(
                        "[{}](/app/object/{})",
                        tag.name.replace(['[', ']'], ""),
                        tag.id
                    ));
                    i += 1 + name.len();
                    copied = i;
                    continue;
                }
            }
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        out.push_str(&text[copied..]);
        out
    }

    /// The composer opens already addressed: `@Name `, tagged, caret after (`seed_tags`:
    /// what was just sent, else the agents a person last @-tagged here, else the object's
    /// default agents - on an agent's own page that agent). Only ever fills an empty draft.
    /// `just_sent` is the tags of the message just posted.
    fn seed_default_agent(&mut self, just_sent: Vec<ObjectAgentOption>, s: &mut State) {
        if !self.draft.is_empty() || self.read_only || self.is_exchange {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        if self.tag_options.is_some() {
            self.finish_seed(just_sent, s);
            return;
        }
        let task = load_object_agents(&object, s);
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let options = task.await.unwrap_or_default();
            cx.update(|s| {
                with(s, &key, |d, s| {
                    if d.tag_options.is_none() {
                        d.tag_options = Some(options);
                    }
                    d.finish_seed(just_sent, s);
                })
            });
        })
        .detach();
    }

    fn finish_seed(&mut self, just_sent: Vec<ObjectAgentOption>, s: &mut State) {
        let options = self.tag_options.clone().unwrap_or_default();
        if !self.draft.is_empty() || options.is_empty() {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        let defaults = if object.type_key == "agent" {
            vec![object.id.clone()]
        } else {
            guest_agents(&object.fields)
        };
        let person_messages: Vec<&str> = self
            .messages
            .iter()
            .filter(|m| s.store.agent(&m.author).is_none())
            .map(|m| m.text.as_str())
            .collect();
        let tags = seed_tags(&options, &person_messages, &defaults, just_sent);
        if tags.is_empty() {
            return;
        }
        let text: String = tags.iter().map(|t| format!("@{} ", t.agent_name)).collect();
        self.tagged = tags;
        let caret = text.len();
        self.set_draft(text, Some(caret), s);
    }

    /// Tags whose @Name survived editing and are present in this text, plus any agent of
    /// this object whose `@Name` was typed out in full (ending the text or followed by a
    /// non-word character) without picking it.
    fn live_tags(&self, text: &str) -> Vec<ObjectAgentOption> {
        let mut tags: Vec<ObjectAgentOption> = self
            .tagged
            .iter()
            .filter(|o| text.contains(&format!("@{}", o.agent_name)))
            .cloned()
            .collect();
        for option in self.tag_options.iter().flatten() {
            if option.agent_name.is_empty() || tags.iter().any(|t| t.key() == option.key()) {
                continue;
            }
            let label = format!("@{}", option.agent_name);
            let typed = text.match_indices(&label).any(|(at, _)| {
                text[at + label.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !is_word_char(c))
            });
            if typed {
                tags.push(option.clone());
            }
        }
        tags
    }

    fn clear_composer(&mut self, s: &mut State) {
        self.reply_to.clear();
        self.tagged.clear();
        self.tagged_objects.clear();
        self.set_draft(String::new(), None, s);
    }

    /// Whether the send button does anything now.
    pub(crate) fn can_send(&self) -> bool {
        !(self.sending
            || self.draft.trim().is_empty()
            || (self.is_exchange && self.audience().is_empty()))
    }

    pub(crate) fn send(&mut self, s: &mut State) {
        let raw = self.draft.trim().to_string();
        let mentions = self.live_tags(&raw);
        let text = self.link_object_tags(&raw);
        if self.sending
            || self.read_only
            || text.is_empty()
            || (self.is_exchange && self.audience().is_empty() && mentions.is_empty())
        {
            return;
        }
        let reply = self.reply_to.clone();
        self.sending = true;
        self.send_error.clear();
        // The object's one chat is __discussion__: a line that @-mentions a guest agent is
        // the harness's wake signal, and the agent replies in the same thread - no envelope.
        // Envelopes are for an opened exchange thread (reply-to-all), where they carry the
        // audience.
        if self.is_exchange {
            let Some(reply_message) = self.reply_message() else {
                self.finish_send(Err(anyhow::anyhow!(
                    "Reload this exchange before replying."
                )));
                return;
            };
            let title = if self.private_recipient.is_empty() {
                self.exchange_title.clone()
            } else {
                format!("Private: {}", self.exchange_title)
            };
            let parent_id = if reply.is_empty() {
                reply_message.id.clone()
            } else {
                reply.clone()
            };
            let audience = self.audience();
            let recipients =
                unique_endpoints(audience.iter().chain(mentions.iter().map(|o| &o.endpoint)));
            let exchange_id = if self.private_recipient.is_empty() {
                reply_message.exchange_id.clone()
            } else {
                uuid::Uuid::new_v4().to_string()
            };
            let reuse = self.pending_send.as_ref().is_some_and(|p| {
                p.text == text
                    && p.reply_to == parent_id
                    && p.title == title
                    && p.request_reply == self.request_reply
                    && p.recipients == recipients
            });
            if !reuse {
                self.pending_send = Some(AgentMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    exchange_id,
                    sender: AgentEndpoint {
                        object_id: self.object_id.clone(),
                        agent_id: String::new(),
                    },
                    recipients,
                    text,
                    reply_to: parent_id,
                    sent_at: chrono::Utc::now().timestamp_millis(),
                    title,
                    request_reply: self.request_reply,
                    historical: false,
                    operation: String::new(),
                    author: String::new(),
                    unknown: None,
                });
            }
            let message = self.pending_send.clone().expect("pending send");
            let task = roostr_core::api::mailbox::send(&message, s);
            let key = self.key.clone();
            s.rt.spawn(async move |cx| {
                let result = task.await;
                cx.update(|s| {
                    with(s, &key, |d, s| match result {
                        Ok(sent) => {
                            d.pending_send = None;
                            d.private_recipient.clear();
                            d.clear_composer(s);
                            d.sending = false;
                            let thread = sent
                                .get("threadId")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            if !thread.is_empty() && thread != d.thread_id {
                                d.switch_thread(thread, s);
                            }
                        }
                        Err(error) => d.finish_send(Err(error)),
                    })
                });
            })
            .detach();
            return;
        }
        let thread = if self.thread_id == DISCUSSION {
            String::new()
        } else {
            self.thread_id.clone()
        };
        let task = roostr_core::api::chat::post(&self.object_id, &text, &reply, &thread, s);
        // Show it now: the write (and an agent's wake) can take a moment.
        self.outgoing = Some(ChatMessage {
            id: format!("sending-{}", uuid::Uuid::new_v4()),
            author: self.me.clone(),
            ts: chrono::Utc::now().timestamp_millis(),
            text: text.clone(),
            origin: String::new(),
            mailbox: None,
        });
        let draft = self.draft.clone();
        let tagged = self.tagged.clone();
        self.clear_composer(s);
        self.derive(s);
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let result = task.await;
            cx.update(|s| {
                with(s, &key, |d, s| match result {
                    Ok(_) => {
                        d.finish_send(Ok(()));
                        // The agent tagged last answers: show it typing until it does.
                        if let Some(agent) = mentions.last() {
                            d.expect_reply(agent, s);
                        }
                        d.seed_default_agent(mentions, s);
                    }
                    Err(error) => {
                        // Not sent: the line leaves the chat and goes back into the composer
                        // (unless something new was typed there).
                        d.outgoing = None;
                        if d.draft.is_empty() {
                            d.tagged = tagged;
                            d.set_draft(draft, None, s);
                        }
                        d.derive(s);
                        d.finish_send(Err(error));
                    }
                })
            });
        })
        .detach();
    }

    fn finish_send(&mut self, result: anyhow::Result<()>) {
        if let Err(error) = result {
            self.send_error = error.to_string();
        }
        self.sending = false;
    }

    /// A private reply opened its own exchange: show that thread.
    fn switch_thread(&mut self, thread: String, s: &mut State) {
        self.thread_id = thread;
        self.landed_on.clear();
        self.fingerprints.clear();
        let draft = load_draft(&self.object_id, &self.thread_id, s);
        self.tagged.clear();
        self.tagged_objects.clear();
        self.set_draft(draft, None, s);
        self.retag_draft(s);
        self.derive(s);
    }

    pub(crate) fn retry(
        &mut self,
        message_id: String,
        stage: &'static str,
        recipient: String,
        s: &mut State,
    ) {
        let key = format!("{message_id}:{stage}:{recipient}");
        if self.retrying.contains(&key) {
            return;
        }
        self.retrying.push(key.clone());
        self.retry_error.clear();
        let task = roostr_core::api::mailbox::retry(
            &self.object_id,
            &message_id,
            stage,
            if recipient.is_empty() {
                None
            } else {
                Some(recipient.as_str())
            },
            s,
        );
        let instance = self.key.clone();
        s.rt.spawn(async move |cx| {
            let result = task.await;
            cx.update(|s| {
                with(s, &instance, |d, _| {
                    if let Err(error) = result {
                        d.retry_error = error.to_string();
                    }
                    d.retrying.retain(|k| k != &key);
                })
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- keyboard

    /// The composer's keys before its editor (GPUI's captured actions): Enter, Tab, ↑/↓ and
    /// Escape drive the open `@` / `!` menu; Escape else cancels a reply. While sending the
    /// text can't change. True when the key is taken.
    pub(crate) fn on_key(&mut self, ev: &KeyEvent, s: &mut State) -> bool {
        use rune::Key;
        if self.sending && edits_text(ev) {
            return true;
        }
        match &ev.key {
            Key::Enter => self.on_enter(ev.mods.shift, s),
            Key::Tab if !ev.mods.shift => self.on_tab(s),
            Key::Up => self.on_move(-1, s),
            Key::Down => self.on_move(1, s),
            Key::Escape => {
                self.on_escape();
                true
            }
            _ => false,
        }
    }

    fn on_enter(&mut self, shift: bool, s: &mut State) -> bool {
        let objects = self.object_matches(&s.store);
        if self.object_tag.is_some() && !objects.is_empty() {
            let pick = objects[self.object_tag_index.min(objects.len() - 1)].clone();
            self.pick_object(pick, s);
            return true;
        }
        // Nothing to pick: the menu closes and Enter still sends.
        if self.object_tag.is_some() && !shift {
            self.object_tag = None;
        }
        let mentions = self.mention_matches();
        if self.mention.is_some() && !mentions.is_empty() {
            let pick = mentions[self.mention_index.min(mentions.len() - 1)].clone();
            self.pick_mention(pick, s);
            return true;
        }
        if self.mention.is_some() && !shift {
            self.mention = None;
        }
        false
    }

    fn on_tab(&mut self, s: &mut State) -> bool {
        let objects = self.object_matches(&s.store);
        if self.object_tag.is_some() && !objects.is_empty() {
            let pick = objects[self.object_tag_index.min(objects.len() - 1)].clone();
            self.pick_object(pick, s);
            return true;
        }
        let mentions = self.mention_matches();
        if self.mention.is_some() && !mentions.is_empty() {
            let pick = mentions[self.mention_index.min(mentions.len() - 1)].clone();
            self.pick_mention(pick, s);
            return true;
        }
        false
    }

    fn on_move(&mut self, delta: isize, s: &State) -> bool {
        let objects = self.object_matches(&s.store).len();
        if self.object_tag.is_some() && objects > 0 {
            let n = objects as isize;
            self.object_tag_index = ((self.object_tag_index as isize + delta + n) % n) as usize;
            return true;
        }
        let n = self.mention_matches().len();
        if self.mention.is_some() && n > 0 {
            self.mention_index =
                ((self.mention_index as isize + delta + n as isize) % n as isize) as usize;
            return true;
        }
        false
    }

    fn on_escape(&mut self) {
        if self.object_tag.is_some() {
            self.object_tag = None;
        } else if self.mention.is_some() && !self.mention_matches().is_empty() {
            self.mention = None;
        } else {
            self.reply_to.clear();
            self.private_recipient.clear();
        }
    }
}

/// A key that would change a read-only field's text.
fn edits_text(ev: &KeyEvent) -> bool {
    use rune::Key;
    match &ev.key {
        Key::Text(_) | Key::Backspace | Key::Delete => true,
        Key::Enter => ev.mods.shift || ev.mods.alt,
        Key::Command(c) => matches!(c.to_ascii_lowercase(), 'x' | 'v' | 'z' | 'y'),
        _ => false,
    }
}

/// A composer pill: its id, the draft text it stands for (`@Name` / `!Name`) and the label
/// it shows (icon, then the name, as GPUI's token did).
pub(crate) struct PillTag {
    pub id: String,
    pub logical: String,
    pub label: String,
}

/// The pill text: the icon, a thin space, the name.
fn pill_label(icon: &str, name: &str) -> String {
    format!("{icon}\u{2009}{name}")
}

/// The composer model for `draft`: each tagged `@Agent` / `!Object` as a pill (the first
/// tag wins where two overlap), the caret at draft byte `caret`.
pub(crate) fn compose(draft: &str, caret: usize, tags: &[PillTag]) -> TextEdit {
    let mut spans: Vec<(std::ops::Range<usize>, &PillTag)> = Vec::new();
    for tag in tags {
        let mut from = 0;
        while let Some(at) = draft[from..].find(tag.logical.as_str()) {
            let range = from + at..from + at + tag.logical.len();
            if !spans
                .iter()
                .any(|(r, _)| r.start < range.end && range.start < r.end)
            {
                spans.push((range.clone(), tag));
            }
            from = range.end;
        }
    }
    spans.sort_by_key(|(r, _)| r.start);
    let mut edit = TextEdit::new(draft);
    for (range, tag) in spans.iter().rev() {
        edit.replace_with_pill(range.clone(), tag.id.clone(), &tag.label);
    }
    // The caret in the field: each pill before it is its label's length, not its text's.
    let mut at = caret as isize;
    for (range, tag) in &spans {
        if range.end <= caret {
            at += tag.label.len() as isize - range.len() as isize;
        } else if range.start < caret {
            // Inside a pill's text: after the pill.
            at += (range.start + tag.label.len()) as isize - caret as isize;
        }
    }
    edit.set_caret(at.max(0) as usize);
    edit.clear_history();
    edit
}

/// The draft text of the composer model and its caret in that text: each pill as the text
/// it stands for.
pub(crate) fn decompose(edit: &TextEdit, tags: &[PillTag]) -> (String, usize) {
    let caret = edit.focus.min(edit.text.len());
    let mut out = String::with_capacity(edit.text.len());
    let mut copied = 0;
    let mut draft_caret = None;
    let mut pills: Vec<(std::ops::Range<usize>, String)> =
        edit.pills().map(|(r, id)| (r, id.to_string())).collect();
    pills.sort_by_key(|(r, _)| r.start);
    for (range, id) in pills {
        if range.start < copied {
            continue;
        }
        if draft_caret.is_none() && caret <= range.start {
            draft_caret = Some(out.len() + caret - copied);
        }
        out.push_str(&edit.text[copied..range.start]);
        match tags.iter().find(|t| t.id == id) {
            Some(tag) => out.push_str(&tag.logical),
            None => out.push_str(&edit.text[range.clone()]),
        }
        if draft_caret.is_none() && caret <= range.end {
            draft_caret = Some(out.len());
        }
        copied = range.end;
    }
    let caret = draft_caret.unwrap_or(out.len() + caret.saturating_sub(copied));
    out.push_str(&edit.text[copied..]);
    (out, caret)
}

/// `(?:^|\s)<sigil>([^<forbid>]*)$` over the text before the caret.
fn tag_query(before: &str, sigil: char, forbid: &[char]) -> Option<TagQuery> {
    let at = before.rfind(sigil)?;
    let query = &before[at + sigil.len_utf8()..];
    if query.contains(forbid) {
        return None;
    }
    if at > 0
        && !before[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
    {
        return None;
    }
    Some(TagQuery {
        start: at,
        query: query.to_string(),
    })
}

/// JS `\w` (no unicode flag).
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `name.replace(/\(.*?\)/g, "")`, then the first and last words' first letters, uppercase.
fn initials_of(name: &str) -> String {
    let mut stripped = String::new();
    let mut rest = name;
    while let Some(open) = rest.find('(') {
        match rest[open..].find(')') {
            Some(close) => {
                stripped.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            None => break,
        }
    }
    stripped.push_str(rest);
    let words: Vec<&str> = stripped.split_whitespace().collect();
    let first = words.first().and_then(|w| w.chars().next());
    let last = if words.len() > 1 {
        words.last().and_then(|w| w.chars().next())
    } else {
        None
    };
    first
        .into_iter()
        .chain(last)
        .collect::<String>()
        .to_uppercase()
}

fn presence_failure(error: &anyhow::Error) -> String {
    if let Some(e) = error.downcast_ref::<roostr_core::harness::HarnessError>() {
        return format!("Agent status unavailable (HTTP {}).", e.status);
    }
    let message = error.to_string();
    if message.is_empty() {
        "The paired harness is unreachable.".into()
    } else {
        message
    }
}

/// The tag a message's origin shows: that object's name, or none when the object is gone or
/// is this chat's own object (a repeat posting in its own chat would only link to the page
/// already open).
pub(crate) fn origin_label(id: &str, here: &str, store: &Store) -> Option<String> {
    if id == here {
        return None;
    }
    store.summary(id).map(|s| {
        if s.name.is_empty() {
            "Untitled".to_string()
        } else {
            s.name.clone()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failing(id: &str, surface: &str) -> AgentPresence {
        AgentPresence {
            id: id.into(),
            surface: surface.into(),
            state: "error".into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_agents_error_shows_only_where_that_agent_belongs() {
        let importer = failing("importer", "importer"); // a scheduled run: its surface is its own transcript
        let guests = vec!["importer".to_string()];
        assert!(
            presence_belongs(&importer, "task", &guests),
            "the recurring task it is on"
        );
        assert!(presence_belongs(&importer, "importer", &[]), "its own page");
        assert!(
            !presence_belongs(&importer, "other-agent", &[]),
            "another agent's chat"
        );
        assert!(
            presence_belongs(&failing("writer", "card"), "card", &[]),
            "the object its turn runs on"
        );
        assert!(!presence_belongs(&importer, "", &guests), "no object open");
    }

    fn scout() -> PillTag {
        PillTag {
            id: "agent:obj:scout".into(),
            logical: "@Scout".into(),
            label: pill_label("🤖", "Scout"),
        }
    }

    #[test]
    fn the_composer_shows_tags_as_pills_and_gives_back_the_draft() {
        let tags = [scout()];
        let draft = "hi @Scout and you";
        let edit = compose(draft, draft.len(), &tags);
        let pills: Vec<_> = edit.pills().map(|(r, id)| (r, id.to_string())).collect();
        assert_eq!(pills.len(), 1);
        assert_eq!(pills[0].1, "agent:obj:scout");
        assert_eq!(&edit.text[pills[0].0.clone()], pill_label("🤖", "Scout"));
        assert_eq!(decompose(&edit, &tags), (draft.to_string(), draft.len()));
        // The caret right after the tag maps across the pill both ways.
        let after = "hi @Scout".len();
        let edit = compose(draft, after, &tags);
        assert_eq!(edit.focus, pills[0].0.end);
        assert_eq!(decompose(&edit, &tags).1, after);
        // Before the tag nothing shifts.
        let edit = compose(draft, 2, &tags);
        assert_eq!((edit.focus, decompose(&edit, &tags).1), (2, 2));
    }

    #[test]
    fn a_tag_query_needs_a_word_start_and_stops_at_its_sigil() {
        assert_eq!(
            tag_query("ask @Sc", '@', &['@', '\n']),
            Some(TagQuery {
                start: 4,
                query: "Sc".into()
            })
        );
        assert_eq!(tag_query("mail@host", '@', &['@', '\n']), None);
        assert_eq!(tag_query("!a\nb", '!', &['!', '@', '\n']), None);
    }

    #[test]
    fn initials_skip_parentheses_and_take_first_and_last_words() {
        assert_eq!(initials_of("Grant Farwell"), "GF");
        assert_eq!(initials_of("Ann (work) Lee Smith"), "AS");
        assert_eq!(initials_of("scout"), "S");
    }
}
