//! An object's discussion (`src/lib/components/Discussion.svelte`). Human and
//! private chat stays local; exchanges send immutable mailbox envelopes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::{Avatar as AvatarRoot, AvatarFallback};
use gpui_kit::component::alert::{Alert, AlertVariant};
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::bubble::{Bubble, BubbleContent, BubbleVariant};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::empty::Empty;
use gpui_kit::component::input::{Enter, Escape, IndentInline};
use gpui_kit::component::input::{
    InlineToken, InlineTokenContext, InputContent, InputEvent, MoveDown, MoveUp, Textarea,
    TextareaState,
};
use gpui_kit::component::label::Label;
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message::{
    Message, MessageAlignment, MessageAvatar, MessageContent, MessageFooter, MessageHeader,
};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, ElementExt as _, IconName, IndexPath, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::color::bubble;
use roostr_core::io;
use roostr_core::nav::{self, Route};
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::options::TagPaint;
use roostr_core::{
    AgentEndpoint, AgentMessage, Conversation, Object, Store, StoreEvent, guest_agents, lite_ui,
    motion, theme,
};
use serde::Deserialize;

use crate::chat::{
    ChatMessage, DISCUSSION, chat_messages, is_legacy_exchange, object_chat_messages,
    reply_recipients, unique_endpoints,
};
use crate::drafts::{load_draft, save_draft};
use crate::md::{MdTheme, render_markdown};
use crate::threads::{ObjectAgentOption, endpoint_name, load_object_agents};

/// How a discussion is shown.
#[derive(Clone, Debug)]
pub struct DiscussionOptions {
    /// Always open, filling its parent (the drawer, a chat object's page).
    pub full: bool,
    /// The full-screen chat page: messages fill the screen, composer pinned.
    pub page_mode: bool,
    /// Which conversation of the object; `"__discussion__"` is the human thread
    /// (the object's one merged chat).
    pub thread_id: String,
    /// Inside the conversation drawer: messages in their own scroller, no
    /// outer margins, 14px sides. An option rather than set after `new`:
    /// the object may already be in the store, and `new` then lays out its
    /// messages - landing at the newest only in a scroller of their own.
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

/// The paired harness's live turn state for one agent (`/agent/status`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct AgentPresence {
    id: String,
    name: String,
    icon: String,
    /// "idle" | "working" | "error"
    state: String,
    surface: String,
    detail: String,
    ts: i64,
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
struct ObjectTag {
    id: String,
    name: String,
    icon: String,
    type_key: String,
}

/// An open `@…` / `!…` menu: the byte offset of its sigil and the text typed after it.
#[derive(Clone, Debug, PartialEq)]
struct TagQuery {
    start: usize,
    query: String,
}

/// A Retry button's action.
type RetryAction = Box<dyn Fn(&mut Discussion, &mut Context<Discussion>)>;

/// Where the message list and its rows were last laid out, recorded while
/// they prepaint: the kit's scroller state keeps its list private, and the
/// reader's distance from the end and a new reply's height are read from
/// these instead. Rows are cleared each render, so only rows laid out in the
/// latest frame are here.
#[derive(Default)]
struct ListLayout {
    viewport: Option<Bounds<Pixels>>,
    rows: HashMap<usize, Bounds<Pixels>>,
}

const PRESENCE_POLL: Duration = Duration::from_millis(2500);
/// How long a sent @-mention shows its agent typing without a reply.
const EXPECT_REPLY: Duration = Duration::from_secs(10 * 60);
const AGENT_TOKEN: &str = "agent:";
const OBJECT_TOKEN: &str = "object:";

pub struct Discussion {
    object_id: String,
    full: bool,
    page_mode: bool,
    thread_id: String,
    /// Inside the conversation drawer (`DiscussionOptions::embedded`).
    embedded: bool,
    /// The window the composer is in. The default tag lands after the
    /// object's agents load and is written here, whether or not the platform
    /// reports this window as the active one at that moment.
    window: AnyWindowHandle,

    object: Option<Arc<Object>>,
    messages: Rc<Vec<ChatMessage>>,
    conversation: Option<Conversation>,
    legacy: bool,
    is_exchange: bool,
    read_only: bool,
    members: Vec<AgentEndpoint>,
    exchange_title: String,

    open: bool,
    /// The message list's virtual-list state (the drawer's and chat page's
    /// own scroller): following the newest message, jump to latest, splices.
    scroller: Entity<MessageScrollerState>,
    /// Where the list and its rows were last laid out.
    layout: Rc<RefCell<ListLayout>>,
    fingerprints: Vec<u64>,
    /// Each shown origin object's tag name (`None`: not listed, no tag), as
    /// the rows were last fingerprinted.
    origins: HashMap<String, Option<String>>,
    landed_on: String,
    /// The `object:thread` whose messages are shown (an arrival needs a
    /// thread already on screen).
    arrival_key: String,
    /// Someone else's message that just arrived at the end: it fades in.
    arrived: Option<String>,
    seen_count: usize,
    focused_once: bool,
    /// A newly followed message to check once laid out (taller than the pane: show its top).
    tall_check: Option<usize>,
    seeded: bool,

    composer: Entity<TextareaState>,
    placeholder: &'static str,
    draft: String,
    reply_to: String,
    sending: bool,
    send_error: String,
    me: String,
    identity_error: String,
    private_recipient: String,
    request_reply: bool,
    retry_error: String,
    retrying: Vec<String>,
    pending_send: Option<AgentMessage>,
    /// A chat line just sent, shown at once (marked "Sending…") until the
    /// object carries the real one or the write fails.
    outgoing: Option<ChatMessage>,

    tag_options: Option<Vec<ObjectAgentOption>>,
    tag_loading: Option<Task<()>>,
    mention: Option<TagQuery>,
    mention_index: usize,
    tagged: Vec<ObjectAgentOption>,
    /// A tag was made from typed text: re-create the composer's pills.
    repill: bool,
    object_tag: Option<TagQuery>,
    object_tag_index: usize,
    tagged_objects: Vec<ObjectTag>,
    opened_rank: HashMap<String, usize>,
    /// The `@` agent menu and the `!` object menu: kit command lists whose
    /// highlight follows `mention_index` / `object_tag_index`.
    mention_menu: Entity<CommandState>,
    object_menu: Entity<CommandState>,

