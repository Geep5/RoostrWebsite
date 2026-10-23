//! An object's discussion (`src/lib/components/Discussion.svelte`). Human and
//! private chat stays local; exchanges send immutable mailbox envelopes.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Enter, Escape, IndentInline};
use gpui_kit::component::input::{
    InlineToken, InlineTokenContext, InputContent, InputEvent, MoveDown, MoveUp, Textarea,
    TextareaState,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::color::bubble;
use roostr_core::io;
use roostr_core::nav::{self, Route};
use roostr_core::objects::{is_image_icon, object_icon};
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

const PRESENCE_POLL: Duration = Duration::from_millis(2500);
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
    list: ListState,
    fingerprints: Vec<u64>,
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
    picker_for: String,
    sending: bool,
    send_error: String,
    me: String,
    identity_error: String,
    private_recipient: String,
    request_reply: bool,
    retry_error: String,
    retrying: Vec<String>,
    pending_send: Option<AgentMessage>,

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
    object_menu_scroll: ScrollHandle,

    presence: Vec<AgentPresence>,
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
        let list = ListState::new(1, ListAlignment::Top, px(600.));
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
            list,
            fingerprints: Vec::new(),
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
            picker_for: String::new(),
            sending: false,
            send_error: String::new(),
            me: String::new(),
            identity_error: String::new(),
            private_recipient: String::new(),
            request_reply: true,
            retry_error: String::new(),
            retrying: Vec::new(),
            pending_send: None,
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
            object_menu_scroll: ScrollHandle::new(),
            presence: Vec::new(),
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
            self.object = object;
            self.derive(cx);
        }
        cx.notify();
    }

    fn derive(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object.clone() else {
            return;
        };
        let thread = self.thread_id.as_str();
        let messages = if thread == DISCUSSION {
            object_chat_messages(&object)
        } else {
            chat_messages(&object, thread)
        };
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
        let at_end = self.at_end();
        self.note_arrival(&previous, at_end);
        self.sync_list();
        self.follow_new_messages(at_end, cx);
        self.update_polling(cx);
        if !self.seeded {
            self.seeded = true;
            self.seed_default_agent(Vec::new(), cx);
        }
    }

    fn message_index(&self, id: &str) -> Option<usize> {
        self.messages.iter().position(|m| m.id == id)
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

    /// Splice the list from the first message whose rendering inputs changed.
    fn sync_list(&mut self) {
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
                    &m.reply_to,
                    &m.origin,
                    &self.me,
                    self.read_only,
                    self.is_exchange,
                )
                    .hash(&mut h);
                for r in &m.reactions {
                    (&r.emoji, &r.authors).hash(&mut h);
                }
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
        // Everything from the first difference on, plus the trailing presence row.
        self.list.splice(prefix..old_total, new_total - prefix);
        self.fingerprints = fingerprints;
    }

    /// The trailing row (empty note, typing, problem) changed height.
    fn remeasure_footer(&self) {
        let n = self.fingerprints.len();
        self.list.splice(n..n + 1, 1);
    }

    /// Within a message or so of the end: the reader is following along.
    /// Read from the list when needed, never from its scroll handler: the
    /// list calls that with its state borrowed, so reading the offsets there
    /// panicked on the first wheel or touch scroll (`RefCell already mutably
    /// borrowed`), which aborted the desktop app and left the browser and iOS
    /// apps taking no input at all.
    fn at_end(&self) -> bool {
        let max = self.list.max_offset_for_scrollbar().y;
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        max - offset < px(120.)
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
            self.list.set_follow_mode(FollowMode::Tail);
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
        self.list.set_follow_mode(FollowMode::Tail);
        self.tall_check = Some(count - 1);
        let _ = cx;
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
                this.sync_list();
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
                self.remeasure_footer();
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
                            this.remeasure_footer();
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
        self.presence_here().find(|p| p.state == "working")
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

    fn origin_name(&self, id: &str, store: &Store) -> String {
        store
            .summary(id)
            .map(|s| s.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "object".into())
    }

    /// Emoji avatar for an author, when it has one.
    fn avatar_emoji(&self, author: &str, store: &Store) -> String {
        if let Some(agent) = store.agent(author)
            && !agent.icon.is_empty()
        {
            return agent.icon.clone();
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

    fn move_object_tag(&mut self, delta: isize, count: usize) {
        let n = count as isize;
        self.object_tag_index = ((self.object_tag_index as isize + delta + n) % n) as usize;
        self.object_menu_scroll
            .scroll_to_item(self.object_tag_index);
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
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(_) => {
                    // Cleared only once the change is committed: a failed write
                    // must not swallow the message.
                    this.clear_composer(window, cx);
                    this.finish_send(Ok(()), window, cx);
                    this.seed_default_agent(mentions, cx);
                }
                Err(error) => this.finish_send(Err(error), window, cx),
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
        self.list.reset(1);
        let draft = load_draft(&self.object_id, &self.thread_id, cx);
        self.tagged.clear();
        self.tagged_objects.clear();
        self.set_draft(draft, None, window, cx);
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

    fn toggle_reaction(&mut self, message_id: String, emoji: String, cx: &mut Context<Self>) {
        if self.read_only || self.is_exchange {
            return;
        }
        self.picker_for.clear();
        self.send_error.clear();
        self.sync_list();
        let task = roostr_core::api::chat::react(&self.object_id, &message_id, &emoji, cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.send_error = error.to_string();
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn toggle_picker(&mut self, message_id: &str, cx: &mut Context<Self>) {
        self.picker_for = if self.picker_for == message_id {
            String::new()
        } else {
            message_id.to_string()
        };
        self.sync_list();
        cx.notify();
    }

    fn scroll_to_message(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.message_index(id)
            && self.own_scroller()
        {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
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

impl Discussion {
    fn render_avatar(&self, id: ElementId, author: &str, store: &Store) -> AnyElement {
        let emoji = self.avatar_emoji(author, store);
        let link = self.author_links(author, store).then(|| author.to_string());
        let title = format!("Open {}", self.who(author, store));
        let base = div()
            .id(id)
            .flex_none()
            .size(px(28.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::white());
        let base = if emoji.is_empty() {
            base.bg(avatar_color(author))
                .text_size(px(10.))
                .font_family(theme::fonts::MONO)
                .child(self.initials(author, store))
        } else if is_image_icon(&emoji) {
            base.bg(theme::hl_light())
                .overflow_hidden()
                .child(roostr_core::ui::emoji_or_image_icon(&emoji, px(28.)))
        } else {
            base.bg(theme::hl_light()).text_size(px(17.)).child(emoji)
        };
        match link {
            Some(target) => base
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
                .into_any_element(),
            None => base.into_any_element(),
        }
    }

    fn render_message(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = self.messages[ix].clone();
        // The actions fade in while the message is hovered
        // (`.msg .actions { transition: opacity 0.1s }`).
        let reveal = (!self.read_only).then(|| {
            let id = SharedString::from(format!("msg-actions-{}", m.id));
            motion::hover(id, motion::HOVER_REVEAL, window, cx)
        });
        let picker_frame = (self.picker_for == m.id).then(|| {
            let id = SharedString::from(format!("picker-appear-{}", m.id));
            motion::appear_frame(id, -4., window, cx)
        });
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

        let mut body = div().min_w_0().max_w(relative(0.78)).flex().flex_col();
        body = if own {
            body.items_end()
        } else {
            body.items_start()
        };

        // Reply quote.
        if !m.reply_to.is_empty()
            && let Some(target) = self.message(&m.reply_to)
        {
            let target_id = target.id.clone();
            body = body.child(
                div()
                    .id(("quote", ix))
                    .flex()
                    .gap(px(6.))
                    .items_baseline()
                    .max_w_full()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .border_l_2()
                    .border_color(theme::accent())
                    .px(px(6.))
                    .py(px(1.))
                    .mb(px(2.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.scroll_to_message(&target_id, cx)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.who(&target.author, store)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(target.text.chars().take(80).collect::<String>()),
                    ),
            );
        }

        // Meta row: author, origin, time.
        let mut meta = div().flex().items_baseline().gap(px(8.));
        if own {
            meta = meta.justify_end();
        } else {
            let name = self.who(&m.author, store);
            meta = meta.child(if self.author_links(&m.author, store) {
                let target = m.author.clone();
                div()
                    .id(("author", ix))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::fg())
                    .cursor_pointer()
                    .hover(|s| s.underline())
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
        if !m.origin.is_empty() {
            let target = m.origin.clone();
            meta = meta.child(
                div()
                    .id(("origin", ix))
                    .text_size(px(11.))
                    .text_color(theme::accent())
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(6.))
                    .px(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.border_color(theme::accent()))
                    .tooltip(|window, cx| Tooltip::new("Asked from this object").build(window, cx))
                    .on_click(move |_, _, cx| nav::navigate(Route::Object(target.clone()), cx))
                    .child(format!("↳ {}", self.origin_name(&m.origin, store))),
            );
        }
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(theme::muted())
                .child(when(m.ts)),
        );
        body = body.child(meta);

        // The bubble.
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
        // Each message's lines read after the previous message's.
        let text = render_markdown(("md", ix), &m.text, &md_theme, (ix as u64) << 24, cx);
        let bubble_el = div()
            .text_size(px(14.))
            .line_height(relative(1.45))
            .px(px(12.))
            .py(px(7.))
            .rounded(px(16.))
            .map(|d| {
                if own {
                    d.rounded_br(px(5.))
                } else {
                    d.rounded_bl(px(5.))
                }
            })
            .bg(if tinted { colors.bg } else { theme::hover() })
            .text_color(if tinted { colors.ink } else { theme::fg() })
            .max_w_full()
            .child(text);
        body = body.child(bubble_el);

        // Delivery / processing status: quiet when all is well.
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
                    .px(px(2.))
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
                div()
                    .id(id)
                    .border_1()
                    .border_color(theme::border())
                    .rounded(px(5.))
                    .px(px(5.))
                    .py(px(1.))
                    .when(disabled, |d| d.opacity(0.5))
                    .when(!disabled, |d| d.cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            on(this, cx)
                        }
                    }))
                    .child("Retry")
            };
            let line = if let Some(failed) = failed_delivery {
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
            body = body.children(line);
        }

        // Reactions.
        if !m.reactions.is_empty() {
            let disabled = self.read_only || self.is_exchange;
            let mine_color = roostr_core::color::hex(&store.chat_color).unwrap_or(theme::accent());
            let chips = m.reactions.iter().enumerate().map(|(i, r)| {
                let mine = r.authors.contains(&self.me);
                let names = r
                    .authors
                    .iter()
                    .map(|a| self.who(a, store))
                    .collect::<Vec<_>>()
                    .join(", ");
                let (id, emoji) = (m.id.clone(), r.emoji.clone());
                div()
                    .id(("chip", i))
                    .border_1()
                    .border_color(if mine { mine_color } else { theme::border() })
                    .when(mine, |d| d.bg(mine_color.opacity(0.16)))
                    .rounded_full()
                    .px(px(8.))
                    .py(px(1.))
                    .text_size(px(12.))
                    .when(!disabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.border_color(theme::accent()))
                    })
                    .tooltip(move |window, cx| Tooltip::new(names.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_reaction(id.clone(), emoji.clone(), cx)
                    }))
                    .child(format!("{} {}", r.emoji, r.authors.len()))
            });
            body = body.child(
                div()
                    .id(("reactions", ix))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .mt(px(4.))
                    .when(own, |d| d.justify_end())
                    .children(chips),
            );
        }

        // Hover actions.
        let actions = reveal.as_ref().map(|(opacity, _)| {
            let action_button = |id: &'static str, label: &'static str| {
                div()
                    .id((id, ix))
                    .text_size(px(13.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover()))
                    .child(label)
            };
            let id = m.id.clone();
            let id2 = m.id.clone();
            let id3 = m.id.clone();
            let exchange = self.is_exchange;
            div()
                .flex()
                .gap(px(2.))
                .self_start()
                .opacity(*opacity)
                .when(!exchange, |d| {
                    d.child(
                        action_button("react", "😀")
                            .tooltip(|window, cx| Tooltip::new("Add reaction").build(window, cx))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_picker(&id, cx)),
                            ),
                    )
                })
                .child(
                    action_button("reply", "↩")
                        .tooltip(move |window, cx| {
                            Tooltip::new(if exchange { "Reply to all" } else { "Reply" })
                                .build(window, cx)
                        })
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
                            action_button("private", "Private")
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .tooltip(|window, cx| {
                                    Tooltip::new("Reply privately in a separate exchange")
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_reply(&id3, true, window, cx)
                                })),
                        )
                    },
                )
        });

        // Reaction picker.
        let picker = picker_frame.map(|frame| {
            let id = m.id.clone();
            let entity = cx.entity().downgrade();
            let entity2 = entity.clone();
            div()
                .absolute()
                .right_0()
                .top(px(24.))
                .size_0()
                .child(deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            frame
                                .apply(div())
                                .occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.picker_for.clear();
                                    this.sync_list();
                                    cx.notify();
                                }))
                                .child(
                                    roostr_properties::EmojiPicker::new(("emoji-picker", ix))
                                        .on_pick(move |emoji, _, cx| {
                                            let (id, emoji) = (id.clone(), emoji.to_string());
                                            let _ = entity.update(cx, |this, cx| {
                                                this.toggle_reaction(id, emoji, cx)
                                            });
                                        })
                                        .on_close(move |_, cx| {
                                            let _ = entity2.update(cx, |this, cx| {
                                                this.picker_for.clear();
                                                this.sync_list();
                                                cx.notify();
                                            });
                                        }),
                                ),
                        ),
                ))
        });

        div()
            .id(ElementId::Name(format!("msg-{mid}").into()))
            .when_some(reveal.map(|(_, hover)| hover), |d, hover| {
                d.on_hover(move |hovered, _, cx| motion::HoverMotion::set(&hover, *hovered, cx))
            })
            .relative()
            .flex()
            .gap(px(10.))
            .items_end()
            .when(own, |d| d.flex_row_reverse())
            .when(!own, |d| {
                d.child(self.render_avatar(("avatar", ix).into(), &m.author, store))
            })
            .child(body)
            .children(actions)
            .children(picker)
            .into_any_element()
    }

    /// The trailing row: empty note, the agent composing, or its problem.
    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut footer = div().flex().flex_col();
        if self.messages.is_empty() {
            footer = footer.child(
                div()
                    .my(px(13.))
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child("No messages yet."),
            );
        }
        let working = self.agent_working().cloned();
        if let Some(agent) = &working {
            let avatar = div()
                .flex_none()
                .size(px(28.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::white());
            let avatar = if agent.icon.is_empty() {
                avatar
                    .bg(avatar_color(&agent.id))
                    .text_size(px(10.))
                    .font_family(theme::fonts::MONO)
                    .child(agent.name.chars().take(2).collect::<String>())
            } else {
                avatar
                    .bg(theme::hl_light())
                    .text_size(px(17.))
                    .child(agent.icon.clone())
            };
            footer = footer.child(
                div()
                    .id("working")
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
                    .child(avatar)
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
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(theme::orange())
                    .child(div().text_size(px(13.)).child("⚠︎"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .line_height(relative(1.4))
                            .child(format!(
                                "{} hit a problem: {}{}",
                                agent.name,
                                agent.detail,
                                if hint.is_empty() {
                                    String::new()
                                } else {
                                    format!(" — {hint}")
                                }
                            )),
                    ),
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
        if ix < n {
            let item = div().pt(top).child(self.render_message(ix, window, cx));
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
                .when(has, |d| d.pt(top))
                .pb(px(4.))
                .child(footer)
                .into_any_element()
        }
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
        let menu = |frame: motion::Target| {
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .right_0()
                .mb(px(6. - frame.y_px()))
                .opacity(frame.alpha())
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(theme::radius::menu())
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.35),
                    offset: point(px(0.), px(6.)),
                    blur_radius: px(24.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .text_size(px(13.))
                .text_color(theme::muted())
                .occlude()
        };
        let row = |id: ElementId, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .px(px(8.))
                .py(px(7.))
                .rounded(px(6.))
                .text_color(theme::fg())
                .cursor_pointer()
                .when(active, |d| d.bg(theme::border()))
        };
        if let Some(mention) = &self.mention {
            let matches = self.mention_matches();
            if !matches.is_empty() {
                let frame = appear(window, cx);
                let rows = matches.into_iter().enumerate().map(|(i, option)| {
                    let icon = if option.icon.is_empty() {
                        div()
                            .size(px(20.))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(avatar_color(&option.endpoint.agent_id))
                            .text_color(theme::white())
                            .text_size(px(9.))
                            .child(
                                option
                                    .agent_name
                                    .chars()
                                    .take(2)
                                    .collect::<String>()
                                    .to_uppercase(),
                            )
                    } else {
                        div()
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.))
                            .child(option.icon.clone())
                    };
                    let picked = option.clone();
                    row(("mention", i).into(), i == self.mention_index)
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.mention_index != i {
                                this.mention_index = i;
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_mention(picked.clone(), window, cx)
                        }))
                        .child(icon)
                        .child(
                            div()
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
                });
                return Some(
                    menu(frame)
                        .id("mention-menu")
                        .debug_selector(|| "mention-menu".into())
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .children(rows)
                        .children(Self::lite_agents_notice(cx))
                        .into_any_element(),
                );
            }
            if self.tag_options.is_some() {
                return Some(
                    menu(appear(window, cx))
                        .debug_selector(|| "mention-none".into())
                        .px(px(12.))
                        .py(px(10.))
                        .child(format!("No agent matches “{}”.", mention.query))
                        .children(Self::lite_agents_notice(cx))
                        .into_any_element(),
                );
            }
        }
        let tag = self.object_tag.as_ref()?;
        let frame = appear(window, cx);
        let matches = self.object_matches(cx);
        let store = Store::global(cx);
        let space = roostr_core::nav::Navigator::global(cx)
            .read(cx)
            .space()
            .to_string();
        let q = tag.query.trim().to_string();
        let search = div()
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
                        q.clone()
                    }),
            );
        let body = if matches.is_empty() {
            div()
                .px(px(12.))
                .py(px(10.))
                .child(format!("No object matches “{q}”."))
                .into_any_element()
        } else {
            let rows = matches.into_iter().enumerate().map(|(i, option)| {
                let icon = object_icon(store.read(cx), &option.icon, &option.type_key, &space);
                let icon_el = if is_image_icon(&icon) {
                    div()
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(roostr_core::ui::emoji_or_image_icon(&icon, px(18.)))
                } else {
                    div()
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.))
                        .child(icon)
                };
                let picked = option.clone();
                let opened = self.opened_rank.contains_key(&option.id);
                row(("object-tag", i).into(), i == self.object_tag_index)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.object_tag_index != i {
                            this.object_tag_index = i;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_object(picked.clone(), window, cx)
                    }))
                    .child(icon_el)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(option.name.clone()),
                    )
                    .when(opened, |d| {
                        d.child(
                            div()
                                .ml_auto()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child("opened"),
                        )
                    })
            });
            div()
                .id("object-menu-list")
                .flex()
                .flex_col()
                .p(px(4.))
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.object_menu_scroll)
                .children(rows)
                .into_any_element()
        };
        Some(
            menu(frame)
                .flex()
                .flex_col()
                .max_h(px(260.))
                .overflow_hidden()
                .child(search)
                .child(body)
                .into_any_element(),
        )
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
                        rgba(0x2ea04338).into(),
                        theme::fg(),
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
                pill(line, theme::white(), rgb(0x1b1b1b).into(), icon, name).into_any_element()
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
                        div()
                            .id("send")
                            .size(px(24.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors.bg)
                            .text_color(colors.ink)
                            .when(disabled, |d| d.opacity(0.35))
                            .when(!disabled, |d| d.cursor_pointer())
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

/// An inline pill: icon, then the name (mentions in the composer and messages).
pub(crate) fn pill(line: Pixels, bg: Hsla, ink: Hsla, icon: String, name: String) -> Div {
    let icon_el = if is_image_icon(&icon) {
        div().child(roostr_core::ui::emoji_or_image_icon(&icon, px(13.)))
    } else {
        div().text_size(px(12.)).child(icon)
    };
    div()
        .h(line)
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(2.))
        .rounded(px(3.))
        .bg(bg)
        .text_color(ink)
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
        if let Some(ix) = self.tall_check.take() {
            cx.on_next_frame(window, move |this, _, cx| {
                // A reply taller than the pane opens at its first line, not its last.
                if let Some(item) = this.list.bounds_for_item(ix)
                    && item.size.height > this.list.viewport_bounds().size.height * 0.8
                {
                    this.list.scroll_to(ListOffset {
                        item_ix: ix,
                        offset_in_item: px(0.),
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
            return root
                .child(
                    div().flex().justify_center().pt(px(4.)).pb(px(12.)).child(
                        div()
                            .id("opener")
                            .flex()
                            .items_center()
                            .bg(rgba(0xffffff0f))
                            .text_color(theme::muted())
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::hover()).text_color(theme::fg()))
                            .map(|d| {
                                if mobile {
                                    // A centred round chat button.
                                    d.size(px(48.))
                                        .rounded_full()
                                        .justify_center()
                                        .text_size(px(20.))
                                        .child("💬")
                                } else {
                                    d.gap(px(4.))
                                        .text_size(px(12.))
                                        .pl(px(10.))
                                        .pr(px(12.))
                                        .py(px(7.))
                                        .rounded(px(18.))
                                        .child(div().text_size(px(12.)).child("💬"))
                                        .child(label)
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
                            })),
                    ),
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
                        .child(div().text_size(px(11.)).text_color(theme::muted()).child(
                            if self.is_exchange {
                                self.exchange_title.to_uppercase()
                            } else {
                                "DISCUSSION".into()
                            },
                        ))
                        .child(
                            div()
                                .id("collapse")
                                .text_size(px(14.))
                                .text_color(theme::muted())
                                .cursor_pointer()
                                .tooltip(|window, cx| Tooltip::new("Collapse").build(window, cx))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open = false;
                                    this.derive(cx);
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                );
        }

        let notice = |text: String, error: bool| {
            div()
                .my(px(12.))
                .pt(px(2.))
                .pb(px(6.))
                .text_size(px(12.))
                .text_color(if error {
                    theme::orange()
                } else {
                    theme::muted()
                })
                .child(text)
        };
        if !self.identity_error.is_empty() {
            root = root
                .child(notice(
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
                    div()
                        .id(("member", i))
                        .text_color(theme::fg())
                        .cursor_pointer()
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
                root = root.child(notice(format!("Participants: {names}"), false));
            }
        }
        if self.legacy {
            root = root.child(notice(
                "This shared exchange is read-only, awaiting migration to object inboxes.".into(),
                false,
            ));
        }
        if self.is_exchange && self.conversation.as_ref().is_some_and(|c| c.closed) {
            root = root.child(notice("This exchange is closed.".into(), false));
        }
        if !self.retry_error.is_empty() {
            root = root.child(notice(format!("Retry failed: {}", self.retry_error), true));
        }

        // Messages.
        if self.own_scroller() {
            root = root.child(
                div()
                    .debug_selector(|| "chat-messages".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        list(
                            self.list.clone(),
                            cx.processor(|this, ix, window, cx| this.render_item(ix, window, cx)),
                        )
                        .size_full(),
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
                        div()
                            .id("cancel-reply")
                            .ml_auto()
                            .flex_none()
                            .cursor_pointer()
                            .tooltip(|window, cx| Tooltip::new("Cancel reply").build(window, cx))
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
            root = root.child(notice(self.send_error.clone(), true));
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
