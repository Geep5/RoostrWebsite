//! The block editor for an object's body (`Editor.svelte`): document state,
//! the ordered write lane, undo, debounced saves and reconciling remote
//! changes. Keyboard and input live in `keys.rs`, block operations and menus
//! in `ops.rs`, drawing in `render.rs`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use gpui_kit::*;
use roostr_core::types::{Block, BlockContent, Object, TextContent, Value, pos, style};
use roostr_core::{JsonMap, Store, StoreEvent, api};

use crate::block_menu::BlockMenu;
use crate::codelint::LintProblem;
use crate::doc::{self, History, Op, Place};
use crate::link_picker::{LinkPicker, LinkPrompt};
use crate::marks::{self, Span};
use crate::text::{Layouts, TextState};
use crate::write_queue::WriteQueue;

/// Keys that reshape the tree, queued while an async structural op runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StructuralKey {
    Enter,
    Tab { shift: bool },
}

/// Rubber-band selection in progress (Anytype selection/provider.tsx).
pub(crate) struct DragSel {
    /// Anchor in window coordinates (the page scrolls under it; see `scroll_y`).
    pub anchor: Point<Pixels>,
    pub base: Vec<String>,
    pub mode: SelMode,
    pub moved: bool,
    /// Started inside a text: the native text drag owns it until it crosses into a second block.
    pub text_origin: bool,
    /// Started on the page background (a plain click there is a background click).
    pub background: bool,
    pub last: Point<Pixels>,
    /// Editor top at arm time, so the anchor follows the page when it scrolls.
    pub editor_top: Pixels,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelMode {
    Replace,
    Toggle,
    Remove,
}

/// A row lifted by a long press (Editor.svelte touchDrag): its floating
/// copy rides the finger above the page, the finger drives the same
/// targeting as the mouse, and the release commits.
pub(crate) struct TouchLift {
    pub label: SharedString,
    /// The row's bounds when it was picked up.
    pub row: Bounds<Pixels>,
    /// The x the finger picked it up at.
    pub x0: Pixels,
    /// The finger.
    pub at: Point<Pixels>,
}

/// The marks toolbar over a text selection.
#[derive(Clone)]
pub(crate) struct Toolbar {
    pub block: String,
    pub from: usize,
    pub to: usize,
    pub x: Pixels,
    pub y: Pixels,
}

/// Slash menu: block, byte offset of the "/", live filter, anchor, and the
/// kit command list (created on first render; it owns the highlight).
#[derive(Clone)]
pub(crate) struct Slash {
    pub block: String,
    pub start: usize,
    pub filter: String,
    pub pos: Option<Point<Pixels>>,
    pub cmd: Option<Entity<gpui_kit::component::command::CommandState>>,
}

/// "Paste as" menu (Anytype editor/page.tsx onPasteUrl).
#[derive(Clone)]
pub(crate) struct PasteMenu {
    pub block: String,
    pub url: String,
    pub processor: Option<crate::embed::Processor>,
    pub pos: Point<Pixels>,
    pub at: usize,
    /// The kit menu showing the choices (built on first render).
    pub popup: Option<Entity<gpui_kit::component::menu::PopupMenu>>,
}

/// Spelling suggestions for the word under a right-click.
#[derive(Clone)]
pub(crate) struct SpellMenu {
    pub block: String,
    pub word: String,
    pub from: usize,
    pub to: usize,
    pub pos: Point<Pixels>,
    pub options: Vec<String>,
    /// The kit menu showing the suggestions (built on first render).
    pub popup: Option<Entity<gpui_kit::component::menu::PopupMenu>>,
}

pub(crate) struct OpenBlockMenu {
    pub block: String,
    pub group: Option<Vec<String>>,
    pub menu: Entity<BlockMenu>,
    pub _sub: Subscription,
}

/// A drop target resolved for the pointer (Anytype hoverData).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DropHint {
    pub id: String,
    pub position: u32,
    /// The targetBot strip under a subtree.
    pub bot: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DropRect {
    pub id: String,
    pub bounds: Bounds<Pixels>,
    pub inner: bool,
    pub bot: bool,
    pub doc_end: bool,
}

/// Geometry of a block row on the last frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowGeom {
    pub bounds: Bounds<Pixels>,
    /// Content starts after the 48px gutter.
    pub gutter: bool,
}

pub(crate) type Rows = Rc<std::cell::RefCell<HashMap<String, RowGeom>>>;