    presence: Vec<AgentPresence>,
    /// The agent this device just @-mentioned and the send time: shown typing
    /// until it posts here, its harness reports an error, or [`EXPECT_REPLY`]
    /// passes. Client-side only - it reads no state from the agent's computer.
    expecting: Option<(AgentPresence, i64)>,
    expect_timer: Option<Task<()>>,
    presence_error: String,
    poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Discussion {
    /// The discussion of `object_id`; loads the object into the store cache.
    pub fn new(
        object_id: String,
        options: DiscussionOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let thread_id = if options.thread_id.is_empty() {
            DISCUSSION.to_string()
        } else {
            options.thread_id
        };
        let draft = load_draft(&object_id, &thread_id, cx);
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 16)
                .submit_on_enter(true)
        });
        composer.update(cx, |state, cx| state.set_value(draft.clone(), window, cx));
        // One row: the trailing footer (empty note, typing, problem).
        let scroller = cx.new(|cx| MessageScrollerState::new(1, cx));
        let mention_menu = cx.new(|cx| CommandState::new(window, cx));
        let object_menu = cx.new(|cx| CommandState::new(window, cx));
        let subscriptions = vec![
            cx.observe(&store, |this, _, cx| this.sync_object(cx)),
            // Lite: the agent list is what this device holds; a refresh (the
            // space finished loading) lists them again.
            cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                if matches!(event, StoreEvent::Refreshed)
                    && roostr_core::lite::status(cx).is_some()
                    && this.tag_loading.is_none()
                {
                    this.tag_options = None;
                    if this.mention.is_some() {
                        this.ensure_tag_options(cx);
                    }
                }
            }),
            cx.subscribe_in(&composer, window, Self::on_composer_event),
        ];
        let mut this = Self {
            object_id,
            full: options.full,
            page_mode: options.page_mode,
            thread_id,
            embedded: options.embedded,
            window: window.window_handle(),
            object: None,
            messages: Rc::new(Vec::new()),
            conversation: None,
            legacy: false,
            is_exchange: false,
            read_only: false,
            members: Vec::new(),
            exchange_title: "Exchange".into(),
            open: false,
            scroller,
            layout: Rc::default(),
            fingerprints: Vec::new(),
            origins: HashMap::new(),
            landed_on: String::new(),
            arrival_key: String::new(),
            arrived: None,
            seen_count: 0,
            focused_once: false,
            tall_check: None,
            seeded: false,
            composer,
            placeholder: "",
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
            repill: false,
            object_tag: None,
            object_tag_index: 0,
            tagged_objects: Vec::new(),
            opened_rank: HashMap::new(),
            mention_menu,
            object_menu,
            presence: Vec::new(),
            expecting: None,
            expect_timer: None,
            presence_error: String::new(),
            poll: None,
            _subscriptions: subscriptions,
        };
        this.load_identity(cx);
        this.sync_object(cx);
        this
    }

    fn is_open(&self) -> bool {
        self.full || self.page_mode || self.open
    }

    /// Messages live in their own scroller (drawer, chat page) rather than the page's.
    fn own_scroller(&self) -> bool {
        self.embedded || self.page_mode
    }

    // ------------------------------------------------------------ object state

    /// Re-derive everything from the cached object when it changed.
    fn sync_object(&mut self, cx: &mut Context<Self>) {
        let object = Store::global(cx).read(cx).object(&self.object_id);
        let changed = match (&self.object, &object) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let guests = |o: &Option<Arc<Object>>| o.as_ref().map(|o| guest_agents(&o.fields));
            let agents_changed = guests(&self.object) != guests(&object);
            self.object = object;
            self.derive(cx);
            // Agents added or removed on the object: the @-options and the
            // composer's default tag follow at once, no reload.
            if agents_changed {
                self.tag_options = None;
                self.tag_loading = None;
                self.seed_default_agent(Vec::new(), cx);
            }
        } else {
            let store = Store::global(cx);
            let store = store.read(cx);
            let renamed = self
                .origins
                .iter()
                .any(|(id, label)| origin_label(id, &self.object_id, store) != *label);
            if renamed {
                self.sync_list(cx);
            }
        }
        cx.notify();
    }

    fn derive(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object.clone() else {
            return;
        };
        let thread = self.thread_id.as_str();
        let mut messages = if thread == DISCUSSION {
            object_chat_messages(&object)
        } else {
            chat_messages(&object, thread)
        };
        // The line being sent shows until the object has it (same author and
        // text, written no earlier than it).
        if let Some(out) = &self.outgoing {
            let landed = messages
                .iter()
                .any(|m| m.author == out.author && m.text == out.text && m.ts >= out.ts - 5_000);
            if landed {
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
        // The object's one chat (__discussion__) is a discussion even though it
        // shows envelopes: tagging sends one, an untagged line stays a chat line.
        // Only an opened exchange thread is reply-to-all.
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
        // Whether the reader was at the end, measured before the splice
        // re-estimates the new rows' heights.
        let at_end = self.at_end(cx);
        self.note_arrival(&previous, at_end);
        self.sync_list(cx);
        self.follow_new_messages(at_end, cx);
        self.update_polling(cx);
        if !self.seeded {
            self.seeded = true;
            if self.draft.is_empty() {
                self.seed_default_agent(Vec::new(), cx);
            } else {
                // A draft kept from before (reload, reopened chat): its
                // `@Agent` names become tags again once the agents are known.
                self.ensure_tag_options(cx);
                self.retag_draft(cx);
            }
        }
    }

    /// Tag every agent of this object whose `@Name` is written out in the
    /// draft (ending it or followed by a non-word character), as a pick
    /// would - for drafts restored from storage, which keep only text.
    fn retag_draft(&mut self, cx: &mut Context<Self>) {
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
            self.repill = true;
            cx.notify();
        }
    }

    fn message(&self, id: &str) -> Option<&ChatMessage> {
        self.messages.iter().find(|m| m.id == id)
    }

    /// Someone else's message arriving at the end of the thread on screen
    /// fades in. The thread's first load, history and your own sent messages
    /// don't, nor anything arriving while you read further up.
    fn note_arrival(&mut self, previous: &[ChatMessage], at_end: bool) {
        let key = format!("{}:{}", self.object_id, self.thread_id);
        let shown = std::mem::replace(&mut self.arrival_key, key.clone()) == key;
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
            self.arrived = Some(last.id.clone());
        }
    }

    /// Splice the list from the first message whose rendering inputs changed;
    /// older messages arriving in front are prepended, keeping the reader's
    /// place.
    fn sync_list(&mut self, cx: &mut Context<Self>) {
        let store = Store::global(cx);
        let store = store.read(cx);
        // A message's origin tag shows its object's name, or nothing once the
        // object is gone: a store change can redraw a row without touching it.
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
        let fingerprints: Vec<u64> = self
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
        let old = &self.fingerprints;
        let prefix = old
            .iter()
            .zip(&fingerprints)
            .take_while(|(a, b)| a == b)
            .count();
        let old_total = old.len() + 1;
        let new_total = fingerprints.len() + 1;
        let older = prefix == 0
            && !old.is_empty()
            && fingerprints.len() > old.len()
            && fingerprints.ends_with(old);
        self.scroller.update(cx, |list, cx| {
            if older {
                list.prepend(new_total - old_total, cx);
            } else {
                // Everything from the first difference on, plus the trailing presence row.
                list.splice(prefix..old_total, new_total - prefix, cx);
            }
        });
        self.fingerprints = fingerprints;
        self.origins = origins;
    }

    /// The trailing row (empty note, typing, problem) changed height.
    fn remeasure_footer(&self, cx: &mut Context<Self>) {
        let n = self.fingerprints.len();
        self.scroller.update(cx, |list, cx| {
            list.splice(n..n + 1, 1, cx);
        });
    }

    /// Within a message or so of the end: the reader is following along.
    /// Read when needed, never from the list's scroll handler: the list calls
    /// that with its state borrowed, so reading the offsets there panicked on
    /// the first wheel or touch scroll (`RefCell already mutably borrowed`),
    /// which aborted the desktop app and left the browser and iOS apps taking
    /// no input at all. (The kit scroller's own handler only defers a notify.)
    /// Away from the end, the distance is measured from where the trailing row
    /// was last laid out; not laid out at all means far up.
    fn at_end(&self, cx: &App) -> bool {
        let list = self.scroller.read(cx);
        if list.is_following_tail() || !list.is_scrolled_up() {
            return true;
        }
        let layout = self.layout.borrow();
        match (layout.viewport, layout.rows.get(&self.fingerprints.len())) {
            (Some(view), Some(footer)) => footer.bottom() - view.bottom() < px(120.),
            _ => false,
        }
    }

    /// Land at the end once per opened thread; on new messages follow when the
    /// reader was at the end, and always for a reply to what you just sent.
    fn follow_new_messages(&mut self, at_end: bool, cx: &mut Context<Self>) {
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
            self.scroller.update(cx, |list, cx| list.scroll_to_end(cx));
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
        self.scroller.update(cx, |list, cx| list.scroll_to_end(cx));
        self.tall_check = Some(count - 1);
    }

    // --------------------------------------------------------------- identity

    fn load_identity(&mut self, cx: &mut Context<Self>) {
        let backend = Store::global(cx).read(cx).backend();
        cx.spawn(async move |this, cx| {
            let result = io::spawn_async(cx, backend.settings()).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(settings) => {
                        this.me = settings.author_id;
                        this.identity_error.clear();
                    }
                    Err(error) => {
                        this.me.clear();
                        let message = error.to_string();
                        this.identity_error = if message.is_empty() {
                            "Discussion identity is unavailable while offline.".into()
                        } else {
                            message
                        };
                    }
                }
                this.sync_list(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // --------------------------------------------------------------- presence

    /// Poll the paired harness while open so the user sees an agent composing
    /// (typing dots) or failing (warning row).
    fn update_polling(&mut self, cx: &mut Context<Self>) {
        let harness = roostr_core::harness::harness(cx);
        let wanted = self.is_open() && harness.is_some() && !self.is_exchange;
        if !wanted {
            if self.poll.take().is_some()
                || !self.presence.is_empty()
                || !self.presence_error.is_empty()
            {
                self.presence.clear();
                self.presence_error.clear();
                self.remeasure_footer(cx);
            }
            return;
        }
        if self.poll.is_some() {
            return;
        }
        let Some(harness) = harness else { return };
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let h = harness.clone();
                let result =
                    io::spawn_async(cx, async move { h.get_json("/agent/status").await }).await;
                let alive = this
                    .update(cx, |this, cx| {
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
                        if presence != this.presence || error != this.presence_error {
                            this.presence = presence;
                            this.presence_error = error;
                            this.remeasure_footer(cx);
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    return;
                }
                cx.background_executor().timer(PRESENCE_POLL).await;
            }
        }));
    }

    /// The harness reports every agent; this chat shows only its own: one
    /// whose turn runs on this object, the agent whose page this is, or one on
    /// the object's Agent list (a recurring object's runs live in the agent's
    /// own transcript, so its surface is the agent, not the object).
    fn presence_here(&self) -> impl Iterator<Item = &AgentPresence> {
        let (object_id, guests) = match &self.object {
            Some(o) => (o.id.as_str(), guest_agents(&o.fields)),
            None => ("", vec![]),
        };
        self.presence
            .iter()
            .filter(move |p| presence_belongs(p, object_id, &guests))
    }

    fn agent_working(&self) -> Option<&AgentPresence> {
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

    /// Show `option`'s agent typing from now (its reply is assumed) and
    /// repaint when that assumption runs out.
    fn expect_reply(&mut self, option: &ObjectAgentOption, cx: &mut Context<Self>) {
        let id = option.endpoint.agent_id.clone();
        let icon = Store::global(cx)
            .read(cx)
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
        self.expect_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EXPECT_REPLY).await;
            let _ = this.update(cx, |this, cx| {
                this.expecting = None;
                this.remeasure_footer(cx);
                cx.notify();
            });
        }));
        self.remeasure_footer(cx);
        cx.notify();
    }

    fn agent_error(&self) -> Option<&AgentPresence> {
        let now = chrono::Utc::now().timestamp_millis();
        self.presence_here()
            .find(|p| p.state == "error" && now - p.ts < 15 * 60_000)
    }

    // ------------------------------------------------------------------ names

    fn who(&self, author: &str, store: &Store) -> String {
        if author == self.me {
            return "You".into();
        }
        if author == "scheduler" {
            return "Scheduler".into();
        }
        // An agent posts as its own object id, wherever it is replying. It is
        // absent from `summaries` (agents are hidden infrastructure), so the
        // name comes from the agent roster.
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
    fn author_links(&self, author: &str, store: &Store) -> bool {
        store.agent(author).is_some() || store.summary(author).is_some()
    }

    /// Avatar letters: a named author's initials, else the id's first two characters.
    fn initials(&self, author: &str, store: &Store) -> String {
        let name = self.who(author, store);
        if name == author.chars().take(6).collect::<String>() {
            return author.chars().take(2).collect();
        }
        let mut stripped = String::new();
        let mut depth = 0;
        let mut rest = name.as_str();
        // `name.replace(/\(.*?\)/g, "")`: drop each "(...)" group.
        while let Some(open) = rest.find('(') {
            match rest[open..].find(')') {
                Some(close) => {
                    stripped.push_str(&rest[..open]);
                    rest = &rest[open + close + 1..];
                }
                None => break,
            }
            depth += 1;
        }
        let _ = depth;
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

    fn member_name(&self, endpoint: &AgentEndpoint, store: &Store) -> String {
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

    /// Emoji avatar for an author, when it has one. An agent always has one:
    /// its own icon, else its type's (🤖), as everywhere else it is shown.
    fn avatar_emoji(&self, author: &str, store: &Store) -> String {
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

    fn reply_all(&self) -> Vec<AgentEndpoint> {
        let responder = AgentEndpoint {
            object_id: self.object_id.clone(),
            agent_id: String::new(),
        };
        self.reply_message()
            .map(|m| reply_recipients(&m, &responder))
            .unwrap_or_default()
    }

    fn audience(&self) -> Vec<AgentEndpoint> {
        let all = self.reply_all();
        if self.private_recipient.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|e| e.object_id == self.private_recipient)
                .collect()
        }
    }

    fn start_reply(
        &mut self,
        message_id: &str,
        privately: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    // ------------------------------------------------------------- composer

    fn on_composer_event(
        &mut self,
        state: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.draft = state.read(cx).value().to_string();
                save_draft(&self.object_id, &self.thread_id, &self.draft, cx);
                self.update_mention(cx);
                cx.notify();
            }
            InputEvent::PressEnter { shift: false, .. } => self.send(window, cx),
            InputEvent::Blur => {
                self.mention = None;
                self.object_tag = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// Replace the composer's text, re-creating the pills of live tags.
    fn set_draft(
        &mut self,
        text: String,
        caret: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = self.tokenized(&text);
        self.composer.update(cx, |s, cx| {
            s.set_value(content, window, cx);
            if let Some(caret) = caret {
                s.set_selected_range(caret..caret, cx);
            }
        });
        self.draft = text;
        save_draft(&self.object_id, &self.thread_id, &self.draft, cx);
    }

    /// The draft with each tagged `@Agent` / `!Object` as an inline pill.
    fn tokenized(&self, text: &str) -> InputContent {
        let mut spans: Vec<(std::ops::Range<usize>, InlineToken)> = Vec::new();
        let agents: Vec<(String, String)> = self
            .tagged
            .iter()
            .map(|o| {
                (
                    format!("@{}", o.agent_name),
                    format!("{AGENT_TOKEN}{}", o.key()),
                )
            })
            .collect();
        let objects: Vec<(String, String)> = self
            .tagged_objects
            .iter()
            .map(|o| (format!("!{}", o.name), format!("{OBJECT_TOKEN}{}", o.id)))
            .collect();
        for (label, id) in agents.iter().chain(&objects) {
            let mut from = 0;
            while let Some(at) = text[from..].find(label.as_str()) {
                let range = from + at..from + at + label.len();
                if !spans
                    .iter()
                    .any(|(r, _)| r.start < range.end && range.start < r.end)
                {
                    spans.push((range.clone(), InlineToken::new(id.clone(), label.clone())));
                }
                from = range.end;
            }
        }
        spans.sort_by_key(|(r, _)| r.start);
        let mut content = InputContent::new(text.to_string());
        for (range, token) in spans {
            if let Ok(next) = content.clone().with_token(range, token) {
                content = next;
            }
        }
        content
    }

    fn caret(&self, cx: &App) -> usize {
        self.composer.read(cx).cursor()
    }

    /// Open/refresh/close a menu from the caret: whichever `@…` or `!…` token
    /// ends at the caret and started last wins.
    fn update_mention(&mut self, cx: &mut Context<Self>) {
        // A tag deleted from the text untags immediately, so the menu offers that agent again.
        let draft = self.draft.clone();
        self.tagged
            .retain(|o| draft.contains(&format!("@{}", o.agent_name)));
        self.tagged_objects
            .retain(|t| draft.contains(&format!("!{}", t.name)));
        let caret = self.caret(cx).min(draft.len());
        let before = &draft[..caret];
        let hit = tag_query(before, '@', &['@', '\n']);
        let obj_hit = tag_query(before, '!', &['!', '@', '\n']);
        if let Some(obj) = &obj_hit
            && hit.as_ref().is_none_or(|h| obj.query.len() < h.query.len())
        {
            self.mention = None;
            self.update_object_tag(obj.clone(), cx);
            return;
        }
        self.object_tag = None;
        let Some(hit) = hit else {
            self.mention = None;
            return;
        };
        // A completed tag is only `@Name ` in the draft; once the caret moves past
        // it the token still matches, so close explicitly.
        if self
            .tagged
            .iter()
            .any(|o| hit.query.starts_with(&format!("{} ", o.agent_name)))
        {
            self.mention = None;
            return;
        }
        // `@Name ` typed out in full (not picked from the menu) tags the agent
        // as a pick would - once the agents are known (re-run when they load).
        // Capitals don't matter: the text takes the agent's own spelling.
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
                }
                self.tagged.push(option);
                self.repill = true;
                self.mention = None;
                cx.notify();
                return;
            }
        }
        if self.mention.as_ref().is_none_or(|m| m.query != hit.query) {
            self.mention_index = 0;
        }
        self.mention = Some(hit);
        self.ensure_tag_options(cx);
    }

    fn ensure_tag_options(&mut self, cx: &mut Context<Self>) {
        if self.tag_options.is_some() || self.tag_loading.is_some() {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        let task = load_object_agents(&object, cx);
        self.tag_loading = Some(cx.spawn(async move |this, cx| {
            let options = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.tag_options = Some(options);
                this.tag_loading = None;
                this.retag_draft(cx);
                this.update_mention(cx);
                cx.notify();
            });
        }));
    }

    /// Lite: the agents listed are those this device holds; offer the space.
    fn lite_agents_notice(cx: &App) -> Option<AnyElement> {
        let space = nav::Navigator::global(cx).read(cx).space().to_string();
        lite_ui::space_notice("agents", &space, cx)
            .map(|notice| div().px(px(8.)).child(notice).into_any_element())
    }

    fn mention_matches(&self) -> Vec<ObjectAgentOption> {
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

    fn pick_mention(
        &mut self,
        option: ObjectAgentOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.mention.take() else {
            return;
        };
        let caret = self.caret(cx).max(at.start).min(self.draft.len());
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
        self.set_draft(text, Some(pos), window, cx);
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn update_object_tag(&mut self, hit: TagQuery, cx: &mut Context<Self>) {
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
            self.opened_rank = roostr_core::recent::opened_ranks(cx);
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

    fn object_matches(&self, cx: &App) -> Vec<ObjectTag> {
        let Some(tag) = &self.object_tag else {
            return Vec::new();
        };
        let Some(object) = &self.object else {
            return Vec::new();
        };
        let store = Store::global(cx);
        let store = store.read(cx);
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

    /// Move the object menu's highlight; the menu scrolls it into view.
    fn move_object_tag(&mut self, delta: isize, count: usize) {
        let n = count as isize;
        self.object_tag_index = ((self.object_tag_index as isize + delta + n) % n) as usize;
    }

    fn pick_object(&mut self, option: ObjectTag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.object_tag.take() else {
            return;
        };
        let caret = self.caret(cx).max(at.start).min(self.draft.len());
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
        self.set_draft(text, Some(pos), window, cx);
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
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
                {
                    let tag = self
                        .tagged_objects
                        .iter()
                        .find(|t| t.name == *name)
                        .expect("tagged name");
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

    /// The composer opens already addressed: `@Name `, tagged, caret after.
    /// Name = the agent last @-tagged in this chat (by a person, not an
    /// agent), else on an agent's own page that agent, else the object's only
    /// agent; otherwise it starts empty. Only ever fills an empty draft.
    /// `just_sent` is the tags of the message just posted.
    fn seed_default_agent(&mut self, just_sent: Vec<ObjectAgentOption>, cx: &mut Context<Self>) {
        if !self.draft.is_empty() || self.read_only || self.is_exchange {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        let loaded = self.tag_options.is_some();
        if !loaded {
            let task = load_object_agents(&object, cx);
            cx.spawn(async move |this, cx| {
                let options = task.await.unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    if this.tag_options.is_none() {
                        this.tag_options = Some(options);
                    }
                    this.finish_seed(just_sent, cx);
                });
            })
            .detach();
        } else {
            self.finish_seed(just_sent, cx);
        }
    }

    fn finish_seed(&mut self, just_sent: Vec<ObjectAgentOption>, cx: &mut Context<Self>) {
        let options = self.tag_options.clone().unwrap_or_default();
        if !self.draft.is_empty() || options.is_empty() {
            return;
        }
        let Some(object) = self.object.clone() else {
            return;
        };
        let store = Store::global(cx);
        let last_tagged = {
            let store = store.read(cx);
            let mut found = None;
            for m in self.messages.iter().rev() {
                if store.agent(&m.author).is_some() {
                    continue;
                }
                let mut best: Option<(&ObjectAgentOption, usize)> = None;
                for option in &options {
                    if let Some(at) = m.text.rfind(&format!("@{}", option.agent_name))
                        && best.is_none_or(|(_, b)| at > b)
                    {
                        best = Some((option, at));
                    }
                }
                if let Some((option, _)) = best {
                    found = Some(option.clone());
                    break;
                }
            }
            found
        };
        let assigned = if object.type_key == "agent" {
            vec![object.id.clone()]
        } else {
            guest_agents(&object.fields)
        };
        let sole = if assigned.len() == 1 {
            options
                .iter()
                .find(|o| o.endpoint.agent_id == assigned[0])
                .cloned()
        } else {
            None
        };
        let Some(pick) = just_sent.last().cloned().or(last_tagged).or(sole) else {
            return;
        };
        let text = format!("@{} ", pick.agent_name);
        self.tagged = vec![pick];
        let caret = text.len();
        let window = self.window;
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.draft.is_empty() {
                        this.set_draft(text, Some(caret), window, cx);
                        cx.notify();
                    }
                });
            });
        });
    }

    /// Tags whose @Name survived editing and are present in this text, plus
    /// any agent of this object whose `@Name` was typed out in full (ending
    /// the text or followed by a non-word character) without picking it.
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

    fn clear_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reply_to.clear();
        self.tagged.clear();
        self.tagged_objects.clear();
        self.set_draft(String::new(), None, window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.composer.update(cx, |s, cx| s.set_readonly(true, cx));
        cx.notify();
        // The object's one chat is __discussion__: a line that @-mentions a
        // guest agent is the harness's wake signal, and the agent replies in
        // the same thread - no envelope. Envelopes are for an opened exchange
        // thread (reply-to-all), where they carry the audience.
        if self.is_exchange {
            let Some(reply_message) = self.reply_message() else {
                self.finish_send(
                    Err(anyhow::anyhow!("Reload this exchange before replying.")),
                    window,
                    cx,
                );
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
            let task = roostr_core::api::mailbox::send(&message, cx);
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |this, window, cx| match result {
                    Ok(sent) => {
                        this.pending_send = None;
                        this.private_recipient.clear();
                        this.clear_composer(window, cx);
                        this.sending = false;
                        this.composer.update(cx, |s, cx| s.set_readonly(false, cx));
                        let thread = sent
                            .get("threadId")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if !thread.is_empty() && thread != this.thread_id {
                            this.switch_thread(thread, window, cx);
                        }
                        cx.notify();
                    }
                    Err(error) => this.finish_send(Err(error), window, cx),
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
        let task = roostr_core::api::chat::post(&self.object_id, &text, &reply, &thread, cx);
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
        self.clear_composer(window, cx);
        self.composer.update(cx, |s, cx| s.set_readonly(false, cx));
        self.derive(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(_) => {
                    this.finish_send(Ok(()), window, cx);
                    // The agent tagged last answers: show it typing until it does.
                    if let Some(agent) = mentions.last() {
                        this.expect_reply(agent, cx);
                    }
                    this.seed_default_agent(mentions, cx);
                }
                Err(error) => {
                    // Not sent: the line leaves the chat and goes back into
                    // the composer (unless something new was typed there).
                    this.outgoing = None;
                    if this.draft.is_empty() {
                        this.tagged = tagged;
                        this.set_draft(draft, None, window, cx);
                    }
                    this.derive(cx);
                    this.finish_send(Err(error), window, cx);
                }
            });
        })
        .detach();
    }

    fn finish_send(
        &mut self,
        result: anyhow::Result<()>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = result {
            self.send_error = error.to_string();
        }
        self.sending = false;
        self.composer.update(cx, |s, cx| s.set_readonly(false, cx));
        cx.notify();
    }

    /// A private reply opened its own exchange: show that thread.
    fn switch_thread(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        self.thread_id = thread;
        self.landed_on.clear();
        self.fingerprints.clear();
        self.scroller.update(cx, |list, cx| list.reset(1, cx));
        let draft = load_draft(&self.object_id, &self.thread_id, cx);
        self.tagged.clear();
        self.tagged_objects.clear();
        self.set_draft(draft, None, window, cx);
        self.retag_draft(cx);
        self.derive(cx);
    }

    fn retry(
        &mut self,
        message_id: String,
        stage: &'static str,
        recipient: String,
        cx: &mut Context<Self>,
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
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.retry_error = error.to_string();
                }
                this.retrying.retain(|k| k != &key);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------- keyboard

    fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        let objects = self.object_matches(cx);
        if self.object_tag.is_some() && !objects.is_empty() {
            let pick = objects[self.object_tag_index.min(objects.len() - 1)].clone();
            self.pick_object(pick, window, cx);
            cx.stop_propagation();
            return;
        }
        // Nothing to pick: the menu closes and Enter still sends.
        if self.object_tag.is_some() && !action.shift {
            self.object_tag = None;
        }
        let mentions = self.mention_matches();
        if self.mention.is_some() && !mentions.is_empty() {
            let pick = mentions[self.mention_index.min(mentions.len() - 1)].clone();
            self.pick_mention(pick, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.mention.is_some() && !action.shift {
            self.mention = None;
        }
        cx.notify();
    }

    fn on_tab(&mut self, _: &IndentInline, window: &mut Window, cx: &mut Context<Self>) {
        let objects = self.object_matches(cx);
        if self.object_tag.is_some() && !objects.is_empty() {
            let pick = objects[self.object_tag_index.min(objects.len() - 1)].clone();
            self.pick_object(pick, window, cx);
            cx.stop_propagation();
            return;
        }
        let mentions = self.mention_matches();
        if self.mention.is_some() && !mentions.is_empty() {
            let pick = mentions[self.mention_index.min(mentions.len() - 1)].clone();
            self.pick_mention(pick, window, cx);
            cx.stop_propagation();
        }
    }

    fn on_move(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let objects = self.object_matches(cx);
        if self.object_tag.is_some() && !objects.is_empty() {
            self.move_object_tag(delta, objects.len());
            cx.notify();
            return true;
        }
        let n = self.mention_matches().len();
        if self.mention.is_some() && n > 0 {
            self.mention_index =
                ((self.mention_index as isize + delta + n as isize) % n as isize) as usize;
            cx.notify();
            return true;
        }
        false
    }

    fn on_escape(&mut self, _: &Escape, _window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.object_tag.is_some() {
            self.object_tag = None;
        } else if self.mention.is_some() && !self.mention_matches().is_empty() {
            self.mention = None;
        } else {
            self.reply_to.clear();
            self.private_recipient.clear();
        }
        cx.notify();
    }
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
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
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

/// Stable avatar hue from the author id.
fn hue(author: &str) -> u32 {
    author
        .encode_utf16()
        .fold(0u32, |h, c| (h * 31 + c as u32) % 360)
}

fn avatar_color(author: &str) -> Hsla {
    hsla(hue(author) as f32 / 360., 0.45, 0.35, 1.)
}

fn when(ts: i64) -> String {
    let Some(d) = chrono::DateTime::from_timestamp_millis(ts) else {
        return String::new();
    };
    let d = d.with_timezone(&chrono::Local);
    let time = d.format("%I:%M %p").to_string();
    if d.date_naive() == chrono::Local::now().date_naive() {
        time
    } else {
        format!("{}, {time}", d.format("%b %-d"))
    }
}

/// The tag a message's origin shows: that object's name, or none when the
/// object is gone or is this chat's own object (a repeat posting in its own
/// chat would only link to the page already open).
fn origin_label(id: &str, here: &str, store: &Store) -> Option<String> {
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

// ===================================================================== render

/// A newly arrived message: opacity 0 → 1 while it rises 6px into place
/// over 160ms.
fn arrival(id: &str, window: &mut Window, cx: &mut App) -> motion::Frame {
    let arrive = motion::Motion::new(motion::Target::default())
        .initial(motion::Target::default().y(6.).opacity(0.))
        .transition(motion::Tween::ease(160).transition());
    let id = SharedString::from(format!("msg-arrive-{id}"));
    motion::use_motion(id, window, cx, motion::MotionState::default)
        .update(cx, |state, cx| state.frame(arrive, window, cx))
}

/// How a round avatar is drawn in one place: message rows and the typing
/// row, or the mention menu.
#[derive(Clone, Copy)]
struct AvatarLook {
    size: f32,
    /// The initials' text size.
    letters: f32,
    /// An emoji's text size.
    emoji: f32,
    /// An emoji sits on a light disc (the mention menu shows it bare).
    disc: bool,
    /// Initials in the monospace face.
    mono: bool,
}

/// A max width no bubble surface reaches: overrides the kit surface's 100%
/// cap (see `render_message`), leaving the bubble's own cap as the real limit.
const UNCAPPED: f32 = 100_000.;

const ROW_AVATAR: AvatarLook = AvatarLook {
    size: 28.,
    letters: 10.,
    emoji: 17.,
    disc: true,
    mono: true,
};

const MENU_AVATAR: AvatarLook = AvatarLook {
    size: 20.,
    letters: 9.,
    emoji: 13.,
    disc: false,
    mono: false,
};

/// An avatar's corner: a squircle-like rounded square (40% of the side, the
/// app-icon look - at 30% a solid initials tile still read as a square).
/// GPUI has no continuous corners, so this is the nearest shape that also
/// clips a picture.
fn avatar_radius(size: f32) -> Pixels {
    px(size * 0.4)
}

/// A kit avatar: an image icon (a web address or `data:image/` URL, as
/// `emoji_or_image_icon` reads them) is its picture; else the emoji, else the
/// initials on the author's stable hue. The kit avatar only draws an icon or
/// its own initials and palette without a picture, so those two go in the
/// fallback slot of the kit's avatar root. Every layer carries the corner:
/// GPUI clips children to rectangles, so a fill inside a rounded root would
/// still paint square.
fn avatar(icon: &str, initials: String, hue_of: &str, look: AvatarLook) -> AnyElement {
    let size = px(look.size);
    let radius = avatar_radius(look.size);
    if is_image_icon(icon) {
        let source: Option<ImageSource> = if icon.starts_with("data:") {
            roostr_core::ui::data_url_image(icon).map(ImageSource::Image)
        } else {
            Some(SharedUri::from(icon.to_string()).into())
        };
        return match source {
            Some(source) => Avatar::new()
                .src(source)
                .with_size(size)
                .rounded(radius)
                .border_0()
                .bg(theme::hl_light())
                .into_any_element(),
            None => div().size(size).flex_none().into_any_element(),
        };
    }
    let fallback = AvatarFallback::new()
        .size_full()
        .rounded(radius)
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::white());
    let fallback = if icon.is_empty() {
        fallback
            .bg(avatar_color(hue_of))
            .text_size(px(look.letters))
            .when(look.mono, |f| f.font_family(theme::fonts::MONO))
            .child(initials)
    } else {
        fallback
            .when(look.disc, |f| f.bg(theme::hl_light()))
            .text_size(px(look.emoji))
            .child(icon.to_string())
    };
    AvatarRoot::new()
        .size(size)
        .flex_none()
        .rounded(radius)
        .overflow_hidden()
        .fallback(fallback)
        .into_any_element()
}

/// Let a kit part's children paint past its box (a bubble's touch selection
/// handles, an avatar's hover ring), where the part clips them by default.
fn overflow_visible<S: Styled>(mut el: S) -> S {
    let style = el.style();
    style.overflow.x = Some(Overflow::Visible);
    style.overflow.y = Some(Overflow::Visible);
    el
}

/// An agent in the `@` menu: avatar, `@Name`, and the object's own name.
fn mention_row(option: &ObjectAgentOption) -> Div {
    let initials = option
        .agent_name
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase();
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .child(avatar(
            &option.icon,
            initials,
            &option.endpoint.agent_id,
            MENU_AVATAR,
        ))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(format!("@{}", option.agent_name)),
        )
        .when(option.name != option.agent_name, |d| {
            d.child(
                div()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(option.name.clone()),
            )
        })
}

/// An object in the `!` menu: icon, name, and `opened` when recently opened.
fn object_row(icon: &str, name: &str, opened: bool) -> Div {
    let icon_el = div()
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    let icon_el = if is_image_icon(icon) {
        icon_el.child(roostr_core::ui::emoji_or_image_icon(icon, px(18.)))
    } else {
        icon_el.text_size(px(13.)).child(icon.to_string())
    };
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(theme::fg())
        .child(icon_el)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_weight(FontWeight::MEDIUM)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(name.to_string()),
        )
        .when(opened, |d| {
            d.child(
                Tag::secondary()
                    .small()
                    .flex_none()
                    .ml_auto()
                    .child("opened"),
            )
        })
}

/// The `!` menu's search row: what is typed after `!` in the composer, or
/// the hint before anything is.
fn object_search_row(q: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(8.))
        .border_b_1()
        .border_color(theme::border())
        .flex_none()
        .text_color(if q.is_empty() {
            theme::muted()
        } else {
            theme::fg()
        })
        .child(
            svg()
                .path("icons/search.svg")
                .size(px(14.))
                .flex_none()
                .text_color(theme::muted()),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(if q.is_empty() {
                    "Search objects - recently opened first".to_string()
                } else {
                    q.to_string()
                }),
        )
}