/// The block editor for an object's body (`Editor.svelte`).
pub struct Editor {
    pub(crate) object_id: String,
    pub(crate) read_only: bool,
    pub(crate) focus_handle: FocusHandle,
    /// Fields and metadata of the object (blocks live in `blocks`).
    pub(crate) object: Option<Object>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) load_error: Option<SharedString>,
    /// The live text of every text block (the web's contenteditable DOM).
    pub(crate) texts: HashMap<String, TextState>,
    /// The block holding the caret.
    pub(crate) active: Option<String>,
    pub(crate) has_focus: bool,
    pub(crate) focus_pending: bool,
    /// Caret to place once the block exists (the web's focusRequest).
    pub(crate) focus_request: Option<(String, usize)>,
    pub(crate) history: History,
    pub(crate) writes: WriteQueue,
    /// Blocks with unsaved user input. Saves fire only for these.
    pub(crate) dirty: HashSet<String>,
    pub(crate) save_timers: HashMap<String, Task<()>>,
    pub(crate) structural_busy: bool,
    pub(crate) structural_queue: VecDeque<StructuralKey>,
    // Multi-block selection.
    pub(crate) selected: Vec<String>,
    pub(crate) last_focused: String,
    pub(crate) drag_sel: Option<DragSel>,
    pub(crate) sel_rect: Option<Bounds<Pixels>>,
    pub(crate) autoscroll: Option<Task<()>>,
    /// Text drag-selection inside one block: (block, moved).
    pub(crate) text_drag: Option<(String, bool)>,
    // Popups.
    pub(crate) toolbar: Option<Toolbar>,
    pub(crate) slash: Option<Slash>,
    pub(crate) block_menu: Option<OpenBlockMenu>,
    pub(crate) link_picker: Option<(String, Entity<LinkPicker>, Point<Pixels>, Subscription)>,
    pub(crate) link_prompt: Option<(Toolbar, Entity<LinkPrompt>, Subscription)>,
    pub(crate) property_suggest: Option<(String, AnyView, Point<Pixels>, Subscription)>,
    pub(crate) turn_into: Option<(Toolbar, AnyView, Subscription)>,
    pub(crate) paste_menu: Option<PasteMenu>,
    pub(crate) spell_menu: Option<SpellMenu>,
    pub(crate) file_notice: String,
    // Drag & drop.
    pub(crate) dragging: Option<String>,
    pub(crate) drop_hint: Option<DropHint>,
    pub(crate) last_valid_drop: Option<DropHint>,
    pub(crate) drag_rects: Vec<DropRect>,
    pub(crate) drag_point: Point<Pixels>,
    pub(crate) drag_scroll: Option<Task<()>>,
    /// The paths of an OS file drag over the editor. The drop is taken from
    /// the window's mouse-up rather than `on_drop`: GPUI's drop needs the
    /// hitbox hovered, which it denies after keyboard input, and a Finder
    /// drag sends no mouse moves to reset that.
    pub(crate) file_drag: Option<Vec<std::path::PathBuf>>,
    /// The row lifted by a touch hold, while the finger holds it.
    pub(crate) touch_lift: Option<TouchLift>,
    /// The viewport is ≤720px wide (BlockNode.svelte / Editor.svelte
    /// `@media (max-width: 720px)`), as of the last frame.
    pub(crate) narrow: bool,
    // Geometry of the last frame.
    pub(crate) layouts: Layouts,
    pub(crate) rows: Rows,
    pub(crate) bot_strips: Rows,
    pub(crate) bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// The part of the editor its scrolling ancestor shows (the paint mask):
    /// edge autoscroll aims its wheel events here, like scrolling the
    /// nearest overflow ancestor directly.
    pub(crate) visible: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Each root block's height as last drawn: long pages build only the
    /// blocks in and near the window, and these size the space for the rest.
    pub(crate) block_heights: Rc<std::cell::RefCell<HashMap<String, f32>>>,
    /// Embed stills and titles by URL (`None` while the fetch runs).
    pub(crate) posters: std::cell::RefCell<HashMap<String, Option<crate::embed::Poster>>>,
    // Spellcheck and Tool code lint.
    pub(crate) misspelled: HashMap<String, Vec<std::ops::Range<usize>>>,
    pub(crate) spell_task: Option<Task<()>>,
    pub(crate) lint: HashMap<String, Vec<LintProblem>>,
    pub(crate) lint_task: Option<Task<()>>,
    pub(crate) lint_run: u64,
    // Caret blink.
    pub(crate) caret_on: bool,
    pub(crate) blink: Option<Task<()>>,
    /// Code blocks whose "Copy" just ran (label shows "Copied").
    pub(crate) copied: HashSet<String>,
    pub(crate) file_views: HashMap<String, Entity<crate::file_block::FileBlock>>,
    /// The object's present, non-hidden properties for the slash menu: (key, name, format).
    pub(crate) present_relations: Vec<(String, String, String)>,
    refresh_pending: bool,
    refresh_again: bool,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    /// The editor for `object_id`'s body. `read_only` shows the body and never edits it.
    pub fn new(
        object_id: String,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let store = Store::global(cx);
        let mut subs =
            vec![cx.subscribe(
                &store,
                |this: &mut Self, _, event: &StoreEvent, cx| match event {
                    StoreEvent::Committed(id) if *id == this.object_id => {
                        this.refresh_when_settled(cx)
                    }
                    StoreEvent::Reconnected => this.refresh_when_settled(cx),
                    _ => {}
                },
            )];
        // Re-render when summaries/relations/types change (link rows, relation blocks, menus).
        subs.push(cx.observe(&store, |_, _, cx| cx.notify()));
        subs.push(cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.has_focus = true;
            this.restart_blink(cx);
            cx.notify();
        }));
        subs.push(cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.has_focus = false;
            if let Some(id) = this.active.clone() {
                this.flush_save(&id, cx);
            }
            this.toolbar = None;
            this.sync_texts();
            cx.notify();
        }));
        // Save typing when the editor goes away (the web's pagehide / unmount flush).
        subs.push(cx.on_release(|this, cx| {
            let ids: Vec<String> = this.dirty.iter().cloned().collect();
            for id in ids {
                if let Some(content) = this.unsaved_content(&id) {
                    let oid = this.object_id.clone();
                    api::note::block_update(&oid, &id, &content, cx).detach_and_log_err(cx);
                }
            }
        }));
        let writes = WriteQueue::new(cx);
        let mut editor = Self {
            object_id,
            read_only,
            focus_handle,
            object: None,
            blocks: Vec::new(),
            load_error: None,
            texts: HashMap::new(),
            active: None,
            has_focus: false,
            focus_pending: false,
            focus_request: None,
            history: History::default(),
            writes,
            dirty: HashSet::new(),
            save_timers: HashMap::new(),
            structural_busy: false,
            structural_queue: VecDeque::new(),
            selected: Vec::new(),
            last_focused: String::new(),
            drag_sel: None,
            sel_rect: None,
            autoscroll: None,
            text_drag: None,
            toolbar: None,
            slash: None,
            block_menu: None,
            link_picker: None,
            link_prompt: None,
            property_suggest: None,
            turn_into: None,
            paste_menu: None,
            spell_menu: None,
            file_notice: String::new(),
            dragging: None,
            drop_hint: None,
            last_valid_drop: None,
            drag_rects: Vec::new(),
            drag_point: Point::default(),
            drag_scroll: None,
            file_drag: None,
            touch_lift: None,
            narrow: false,
            layouts: Layouts::default(),
            rows: Rows::default(),
            bot_strips: Rows::default(),
            bounds: Rc::new(std::cell::Cell::new(Bounds::default())),
            visible: Rc::new(std::cell::Cell::new(Bounds::default())),
            block_heights: Rc::new(std::cell::RefCell::new(HashMap::new())),
            posters: Default::default(),
            misspelled: HashMap::new(),
            spell_task: None,
            lint: HashMap::new(),
            lint_task: None,
            lint_run: 0,
            caret_on: true,
            blink: None,
            copied: HashSet::new(),
            file_views: HashMap::new(),
            present_relations: Vec::new(),
            refresh_pending: false,
            refresh_again: false,
            _subscriptions: subs,
        };
        editor.load(cx);
        crate::spellcheck::ensure_loaded(cx);
        editor
    }

    /// Put the caret at the start of the body (Enter in the title): into the
    /// first line when it is an empty text line, else into a fresh empty
    /// line added above whatever comes first (nothing existing is edited).
    pub fn focus_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let roots = self.root_ids();
        let first = roots.first().and_then(|id| self.block(id)).cloned();
        if let Some(first) = &first
            && let Some(t) = &first.content.text
            && t.text.is_empty()
            && t.style != style::TITLE
            && t.style != style::CODE
        {
            self.focus_block(&first.id, 0, cx);
            window.focus(&self.focus_handle, cx);
            return;
        }
        let id = new_id();
        let block = text_block(&id, "", style::PARAGRAPH);
        let (target, position) = match &first {
            Some(f) => (f.id.clone(), pos::TOP),
            None => (String::new(), pos::BOTTOM),
        };
        let add = self.w_block_add(block, &target, position, cx);
        self.focus_request = Some((id, 0));
        cx.spawn(async move |this, cx| {
            let _ = add.await;
            let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── data

    fn load(&mut self, cx: &mut Context<Self>) {
        let fetch = Store::global(cx).read(cx).fetch_object(&self.object_id, cx);
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(object) => this.adopt(object, cx),
                    Err(error) => this.load_error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fold a fresh copy of the object into the one on screen (reconcile.ts):
    /// blocks are replaced, but the focused block keeps its live text.
    pub(crate) fn adopt(&mut self, fresh: Object, cx: &mut Context<Self>) {
        if fresh.id != self.object_id {
            return;
        }
        self.blocks = fresh.blocks.clone();
        self.object = Some(Object {
            blocks: Vec::new(),
            ..fresh
        });
        self.load_error = None;
        self.sync_texts();
        if let Some(id) = &self.active
            && !self.texts.contains_key(id)
        {
            self.active = None;
        }
        self.selected
            .retain(|id| self.blocks.iter().any(|b| b.id == *id));
        self.schedule_spell(cx);
        self.schedule_lint(cx);
        cx.notify();
    }

    /// Bring every text's live state in line with the block list
    /// ([`sync_live_texts`]); the block holding the caret, while the editor
    /// has focus, is the one being edited.
    pub(crate) fn sync_texts(&mut self) {
        let editing = if self.has_focus {
            self.active.as_deref()
        } else {
            None
        };
        sync_live_texts(&self.blocks, &mut self.texts, editing);
    }

    /// One more read of the object once typing has saved and writes landed
    /// (`refreshWhenSettled`): deferred, never dropped, bursts collapse.
    pub(crate) fn refresh_when_settled(&mut self, cx: &mut Context<Self>) {
        if self.refresh_pending {
            self.refresh_again = true;
            return;
        }
        self.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            loop {
                let _ = this.update(cx, |this, _| this.refresh_again = false);
                // settled(): no typing waiting to save and every write landed.
                loop {
                    let Ok(idle) = this.update(cx, |this, cx| this.writes.idle(cx)) else {
                        return;
                    };
                    idle.await;
                    let Ok(dirty) = this.update(cx, |this, _| !this.dirty.is_empty()) else {
                        return;
                    };
                    if !dirty {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                }
                let Ok(raced) = this.update(cx, |this, cx| this.read_unless_raced(cx)) else {
                    return;
                };
                let ok = raced.await;
                let Ok(again) = this.update(cx, |this, _| {
                    if !ok {
                        this.refresh_again = true;
                    }
                    this.refresh_again
                }) else {
                    return;
                };
                if !again {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.refresh_pending = false);
        })
        .detach();
    }

    /// Writes issued so far (unsaved typing counts as one more): a read taken
    /// before this changes is stale.
    pub(crate) fn write_mark(&self) -> usize {
        self.writes.issued() + usize::from(!self.dirty.is_empty())
    }

    /// Re-read the object; false when the read raced a newer local edit and was dropped.
    fn read_unless_raced(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let mark = self.write_mark();
        let fetch = Store::global(cx).read(cx).fetch_object(&self.object_id, cx);
        cx.spawn(async move |this, cx| {
            let Ok(fresh) = fetch.await else { return true };
            this.update(cx, |this, cx| {
                if this.write_mark() != mark {
                    return false;
                }
                this.adopt(fresh, cx);
                true
            })
            .unwrap_or(true)
        })
    }

    /// Persist every pending edit, wait for the writes, re-read server state,
    /// then honor a pending caret request (`refresh`).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.flush_all(cx);
        let idle = self.writes.idle(cx);
        cx.spawn(async move |this, cx| {
            idle.await;
            let Ok(read) = this.update(cx, |this, cx| this.read_unless_raced(cx)) else {
                return;
            };
            read.await;
            let _ = this.update(cx, |this, cx| {
                if let Some((id, offset)) = this.focus_request.take() {
                    this.focus_block(&id, offset, cx);
                }
                cx.notify();
            });
        })
    }

    // ───────────────────────────────────────────── derived

    pub(crate) fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub(crate) fn text_of(&self, id: &str) -> Option<&TextContent> {
        self.block(id).and_then(|b| b.content.text.as_ref())
    }

    pub(crate) fn style_of(&self, id: &str) -> Option<u32> {
        self.text_of(id).map(|t| t.style)
    }

    /// Body roots: blocks nobody lists, minus the discussion envelope and mailbox internals.
    pub(crate) fn root_ids(&self) -> Vec<String> {
        let referenced: HashSet<&str> = self
            .blocks
            .iter()
            .flat_map(|b| b.children_ids.iter().map(String::as_str))
            .collect();
        self.blocks
            .iter()
            .filter(|b| {
                if referenced.contains(b.id.as_str()) || b.id == "__content__" {
                    return false;
                }
                let kind = b
                    .content
                    .custom
                    .as_ref()
                    .map(|c| c.content_type.as_str())
                    .unwrap_or("");
                !matches!(
                    kind,
                    "discussion" | "agent_message" | "message_delivery" | "message_processing"
                )
            })
            .map(|b| b.id.clone())
            .collect()
    }

    /// Blocks in document order (tree DFS).
    pub(crate) fn flat_ids(&self) -> Vec<String> {
        let by_id: HashMap<&str, &Block> = self.blocks.iter().map(|b| (b.id.as_str(), b)).collect();
        let mut out = Vec::new();
        fn walk(by_id: &HashMap<&str, &Block>, id: &str, out: &mut Vec<String>) {
            let Some(b) = by_id.get(id) else { return };
            out.push(id.to_string());
            for c in &b.children_ids {
                walk(by_id, c, out);
            }
        }
        for id in self.root_ids() {
            walk(&by_id, &id, &mut out);
        }
        out
    }

    /// Text blocks in document order, for prev/next navigation and merge.
    pub(crate) fn flat_text(&self) -> Vec<String> {
        self.flat_ids()
            .into_iter()
            .filter(|id| self.text_of(id).is_some())
            .collect()
    }

    pub(crate) fn parent_id_of(&self, id: &str) -> String {
        doc::parent_of(&self.blocks, id)
            .map(|b| b.id.clone())
            .unwrap_or_default()
    }

    /// Ids from `set` whose ancestors are not in it (subtrees act whole).
    pub(crate) fn topmost_of(&self, set: &HashSet<String>) -> Vec<String> {
        let mut parent: HashMap<&str, &str> = HashMap::new();
        for b in &self.blocks {
            for c in &b.children_ids {
                parent.insert(c.as_str(), b.id.as_str());
            }
        }
        self.flat_ids()
            .into_iter()
            .filter(|id| {
                if !set.contains(id) {
                    return false;
                }
                let mut p = parent.get(id.as_str()).copied();
                while let Some(pid) = p {
                    if set.contains(pid) {
                        return false;
                    }
                    p = parent.get(pid).copied();
                }
                true
            })
            .collect()
    }

    pub(crate) fn topmost_selected(&self) -> Vec<String> {
        let set: HashSet<String> = self.selected.iter().cloned().collect();
        self.topmost_of(&set)
    }

    /// The live text and marks of a block (`fromDom`).
    pub(crate) fn live(&self, id: &str) -> (String, Vec<Span>) {
        match self.texts.get(id) {
            Some(t) => (t.text.clone(), t.spans.clone()),
            None => match self.text_of(id) {
                Some(t) => (
                    t.text.clone(),
                    marks::spans_from_marks(&t.text, t.marks.as_deref().unwrap_or(&[])),
                ),
                None => (String::new(), Vec::new()),
            },
        }
    }

    /// `contentFor`: the block's text content with new text/marks, keeping style, checked and color.
    pub(crate) fn content_for(&self, id: &str, text: &str, spans: &[Span]) -> BlockContent {
        let cur = self.text_of(id);
        text_content(
            text,
            spans,
            cur.map_or(style::PARAGRAPH, |t| t.style),
            cur.and_then(|t| t.checked).unwrap_or(false),
            cur.and_then(|t| t.color.clone()).unwrap_or_default(),
        )
    }

    /// Rewrite a block's live text (the web's direct `el.innerHTML = ...`).
    pub(crate) fn set_live(&mut self, id: &str, text: String, spans: Vec<Span>) {
        match self.texts.get_mut(id) {
            Some(t) => t.set_content(text, spans),
            None => {
                self.texts
                    .insert(id.to_string(), TextState::new(text, spans));
            }
        }
    }

    pub(crate) fn object_fields(&self) -> roostr_core::types::Fields {
        self.object
            .as_ref()
            .map(|o| o.fields.clone())
            .unwrap_or_default()
    }

    pub(crate) fn space_id(&self, cx: &App) -> String {
        roostr_core::relations::object_space_id(Store::global(cx).read(cx), &self.object_fields())
    }

    // ───────────────────────────────────────────── commit & undo

    /// One user action: apply its ops now, remember how to undo it, persist
    /// the same ops in order. A failed write re-reads server truth.
    pub(crate) fn commit(&mut self, ops: Vec<Op>, record: bool, cx: &mut Context<Self>) -> Vec<Op> {
        let inverse = doc::apply_all(&mut self.blocks, &ops);
        if record {
            self.history.record(inverse.clone());
        }
        for op in ops {
            let write = self.op_write(op, cx);
            cx.spawn(async move |this, cx| {
                if let Err(error) = write.await {
                    log::error!("[editor] write failed, restoring server state: {error:#}");
                    let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
                }
            })
            .detach();
        }
        self.sync_texts();
        self.schedule_spell(cx);
        self.schedule_lint(cx);
        cx.notify();
        inverse
    }

    /// Persist `op` through the write queue (`opWrite`).
    fn op_write(&self, op: Op, cx: &mut Context<Self>) -> Task<Result<JsonMap>> {
        match op {
            Op::Content { id, content } => self.w_block_update(&id, content, cx),
            Op::Insert { block, at } => {
                if at.target.is_empty() {
                    self.w_block_add(block, "", pos::NONE, cx)
                } else {
                    self.w_block_add(block, &at.target, at.position, cx)
                }
            }
            Op::Remove { id } => self.w_block_remove(&id, cx),
            Op::Move { id, to } => {
                if to.target.is_empty() {
                    self.w_block_move(&id, "", pos::BOTTOM, cx)
                } else {
                    self.w_block_move(&id, &to.target, to.position, cx)
                }
            }
        }
    }

    /// Undo (or redo) the last action: pending typing first becomes its own step.
    pub(crate) fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
        self.flush_all(cx);
        let tx = if redo {
            self.history.take_redo()
        } else {
            self.history.take_undo()
        };
        let Some(tx) = tx else { return };
        // Undo restores content into blocks; the focused block shows it too.
        let mut caret_block = String::new();
        let inverse = doc::apply_all(&mut self.blocks, &tx);
        for op in &tx {
            let id = match op {
                Op::Insert { block, .. } => block.id.clone(),
                Op::Content { id, .. } | Op::Remove { id } | Op::Move { id, .. } => id.clone(),
            };
            if let Op::Content { id, content } = op
                && let Some(t) = &content.text
            {
                let spans = marks::spans_from_marks(&t.text, t.marks.as_deref().unwrap_or(&[]));
                self.set_live(id, t.text.clone(), spans);
            }
            if !matches!(op, Op::Remove { .. }) && self.block(&id).is_some() {
                caret_block = id;
            }
        }
        for op in tx {
            let write = self.op_write(op, cx);
            cx.spawn(async move |this, cx| {
                if let Err(error) = write.await {
                    log::error!("[editor] write failed, restoring server state: {error:#}");
                    let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
                }
            })
            .detach();
        }
        if redo {
            self.history.push_undo(inverse);
        } else {
            self.history.push_redo(inverse);
        }
        self.sync_texts();
        if !caret_block.is_empty() && self.texts.contains_key(&caret_block) {
            let len = self.texts[&caret_block].text.len();
            self.focus_block(&caret_block, len, cx);
        }
        cx.notify();
    }

    // ───────────────────────────────────────────── saves

    /// Supersede any pending debounced save (structural mutations write their own truth).
    pub(crate) fn cancel_pending(&mut self, id: &str) {
        self.save_timers.remove(id);
        self.dirty.remove(id);
    }

    pub(crate) fn schedule_save(&mut self, id: &str, cx: &mut Context<Self>) {
        self.dirty.insert(id.to_string());
        let block = id.to_string();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            let _ = this.update(cx, |this, cx| this.flush_save(&block, cx));
        });
        self.save_timers.insert(id.to_string(), timer);
    }

    /// Save every block with unsaved typing now.
    pub(crate) fn flush_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.dirty.iter().cloned().collect();
        for id in ids {
            self.flush_save(&id, cx);
        }
    }

    /// What a dirty block would save, if it differs from state.
    fn unsaved_content(&self, id: &str) -> Option<BlockContent> {
        let live = self.texts.get(id)?;
        let cur = self.text_of(id)?;
        let marks = marks::marks_from_spans(&live.text, &live.spans);
        let stored = marks::normalized_marks(&cur.text, cur.marks.as_deref().unwrap_or(&[]));
        if cur.text == live.text && stored == marks {
            return None;
        }
        Some(self.content_for(id, &live.text, &live.spans))
    }

    /// The typing since the last save is one undo step; state follows the live text.
    pub(crate) fn flush_save(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.dirty.contains(id) {
            return;
        }
        self.save_timers.remove(id);
        if self.block(id).is_none() || !self.texts.contains_key(id) {
            return;
        }
        self.dirty.remove(id);
        if let Some(content) = self.unsaved_content(id) {
            self.commit(
                vec![Op::Content {
                    id: id.to_string(),
                    content,
                }],
                true,
                cx,
            );
        }
    }

    // ───────────────────────────────────────────── caret

    /// Move the caret into `id` at byte `offset` (`focusSync`).
    pub(crate) fn focus_block(&mut self, id: &str, offset: usize, cx: &mut Context<Self>) {
        if !self.texts.contains_key(id) {
            if self.block(id).is_some() {
                self.sync_texts();
            }
            if !self.texts.contains_key(id) {
                self.focus_request = Some((id.to_string(), offset));
                return;
            }
        }
        if let Some(old) = self.active.clone()
            && old != id
        {
            self.flush_save(&old, cx);
        }
        self.active = Some(id.to_string());
        if let Some(t) = self.texts.get_mut(id) {
            t.move_to(offset);
        }
        self.last_focused = id.to_string();
        self.focus_pending = true;
        self.restart_blink(cx);
        cx.notify();
    }

    pub(crate) fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_on = true;
        self.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(530))
                    .await;
                let Ok(()) = this.update(cx, |this, cx| {
                    this.caret_on = !this.caret_on;
                    cx.notify();
                }) else {
                    return;
                };
            }
        }));
    }

    // ───────────────────────────────────────────── queued writes

    pub(crate) fn w_block_add(
        &self,
        block: Block,
        target: &str,
        position: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let target = target.to_string();
        self.writes.run(cx, move |cx| {
            api::note::block_add(&oid, &block, &target, position, cx)
        })
    }

    pub(crate) fn w_block_update(
        &self,
        id: &str,
        content: BlockContent,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes.run(cx, move |cx| {
            api::note::block_update(&oid, &id, &content, cx)
        })
    }

    pub(crate) fn w_block_move(
        &self,
        id: &str,
        target: &str,
        position: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        let target = target.to_string();
        self.writes.run(cx, move |cx| {
            api::note::block_move(&oid, &id, &target, position, cx)
        })
    }

    pub(crate) fn w_block_remove(&self, id: &str, cx: &mut Context<Self>) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes
            .run(cx, move |cx| api::note::block_remove(&oid, &id, cx))
    }

    pub(crate) fn w_set_attrs(
        &self,
        id: &str,
        align: Option<u32>,
        background: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes.run(cx, move |cx| {
            api::note::block_set_attrs(&oid, &id, align, background.as_deref(), cx)
        })
    }

    pub(crate) fn w_set_field(
        &self,
        key: &str,
        value: Value,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let key = key.to_string();
        self.writes
            .run(cx, move |cx| api::note::set_field(&oid, &key, value, cx))
    }

    /// `table.create` in the write lane; resolves to the result map (`id`).
    pub(crate) fn w_table_create(
        &self,
        target: &str,
        position: u32,
        rows: u32,
        cols: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let target = target.to_string();
        self.writes.run(cx, move |cx| {
            let task = api::table::create(&oid, &target, position, rows, cols, cx);
            cx.background_spawn(async move {
                let id = task.await?;
                let mut map = JsonMap::new();
                map.insert("id".into(), serde_json::Value::String(id));
                Ok(map)
            })
        })
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Bring every text's live state in line with the block list: a fresh read
/// of the object (a remote edit, a refresh after a write) re-renders each
/// block whose content differs - except `editing`, the block whose text has
/// focus, which keeps what is on screen: it is the person's, possibly
/// unsaved, and their caret is in it. That is BlockNode.svelte's render
/// effect (`if (document.activeElement !== textEl) textEl.innerHTML = …`):
/// the focused block shows a remote edit only once it loses focus. Texts of
/// blocks that are gone are dropped; new blocks get one.
fn sync_live_texts(
    blocks: &[Block],
    texts: &mut HashMap<String, TextState>,
    editing: Option<&str>,
) {
    let mut seen = HashSet::new();
    for b in blocks {
        let Some(t) = &b.content.text else { continue };
        seen.insert(b.id.as_str());
        if editing == Some(b.id.as_str()) && texts.contains_key(&b.id) {
            continue;
        }
        let spans = marks::spans_from_marks(&t.text, t.marks.as_deref().unwrap_or(&[]));
        match texts.get_mut(&b.id) {
            Some(live) => {
                if live.text != t.text || live.spans != spans {
                    live.set_content(t.text.clone(), spans);
                }
            }
            None => {
                texts.insert(b.id.clone(), TextState::new(t.text.clone(), spans));
            }
        }
    }
    texts.retain(|id, _| seen.contains(id.as_str()));
}

/// A fresh block id.
pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `{ id, childrenIds: [], content: { text: { text, style } } }`.
pub(crate) fn text_block(id: &str, text: &str, style: u32) -> Block {
    Block {
        id: id.to_string(),
        content: BlockContent {
            text: Some(TextContent {
                text: text.to_string(),
                style,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A text content with every field set (as the web writes them).
pub(crate) fn text_content(
    text: &str,
    spans: &[Span],
    style: u32,
    checked: bool,
    color: String,
) -> BlockContent {
    BlockContent {
        text: Some(TextContent {
            text: text.to_string(),
            style,
            marks: Some(marks::marks_from_spans(text, spans)),
            checked: Some(checked),
            color: Some(color),
        }),
        ..Default::default()
    }
}

/// A custom block's content.
pub(crate) fn custom_content(content_type: &str, meta: &[(&str, &str)]) -> BlockContent {
    BlockContent {
        custom: Some(roostr_core::types::CustomContent {
            content_type: content_type.to_string(),
            meta: Some(
                meta.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            data: None,
        }),
        ..Default::default()
    }
}

pub(crate) fn place(target: &str, position: u32) -> Place {
    Place::new(target, position)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui_kit's glob exports its own `test` attribute.
    use super::{Block, HashMap, style, sync_live_texts, text_block};

    fn set_text(blocks: &mut [Block], id: &str, text: &str) {
        let block = blocks.iter_mut().find(|b| b.id == id).unwrap();
        block.content.text.as_mut().unwrap().text = text.to_string();
    }

    #[test]
    fn remote_edits_reach_every_block_but_the_one_being_edited() {
        let mut blocks = vec![
            text_block("a", "alpha", style::PARAGRAPH),
            text_block("b", "beta", style::PARAGRAPH),
        ];
        let mut texts = HashMap::new();
        sync_live_texts(&blocks, &mut texts, None);
        assert_eq!(texts["a"].text, "alpha");
        assert_eq!(texts["b"].text, "beta");

        // The caret is in "a" and the person has typed there (unsaved);
        // another device edits both blocks and adds a third.
        let typed = texts.get_mut("a").unwrap();
        typed.set_content("alpha, typed here".into(), Vec::new());
        typed.move_to(typed.text.len());
        set_text(&mut blocks, "a", "alpha, edited elsewhere");
        set_text(&mut blocks, "b", "beta, edited elsewhere");
        blocks.push(text_block("c", "gamma", style::PARAGRAPH));
        sync_live_texts(&blocks, &mut texts, Some("a"));
        assert_eq!(
            texts["a"].text, "alpha, typed here",
            "the focused block keeps its text"
        );
        assert_eq!(
            texts["a"].cursor(),
            "alpha, typed here".len(),
            "and its caret"
        );
        assert_eq!(texts["b"].text, "beta, edited elsewhere");
        assert_eq!(texts["c"].text, "gamma");

        // Once the block loses focus it shows what is stored.
        sync_live_texts(&blocks, &mut texts, None);
        assert_eq!(texts["a"].text, "alpha, edited elsewhere");

        // A block removed elsewhere goes, even the one that was focused.
        blocks.retain(|b| b.id != "a");
        sync_live_texts(&blocks, &mut texts, Some("a"));
        assert!(!texts.contains_key("a"));
        assert_eq!(texts.len(), 2);
    }
}