impl Discussion {
    /// A row's avatar; an author that is an object opens it on click, with a
    /// hover ring and an "Open …" tooltip.
    fn render_avatar(&self, id: ElementId, author: &str, store: &Store) -> AnyElement {
        let face = avatar(
            &self.avatar_emoji(author, store),
            self.initials(author, store),
            author,
            ROW_AVATAR,
        );
        if !self.author_links(author, store) {
            return face;
        }
        let target = author.to_string();
        let title = format!("Open {}", self.who(author, store));
        div()
            .id(id)
            .flex_none()
            .size(px(ROW_AVATAR.size))
            .rounded(avatar_radius(ROW_AVATAR.size))
            .cursor_pointer()
            .hover(|s| {
                s.shadow(vec![BoxShadow {
                    color: theme::accent(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(move |_, _, cx| nav::navigate(Route::Object(target.clone()), cx))
            .child(face)
            .into_any_element()
    }

    fn render_message(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = self.messages[ix].clone();
        // The actions fade in while the message is hovered
        // (`.msg .actions { transition: opacity 0.1s }`).
        let reveal = (!self.read_only).then(|| {
            let id = SharedString::from(format!("msg-actions-{}", m.id));
            motion::hover(id, motion::HOVER_REVEAL, window, cx)
        });
        // The kit row puts 0.5rem between avatar and column and starts the
        // footer 2.5rem in; Roostr's bubble column is 38px in (28px avatar,
        // 10px gap) and its status text 40px.
        let rem = window.rem_size();
        let column_inset = (px(10.) - rem * 0.5).max(px(0.));
        let status_inset = (px(40.) - rem * 2.5).max(px(0.));
        let store_entity = Store::global(cx);
        let store = store_entity.read(cx);
        let own = m.author == self.me;
        let color = if own {
            store.chat_color.clone()
        } else {
            store
                .agent(&m.author)
                .map(|a| a.chat_color.clone())
                .unwrap_or_default()
        };
        let tinted = !color.is_empty();
        let colors = bubble(&color);
        let mid = SharedString::from(m.id.clone());
        // Your messages on the right, everyone else's on the left.
        let align = if own {
            MessageAlignment::End
        } else {
            MessageAlignment::Start
        };

        // Header: author and time.
        let mut header = MessageHeader::new()
            .content_inset(false)
            .flex_col()
            .gap_0()
            .map(|h| if own { h.items_end() } else { h.items_start() })
            .font_weight(FontWeight::NORMAL)
            .line_height(phi())
            .text_color(theme::fg());
        let mut meta = div().flex().items_baseline().gap(px(8.));
        if own {
            meta = meta.justify_end();
        } else {
            let name = self.who(&m.author, store);
            meta = meta.child(if self.author_links(&m.author, store) {
                let target = m.author.clone();
                Link::new(("author", ix))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::fg())
                    .text_decoration_color(theme::fg().opacity(0.4))
                    .on_click(move |_, _, cx| nav::navigate(Route::Object(target.clone()), cx))
                    .child(name)
                    .into_any_element()
            } else {
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name)
                    .into_any_element()
            });
        }
        let sending = self.outgoing.as_ref().is_some_and(|o| o.id == m.id);
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(theme::muted())
                .child(if sending { "Sending…".to_string() } else { when(m.ts) }),
        );
        header = header.child(meta);

        // The bubble: your chat colour, an agent's own colour, else neutral;
        // the corner by the sender is the sharp one.
        let md_theme = MdTheme {
            ink: if tinted { colors.ink } else { theme::fg() },
            tint: if tinted { Some(colors.tint) } else { None },
            mention_names: store
                .agents
                .iter()
                .filter(|a| !a.name.is_empty())
                .map(|a| (a.name.clone(), a.icon.clone()))
                .collect(),
        };
        // Each message's lines read after the previous message's. The object
        // it came from leads the body as that object's tag; a deleted one, none.
        let origin = origin_label(&m.origin, &self.object_id, store);
        let text = render_markdown(
            ("md", ix),
            &m.text,
            origin.as_deref().map(|name| (m.origin.as_str(), name)),
            &md_theme,
            (ix as u64) << 24,
            cx,
        );
        let variant = if !tinted {
            BubbleVariant::Muted
        } else if own {
            BubbleVariant::Filled
        } else {
            BubbleVariant::Tinted
        };
        let surface = BubbleContent::new()
            .rounded(px(16.))
            .map(|d| {
                if own {
                    d.rounded_br(px(5.))
                } else {
                    d.rounded_bl(px(5.))
                }
            })
            .border_0()
            .px(px(12.))
            .py(px(7.))
            .text_size(px(14.))
            .line_height(relative(1.45))
            .bg(if tinted { colors.bg } else { theme::hover() })
            .text_color(if tinted { colors.ink } else { theme::fg() })
            // No percentage cap on the surface. The kit gives it 100% of the
            // bubble, which is sized by its text: while the list measured a
            // row that resolved one way and when it drew another, so the text
            // was measured on fewer lines than it drew and ran over the next
            // messages. A fixed cap (none, really) overrides it; the bubble's
            // own cap below is the real limit, and min_w_0 wraps the text to it.
            .min_w_0()
            .max_w(px(UNCAPPED))
            .map(overflow_visible)
            .child(
                div()
                    .debug_selector({
                        let mid = mid.clone();
                        move || format!("msg-text-{mid}")
                    })
                    .min_w_0()
                    .child(text),
            );
        // Not `.alignment(...)`: the kit aligns a bubble with auto side
        // margins, which size it to its one-line text width - measured and
        // drawn differently (bubbles over the next messages) and wider than
        // the chat. The message content aligns it to its side instead.
        let bubble_el = Bubble::new()
            .with_variant(variant)
            .min_w_0()
            // 100% of the message content, which fills the column (78% of the
            // full-width row): measured and drawn the same.
            .max_w_full()
            .content(surface);

        // Delivery / processing status: quiet when all is well.
        let mut status_line = None;
        if let Some(entry) = &m.mailbox {
            let failed_delivery = if entry.outgoing {
                entry.deliveries.iter().find(|d| d.status == "failed")
            } else {
                None
            };
            let pending_delivery =
                entry.outgoing && entry.deliveries.iter().any(|d| d.status == "pending");
            let local_work = entry.incoming
                && (!entry.message.operation.is_empty()
                    || entry
                        .message
                        .recipients
                        .iter()
                        .any(|r| r.object_id == self.object_id && !r.agent_id.is_empty()));
            let waiting_for = entry
                .message
                .recipients
                .iter()
                .filter(|r| !r.agent_id.is_empty())
                .map(|r| {
                    store
                        .agent(&r.agent_id)
                        .map(|a| a.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "the agent".into())
                })
                .collect::<Vec<_>>()
                .join(", ");
            let suffix = |e: &str| {
                if e.is_empty() {
                    String::new()
                } else {
                    format!(" · {e}")
                }
            };
            // `.status { display: flex; flex-wrap: wrap }`: the sentence wraps within
            // the bubble column and Retry follows it.
            let status = |failed: bool, text: String| {
                div()
                    .max_w_full()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap_x(px(6.))
                    .gap_y(px(3.))
                    .pt(px(2.))
                    // Under the bubble column, 2px in.
                    .map(|d| {
                        if own {
                            d.px(px(2.))
                        } else {
                            d.pl(status_inset).pr(px(2.))
                        }
                    })
                    .text_size(px(11.))
                    .line_height(relative(1.5))
                    .text_color(if failed {
                        theme::orange()
                    } else {
                        theme::muted()
                    })
                    .child(div().min_w_0().max_w_full().child(text))
            };
            let retry_button = |id: ElementId, disabled: bool, on: RetryAction| {
                Button::new(id)
                    .outline()
                    .xsmall()
                    .label("Retry")
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            on(this, cx)
                        }
                    }))
            };
            status_line = if let Some(failed) = failed_delivery {
                let recipient = failed.recipient.object_id.clone();
                let key = format!("{}:delivery:{}", m.id, recipient);
                let id = m.id.clone();
                Some(
                    status(
                        true,
                        format!(
                            "Not delivered to {}{}",
                            self.member_name(&failed.recipient, store),
                            suffix(&failed.error)
                        ),
                    )
                    .child(retry_button(
                        ("retry-d", ix).into(),
                        self.retrying.contains(&key),
                        Box::new(move |this, cx| {
                            this.retry(id.clone(), "delivery", recipient.clone(), cx)
                        }),
                    )),
                )
            } else if local_work && entry.processing.status == "failed" {
                let key = format!("{}:processing:", m.id);
                let id = m.id.clone();
                Some(
                    status(true, format!("Failed{}", suffix(&entry.processing.error))).child(
                        retry_button(
                            ("retry-p", ix).into(),
                            self.retrying.contains(&key),
                            Box::new(move |this, cx| {
                                this.retry(id.clone(), "processing", String::new(), cx)
                            }),
                        ),
                    ),
                )
            } else if local_work && entry.processing.status == "held" {
                Some(status(
                    true,
                    format!("On hold{}", suffix(&entry.processing.error)),
                ))
            } else if local_work && entry.processing.status == "awaiting_approval" {
                Some(status(
                    false,
                    "Waiting for approval on the serving computer".into(),
                ))
            } else if pending_delivery {
                Some(status(false, "Sending…".into()))
            } else if local_work
                && (entry.processing.status == "pending" || entry.processing.status == "processing")
            {
                Some(status(false, format!("Waiting for {waiting_for}…")))
            } else {
                None
            };
        }

        // Hover actions, beside the bubble on the side away from the sender.
        let actions = reveal.as_ref().map(|(opacity, _)| {
            let id2 = m.id.clone();
            let id3 = m.id.clone();
            let exchange = self.is_exchange;
            div()
                .absolute()
                .top_0()
                .map(|d| {
                    if own {
                        d.right(relative(1.)).pr(px(10.))
                    } else {
                        d.left(relative(1.)).pl(px(10.))
                    }
                })
                .flex()
                .gap(px(2.))
                .opacity(*opacity)
                .child(
                    Button::new(("reply", ix))
                        .ghost()
                        .xsmall()
                        .label("↩")
                        .tooltip(if exchange { "Reply to all" } else { "Reply" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_reply(&id2, false, window, cx)
                        })),
                )
                .when(
                    m.mailbox
                        .as_ref()
                        .is_some_and(|e| e.message.sender.object_id != self.object_id),
                    |d| {
                        d.child(
                            Button::new(("private", ix))
                                .ghost()
                                .xsmall()
                                .label("Private")
                                .text_color(theme::muted())
                                .tooltip("Reply privately in a separate exchange")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_reply(&id3, true, window, cx)
                                })),
                        )
                    },
                )
        });

        // The bubble column is at most 78% of the row. It stays full-width
        // (the kit's default) under that cap: a column sized by its content
        // made every percentage inside it count as nothing while the list
        // measured the row and as the real width when it drew, so text was
        // measured on fewer lines than it drew - bubbles over the next ones.
        // The bubble still hugs its text: the kit aligns it to its side.
        let mut column = StyleRefinement::default()
            .w_full()
            .max_w(relative(0.78))
            .gap_0();
        if !own {
            column = column.pl(column_inset);
        }
        let mut message = Message::new()
            .alignment(align)
            .gap_0()
            .with_stack_style(column)
            .header(header)
            .content(
                MessageContent::new()
                    .w_full()
                    .relative()
                    // Aligns the bubble to its side (see bubble_el: not the kit's
                    // own alignment, which `.bubble(...)` would pass down to it).
                    .map(|c| if own { c.items_end() } else { c.items_start() })
                    .child(bubble_el)
                    .children(actions),
            );
        if !own {
            message = message.avatar_slot(
                MessageAvatar::new()
                    .min_w(px(ROW_AVATAR.size))
                    .size(px(ROW_AVATAR.size))
                    .bg(transparent_black())
                    .map(overflow_visible)
                    .child(self.render_avatar(("avatar", ix).into(), &m.author, store)),
            );
        }
        if let Some(status) = status_line {
            message = message.footer(
                MessageFooter::new()
                    .content_inset(false)
                    .max_w(relative(0.78))
                    .font_weight(FontWeight::NORMAL)
                    .child(status),
            );
        }

        div()
            .id(ElementId::Name(format!("msg-{mid}").into()))
            .debug_selector({
                let mid = mid.clone();
                move || format!("msg-{mid}")
            })
            .when_some(reveal.map(|(_, hover)| hover), |d, hover| {
                d.on_hover(move |hovered, _, cx| motion::HoverMotion::set(&hover, *hovered, cx))
            })
            .relative()
            .w_full()
            .child(message)
            .into_any_element()
    }

    /// The trailing row: empty note, the agent composing, or its problem.
    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut footer = div().flex().flex_col();
        if self.messages.is_empty() {
            // A muted one-liner, not the kit's centred empty-state card.
            footer = footer.child(
                Empty::new()
                    .flex_none()
                    .items_start()
                    .justify_start()
                    .gap_0()
                    .p_0()
                    .my(px(13.))
                    .rounded_none()
                    .text_left()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child("No messages yet."),
            );
        }
        let working = self.agent_working().cloned();
        if let Some(agent) = &working {
            footer = footer.child(
                div()
                    .id("working")
                    .debug_selector(|| "agent-typing".into())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .tooltip(|window, cx| {
                        Tooltip::new("The agent is composing a reply").build(window, cx)
                    })
                    .child(avatar(
                        &agent.icon,
                        agent.name.chars().take(2).collect(),
                        &agent.id,
                        ROW_AVATAR,
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(agent.name.clone()),
                    )
                    .child(typing_dots()),
            );
        }
        if working.is_none()
            && let Some(agent) = self.agent_error()
        {
            let hint = {
                let d = agent.detail.to_lowercase();
                if ["auth", "key", "401", "credential"]
                    .iter()
                    .any(|k| d.contains(k))
                {
                    "Fix in Settings → Agent."
                } else {
                    ""
                }
            };
            footer = footer.child(
                Alert::warning(
                    "agent-problem",
                    format!(
                        "{} hit a problem: {}{}",
                        agent.name,
                        agent.detail,
                        if hint.is_empty() {
                            String::new()
                        } else {
                            format!(" — {hint}")
                        }
                    ),
                )
                .xsmall()
                .mt(px(2.))
                .mb(px(6.))
                .text_size(px(12.))
                .line_height(relative(1.4))
                .text_color(theme::orange()),
            );
        }
        let _ = cx;
        footer.into_any_element()
    }

    fn render_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = self.messages.len();
        let top = if ix == 0 { px(4.) } else { px(14.) };
        // Only records the bounds: the list holds its state borrowed while it
        // lays rows out, so nothing here may read or change the scroller.
        let layout = self.layout.clone();
        let laid_out = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut App| {
            layout.borrow_mut().rows.insert(ix, bounds);
        };
        if ix < n {
            let item = div()
                .relative()
                .pt(top)
                .child(self.render_message(ix, window, cx))
                .on_prepaint(laid_out);
            if self.arrived.as_deref() != Some(self.messages[ix].id.as_str()) {
                return item.into_any_element();
            }
            let frame = arrival(&self.messages[ix].id, window, cx);
            if !frame.is_animating() {
                self.arrived = None;
            }
            frame.apply(item).into_any_element()
        } else {
            let footer = self.render_footer(cx);
            let has = n == 0 || self.agent_working().is_some() || self.agent_error().is_some();
            div()
                .relative()
                .when(has, |d| d.pt(top))
                .pb(px(4.))
                .child(footer)
                .on_prepaint(laid_out)
                .into_any_element()
        }
    }

    /// Highlight row `index` of a command menu once this frame has installed
    /// its rows. The keyboard moves `mention_index` / `object_tag_index`
    /// (the composer keeps the focus); a hovered row reports back through
    /// the menu's `on_select`.
    fn sync_highlight(
        menu: &Entity<CommandState>,
        index: usize,
        count: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        if count == 0 {
            return;
        }
        let want = IndexPath::new(index.min(count - 1));
        let menu = menu.clone();
        window.defer(cx, move |window, cx| {
            menu.update(cx, |state, cx| {
                if state.matched_count() > want.row && state.selected_index() != Some(want) {
                    state.set_selected_index(Some(want), window, cx);
                }
            })
        });
    }

    fn render_mention_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The menu rises out of the composer as it opens; it is absolute, so
        // the frame moves its margin rather than the box.
        let appear = |window: &mut Window, cx: &mut App| {
            motion::appear_frame("composer-menu-appear", 4., window, cx).target()
        };
        // The kit command palette draws the frame (panel, border, menu
        // radius); this places it above the composer with its shadow.
        let menu = |frame: motion::Target, command: Command| {
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .mb(px(6. - frame.y_px()))
                .opacity(frame.alpha())
                .occlude()
                // The composer keeps the focus: a press in the menu picks
                // without moving it there.
                .capture_any_mouse_down(|_, window, _| window.prevent_default())
                .child(
                    command
                        .shadow(vec![BoxShadow {
                            color: hsla(0., 0., 0., 0.35),
                            offset: point(px(0.), px(6.)),
                            blur_radius: px(24.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .text_size(px(13.))
                        .text_color(theme::muted()),
                )
        };
        let entity = cx.entity().downgrade();
        if let Some(mention) = &self.mention {
            let matches = self.mention_matches();
            if !matches.is_empty() || self.tag_options.is_some() {
                let frame = appear(window, cx);
                let found = !matches.is_empty();
                let query = mention.query.clone();
                Self::sync_highlight(
                    &self.mention_menu,
                    self.mention_index,
                    matches.len(),
                    window,
                    cx,
                );
                let items = matches.into_iter().map(|option| {
                    CommandItem::new()
                        .label(format!("@{}", option.agent_name))
                        .child(move |_, _| mention_row(&option))
                });
                let selected = entity.clone();
                let command = Command::new(&self.mention_menu)
                    .searchable(false)
                    .max_h(px(252.))
                    .items(items)
                    .on_select(move |ix, _, cx| {
                        let _ = selected.update(cx, |this, cx| {
                            if this.mention_index != ix.row {
                                this.mention_index = ix.row;
                                cx.notify();
                            }
                        });
                    })
                    .on_confirm(move |ix, window, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(option) = this.mention_matches().get(ix.row).cloned() {
                                this.pick_mention(option, window, cx);
                            }
                        });
                    })
                    .empty(move |_, _, _| {
                        div()
                            .px(px(8.))
                            .py(px(6.))
                            .child(format!("No agent matches “{query}”."))
                    })
                    .footer(|_, _, cx| {
                        Self::lite_agents_notice(cx).unwrap_or_else(|| div().into_any_element())
                    });
                return Some(
                    menu(frame, command)
                        .debug_selector(move || {
                            if found {
                                "mention-menu".into()
                            } else {
                                "mention-none".into()
                            }
                        })
                        .into_any_element(),
                );
            }
        }
        let tag = self.object_tag.as_ref()?;
        let frame = appear(window, cx);
        let matches = self.object_matches(cx);
        Self::sync_highlight(
            &self.object_menu,
            self.object_tag_index,
            matches.len(),
            window,
            cx,
        );
        let store = Store::global(cx);
        let space = roostr_core::nav::Navigator::global(cx)
            .read(cx)
            .space()
            .to_string();
        let items: Vec<CommandItem> = matches
            .into_iter()
            .map(|option| {
                let icon = object_icon(store.read(cx), &option.icon, &option.type_key, &space);
                let opened = self.opened_rank.contains_key(&option.id);
                let name = option.name;
                CommandItem::new()
                    .label(name.clone())
                    .child(move |_, _| object_row(&icon, &name, opened))
            })
            .collect();
        let q = tag.query.trim().to_string();
        let search = q.clone();
        let selected = entity.clone();
        let command = Command::new(&self.object_menu)
            .searchable(false)
            .max_h(px(220.))
            .header(move |_, _, _| object_search_row(&search))
            .items(items)
            .on_select(move |ix, _, cx| {
                let _ = selected.update(cx, |this, cx| {
                    if this.object_tag_index != ix.row {
                        this.object_tag_index = ix.row;
                        cx.notify();
                    }
                });
            })
            .on_confirm(move |ix, window, cx| {
                let _ = entity.update(cx, |this, cx| {
                    let pick = this.object_matches(cx).get(ix.row).cloned();
                    if let Some(option) = pick {
                        this.pick_object(option, window, cx);
                    }
                });
            })
            .empty(move |_, _, _| {
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .child(format!("No object matches “{q}”."))
            });
        Some(menu(frame, command).into_any_element())
    }

    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let store = Store::global(cx);
        let chat_color = store.read(cx).chat_color.clone();
        let colors = bubble(&chat_color);
        let disabled = self.sending
            || self.draft.trim().is_empty()
            || (self.is_exchange && self.audience().is_empty());
        let placeholder = if self.is_exchange {
            "Write a message… (@ agent, ! object)"
        } else {
            "Write a comment… (@ agent, ! object)"
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.composer
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let agents: HashMap<String, String> = store
            .read(cx)
            .agents
            .iter()
            .map(|a| (a.name.clone(), a.icon.clone()))
            .collect();
        let objects: HashMap<String, (String, String)> = self
            .tagged_objects
            .iter()
            .map(|t| (t.id.clone(), (t.icon.clone(), t.type_key.clone())))
            .collect();
        let token_view =
            move |token: &InlineTokenContext, _: &mut Window, cx: &mut App| -> AnyElement {
                let id = token.token().id().to_string();
                let text = token.token().text().to_string();
                let line = token.line_height();
                if let Some(object_id) = id.strip_prefix(OBJECT_TOKEN) {
                    let (icon, type_key) = objects.get(object_id).cloned().unwrap_or_default();
                    let store = Store::global(cx);
                    let space = roostr_core::nav::Navigator::global(cx)
                        .read(cx)
                        .space()
                        .to_string();
                    let glyph = object_icon(store.read(cx), &icon, &type_key, &space);
                    return pill(
                        line,
                        roostr_core::options::tint_paint(rgb(OBJECT_GREEN).into()),
                        glyph,
                        text.trim_start_matches('!').to_string(),
                    )
                    .into_any_element();
                }
                let name = text.trim_start_matches('@').to_string();
                let icon = agents
                    .get(&name)
                    .cloned()
                    .filter(|i| !i.is_empty())
                    .unwrap_or_else(|| "🤖".into());
                // The agent's own chat colour.
                let space = roostr_core::nav::Navigator::global(cx).read(cx).space().to_string();
                let paint =
                    roostr_core::options::agent_tag_paint(Store::global(cx).read(cx), &name, &space);
                pill(line, paint, icon, name).into_any_element()
            };
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::menu())
            .mt(px(10.))
            .children(self.render_mention_menu(window, cx))
            .child(
                div()
                    .capture_action(cx.listener(Self::on_enter))
                    .capture_action(cx.listener(Self::on_tab))
                    .capture_action(cx.listener(Self::on_escape))
                    .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                        if this.on_move(-1, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                        if this.on_move(1, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.update_mention(cx);
                            cx.notify();
                        }),
                    )
                    .on_key_up(cx.listener(|this, e: &KeyUpEvent, _, cx| {
                        let key = e.keystroke.key.as_str();
                        if matches!(key, "left" | "right" | "up" | "down")
                            && this.mention.is_none()
                            && this.object_tag.is_none()
                        {
                            this.update_mention(cx);
                            cx.notify();
                        }
                    }))
                    .child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .bordered(false)
                            .readonly(self.sending)
                            .token(token_view)
                            .xsmall()
                            .px(px(8.))
                            .pt(px(12.))
                            .pb(px(4.))
                            .text_size(px(14.))
                            .line_height(relative(1.45))
                            .text_color(theme::fg()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .min_h(px(48.))
                    .child(div())
                    .child(
                        // A round button in your chat colour; dimmed (not
                        // the kit's disabled look) while there is nothing to send.
                        Button::new("send")
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(colors.bg)
                                    .foreground(colors.ink)
                                    .hover(colors.bg)
                                    .active(colors.bg),
                            )
                            .rounded(px(12.))
                            .size(px(24.))
                            .p_0()
                            .bg(colors.bg)
                            .when(disabled, |b| b.opacity(0.35))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !disabled {
                                    this.send(window, cx)
                                }
                            }))
                            .child(
                                svg()
                                    .path("icons/arrow-up.svg")
                                    .size(px(16.))
                                    .text_color(colors.ink),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_audience(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = Store::global(cx);
        let store = store.read(cx);
        let reply_all = self.reply_all();
        let current = if self.private_recipient.is_empty() {
            format!("Reply to all ({})", reply_all.len())
        } else {
            reply_all
                .iter()
                .find(|r| r.object_id == self.private_recipient)
                .map(|r| format!("Private · {}", self.member_name(r, store)))
                .unwrap_or_else(|| format!("Reply to all ({})", reply_all.len()))
        };
        let options: Vec<(String, String)> =
            std::iter::once((String::new(), format!("Reply to all ({})", reply_all.len())))
                .chain(reply_all.iter().map(|r| {
                    (
                        r.object_id.clone(),
                        format!("Private · {}", self.member_name(r, store)),
                    )
                }))
                .collect();
        let selected = self.private_recipient.clone();
        let entity = cx.entity().downgrade();
        let audience_text = if self.private_recipient.is_empty() {
            let names = self
                .audience()
                .iter()
                .map(|e| self.member_name(e, store))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "To {}",
                if names.is_empty() {
                    "no recipients".to_string()
                } else {
                    names
                }
            )
        } else {
            "New private exchange. Only this recipient receives your message.".to_string()
        };
        let request_reply = self.request_reply;
        div()
            .mt(px(10.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .text_size(px(11.))
            .text_color(theme::muted())
            .line_height(relative(1.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child("Reply audience")
                    .child(
                        Button::new("audience")
                            .label(current)
                            .outline()
                            .xsmall()
                            .disabled(self.sending)
                            .dropdown_menu(move |mut menu, _, _| {
                                for (value, label) in &options {
                                    let (value, entity) = (value.clone(), entity.clone());
                                    menu = menu.item(
                                        PopupMenuItem::new(label.clone())
                                            .checked(value == selected)
                                            .on_click(move |_, _, cx| {
                                                let _ = entity.update(cx, |this, cx| {
                                                    this.private_recipient = value.clone();
                                                    cx.notify();
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                    ),
            )
            .child(div().child(audience_text))
            .child(
                Checkbox::new("request-reply")
                    .label("Ask agents to respond")
                    .checked(request_reply)
                    .disabled(self.sending)
                    .xsmall()
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.request_reply = *checked;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

/// The green that object links and `!object` tags are tinted with.
pub(crate) const OBJECT_GREEN: u32 = 0x2ea043;

/// An inline kit `Tag` in `paint` (the kit tag look: tinted fill, border,
/// light text): icon, then the name, one composer line tall (mention and
/// object tokens in the composer).
pub(crate) fn pill(line: Pixels, paint: TagPaint, icon: String, name: String) -> Tag {
    let icon_el = if is_image_icon(&icon) {
        div().child(roostr_core::ui::emoji_or_image_icon(&icon, px(13.)))
    } else {
        div().text_size(px(12.)).child(icon)
    };
    Tag::custom(paint.bg, paint.fg, paint.border)
        .rounded(px(4.))
        .h(line)
        .gap(px(3.))
        .px(px(4.))
        .py_0()
        .text_size(px(14.))
        .line_height(line - px(2.))
        .font_weight(FontWeight::MEDIUM)
        .child(icon_el)
        .child(name)
}

/// Three pulsing dots (`.dots`): the agent composing.
fn typing_dots() -> impl IntoElement {
    div()
        .flex()
        .gap(px(3.))
        .items_center()
        .children((0..3).map(|i| {
            div()
                .size(px(5.))
                .rounded_full()
                .bg(theme::muted())
                .with_animation(
                    ("dot", i as usize),
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    move |d, t| {
                        // 0%, 60%, 100%: faint and resting; 30%: lit and lifted 2px. Each dot 0.2s later.
                        let t = (t - i as f32 * (0.2 / 1.2)).rem_euclid(1.);
                        let k = if t < 0.3 {
                            t / 0.3
                        } else if t < 0.6 {
                            1. - (t - 0.3) / 0.3
                        } else {
                            0.
                        };
                        d.opacity(0.25 + 0.75 * k).mt(px(-2. * k)).mb(px(2. * k))
                    },
                )
        }))
}

impl Render for Discussion {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.repill) {
            cx.defer_in(window, |this, window, cx| {
                let caret = this.caret(cx);
                let draft = this.draft.clone();
                this.set_draft(draft, Some(caret), window, cx);
                cx.notify();
            });
        }
        if self.is_open() && !self.focused_once && self.object.is_some() {
            self.focused_once = true;
            let composer = self.composer.clone();
            window.defer(cx, move |window, cx| {
                composer.update(cx, |s, cx| s.focus(window, cx))
            });
        }
        // Rows laid out in this frame record themselves again.
        self.layout.borrow_mut().rows.clear();
        if let Some(ix) = self.tall_check.take() {
            cx.on_next_frame(window, move |this, _, cx| {
                // A reply taller than the pane opens at its first line, not its last.
                let tall = {
                    let layout = this.layout.borrow();
                    match (layout.rows.get(&ix), layout.viewport) {
                        (Some(item), Some(view)) => item.size.height > view.size.height * 0.8,
                        _ => false,
                    }
                };
                if tall {
                    this.scroller.update(cx, |list, cx| {
                        list.scroll_to_item(ix, cx);
                    });
                    cx.notify();
                }
            });
        }
        let store_entity = Store::global(cx);
        let mut root = div().flex().flex_col();
        // Phones: no 48px rail margin (it skewed the centred opener), 16px sides.
        let mobile = roostr_core::layout::is_mobile(window);
        root = if self.embedded {
            root.flex_1().min_h_0().px(px(14.)).pb(px(12.))
        } else if self.page_mode {
            root.flex_1().min_h_0().mt(px(8.))
        } else if self.full {
            root.mt(px(8.))
                .flex_1()
                .min_h(window.viewport_size().height * 0.6)
        } else if mobile {
            root.mt(px(32.))
        } else {
            root.mt(px(32.)).ml(px(48.))
        };
        if mobile && !self.embedded {
            root = root.px(px(16.));
        }

        let Some(_) = &self.object else {
            return root.into_any_element();
        };

        if !self.is_open() {
            let count = self.messages.len();
            let label = if count > 0 {
                format!("{count} comment{}", if count == 1 { "" } else { "s" })
            } else {
                "Start a discussion".into()
            };
            let opener = Button::new("opener")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(rgba(0xffffff0f).into())
                        .foreground(theme::muted())
                        .hover(theme::hover())
                        .active(theme::hover()),
                )
                .xsmall()
                .bg(rgba(0xffffff0f))
                .font_weight(FontWeight::MEDIUM)
                .map(|b| {
                    if mobile {
                        // A centred round chat button.
                        b.rounded(px(24.))
                            .size(px(48.))
                            .p_0()
                            .child(div().text_size(px(20.)).child("💬"))
                    } else {
                        b.rounded(px(18.))
                            .h_auto()
                            .pl(px(10.))
                            .pr(px(12.))
                            .py(px(7.))
                            .child(div().text_size(px(12.)).child("💬"))
                            .child(div().text_size(px(12.)).child(label))
                    }
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    // Anytype's phone idiom: the discussion opens as its own page.
                    if roostr_core::layout::is_mobile(window) {
                        nav::navigate(Route::Chat(this.object_id.clone()), cx);
                        return;
                    }
                    this.open = true;
                    this.derive(cx);
                    this.composer.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                }));
            return root
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .pt(px(4.))
                        .pb(px(12.))
                        .child(opener),
                )
                .into_any_element();
        }

        if !self.full {
            // A short rule, then the 600-weight title.
            root = root
                .child(div().w_full().h(px(1.)).bg(theme::border()).mb(px(16.)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .mb(px(8.))
                        .child(
                            Label::new(if self.is_exchange {
                                self.exchange_title.to_uppercase()
                            } else {
                                "DISCUSSION".into()
                            })
                            .line_height(phi())
                            .text_size(px(11.))
                            .text_color(theme::muted()),
                        )
                        .child(
                            Button::new("collapse")
                                .ghost()
                                .xsmall()
                                .text_color(theme::muted())
                                .tooltip("Collapse")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open = false;
                                    this.derive(cx);
                                    cx.notify();
                                }))
                                .child(div().text_size(px(14.)).child("×")),
                        ),
                );
        }

        // Notices: warnings in orange, the rest muted.
        let notice = |id: &'static str, text: String, error: bool| {
            Alert::new(id, text)
                .with_variant(if error {
                    AlertVariant::Warning
                } else {
                    AlertVariant::Default
                })
                .when(error, |a| a.icon(IconName::TriangleAlert))
                .xsmall()
                .my(px(12.))
                .text_size(px(12.))
                .text_color(if error {
                    theme::orange()
                } else {
                    theme::muted()
                })
        };
        if !self.identity_error.is_empty() {
            root = root
                .child(notice(
                    "identity-error",
                    format!(
                        "Cannot load your discussion identity: {}",
                        self.identity_error
                    ),
                    true,
                ))
                .child(
                    Button::new("identity-retry")
                        .label("Retry discussion identity")
                        .outline()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| this.load_identity(cx))),
                );
        }
        let paired = roostr_core::harness::harness(cx).is_some();
        if !self.is_exchange && paired && !self.presence_error.is_empty() {
            root = root.child(notice(
                "presence-error",
                format!(
                    "{} Check that the paired native app and harness are running. Discussion remains available.",
                    self.presence_error
                ),
                true,
            ));
        }
        {
            let store = store_entity.read(cx);
            if self.is_exchange && !self.members.is_empty() {
                let links = self.members.iter().enumerate().map(|(i, member)| {
                    let target = member.object_id.clone();
                    Link::new(("member", i))
                        .text_color(theme::fg())
                        .text_decoration_color(theme::fg().opacity(0.4))
                        .on_click(move |_, _, cx| nav::navigate(Route::Object(target.clone()), cx))
                        .child(self.member_name(member, store))
                });
                root = root.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x(px(8.))
                        .gap_y(px(5.))
                        .py(px(8.))
                        .text_size(px(11.))
                        .line_height(relative(1.5))
                        .text_color(theme::muted())
                        .child("Participants")
                        .children(links),
                );
            } else if self.is_exchange
                && let Some(conversation) = &self.conversation
                && !conversation.participants.is_empty()
            {
                let names = conversation
                    .participants
                    .iter()
                    .map(|p| self.who(p, store))
                    .collect::<Vec<_>>()
                    .join(", ");
                root = root.child(notice(
                    "participants",
                    format!("Participants: {names}"),
                    false,
                ));
            }
        }
        if self.legacy {
            root = root.child(notice(
                "legacy",
                "This shared exchange is read-only, awaiting migration to object inboxes.".into(),
                false,
            ));
        }
        if self.is_exchange && self.conversation.as_ref().is_some_and(|c| c.closed) {
            root = root.child(notice("closed", "This exchange is closed.".into(), false));
        }
        if !self.retry_error.is_empty() {
            root = root.child(notice(
                "retry-error",
                format!("Retry failed: {}", self.retry_error),
                true,
            ));
        }

        // Messages.
        if self.own_scroller() {
            // The kit's message scroller: follows the newest message, a
            // "Jump to latest" button while reading further up. Rows carry
            // their own spacing, so its row and list padding are off.
            let layout = self.layout.clone();
            root = root.child(
                div()
                    .debug_selector(|| "chat-messages".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_prepaint(move |bounds, _, _| layout.borrow_mut().viewport = Some(bounds))
                    .child(
                        MessageScroller::new(
                            "chat-messages",
                            self.scroller.clone(),
                            cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)),
                        )
                        .scrollbar(false)
                        .with_list_style(StyleRefinement::default().p_0())
                        .with_row_style(StyleRefinement::default().p_0()),
                    ),
            );
        } else {
            let items: Vec<AnyElement> = (0..=self.messages.len())
                .map(|ix| self.render_item(ix, window, cx))
                .collect();
            // `.discussion.full .messages { flex: 1 }`: the composer sits at the
            // bottom of the 60vh-tall section, not right under the messages.
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .when(self.full, |d| d.flex_1())
                    .children(items),
            );
        }

        if !self.read_only
            && !self.reply_to.is_empty()
            && let Some(target) = self.message(&self.reply_to)
        {
            let store = store_entity.read(cx);
            root = root.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .border_l_2()
                    .border_color(theme::accent())
                    .px(px(8.))
                    .py(px(2.))
                    .mt(px(8.))
                    .mb(px(4.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Replying to {}", self.who(&target.author, store))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(target.text.chars().take(60).collect::<String>()),
                    )
                    .child(
                        Button::new("cancel-reply")
                            .ghost()
                            .xsmall()
                            .ml_auto()
                            .flex_none()
                            .tooltip("Cancel reply")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reply_to.clear();
                                this.private_recipient.clear();
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            );
        }
        if !self.send_error.is_empty() {
            root = root.child(notice("send-error", self.send_error.clone(), true));
        }
        if !self.read_only {
            if self.is_exchange {
                root = root.child(self.render_audience(cx));
            }
            root = root.child(self.render_composer(window, cx));
        }
        root.into_any_element()
    }
}

impl Focusable for Discussion {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

#[cfg(test)]
mod presence_tests {
    use super::{AgentPresence, presence_belongs};

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
}
