//! The block editor for an object's body (`Editor.svelte`): document state,
//! the ordered write lane, undo, debounced saves and reconciling remote
//! changes. Keyboard and input live in `keys.rs`, block operations and menus
//! in `ops.rs`, drawing in `render.rs`.
//!
//! Every open body is an [`Editor`] in the [`Editors`] slot, keyed by object
//! id. Code that changes one takes it out of the slot for the call
//! ([`with_editor`]), so its methods keep `&mut self` beside `&mut State`;
//! async work re-enters the same way and finds nothing once the editor closed
//! (GPUI's "entity gone").

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use anyhow::Result;
use roostr_core::types::{Block, BlockContent, Object, TextContent, Value, pos, style};
use roostr_core::{Cx, JsonMap, State, StoreEvent, api};
use roostr_rt::Task;
use rune::TextEdit;

use crate::block_menu::BlockMenuState;
use crate::codelint::LintProblem;
use crate::doc::{self, History, Op, Place};
use crate::file_block::FileBlock;
use crate::link_picker::{LinkPickerState, LinkPromptState};
use crate::marks::{self, Span};
use crate::text::TextState;
use crate::write_queue::WriteQueue;

/// How long typing waits before it saves.
pub(crate) const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

/// Keys that reshape the tree, queued while an async structural op runs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StructuralKey {
    Enter,
    Tab { shift: bool },
}

/// Rubber-band selection in progress (Anytype selection/provider.tsx), in window points.
#[derive(Clone, Debug)]
pub(crate) struct DragSel {
    pub anchor: (f32, f32),
    pub base: Vec<String>,
    pub mode: SelMode,
    pub moved: bool,
    /// Started on the page background (a plain click there is a background click).
    pub background: bool,
    pub last: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelMode {
    Replace,
    Toggle,
    Remove,
}

/// The marks toolbar over a text selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Toolbar {
    pub block: String,
    pub from: usize,
    pub to: usize,
}

/// Slash menu: block, byte offset of the "/", live filter and the highlighted row.
#[derive(Clone, Debug)]
pub(crate) struct Slash {
    pub block: String,
    pub start: usize,
    pub filter: String,
    /// Index into the flat list of entries the filter leaves.
    pub selected: usize,
}

/// "Paste as" menu (Anytype editor/page.tsx onPasteUrl).
#[derive(Clone, Debug)]
pub(crate) struct PasteMenu {
    pub block: String,
    pub url: String,
    pub processor: Option<crate::embed::Processor>,
    pub at: usize,
}

/// Spelling suggestions for a misspelled word.
#[derive(Clone, Debug)]
pub(crate) struct SpellMenu {
    pub block: String,
    pub word: String,
    pub from: usize,
    pub to: usize,
    pub options: Vec<String>,
}

/// The block action menu, open on `block` (and the multi-selection it was opened in).
pub(crate) struct OpenBlockMenu {
    pub block: String,
    pub group: Option<Vec<String>>,
    /// Window point it opened at.
    pub at: (f32, f32),
    pub menu: BlockMenuState,
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
    pub bounds: [f32; 4],
    pub inner: bool,
    pub bot: bool,
    pub doc_end: bool,
}

/// Geometry of a block row as last laid out, in window points `[x, y, w, h]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowGeom {
    pub bounds: [f32; 4],
    /// Content starts after the gutter.
    pub gutter: bool,
}

/// Every open body editor, by object id, and the file pages.
#[derive(Default)]
pub struct Editors {
    pub(crate) map: HashMap<String, Editor>,
    /// What a field's accessor hands Rune for a block that is gone (never shown).
    pub(crate) scratch: TextEdit,
}

/// The block editor for an object's body (`Editor.svelte`).
pub struct Editor {
    pub(crate) object_id: String,
    pub(crate) read_only: bool,
    /// Fields and metadata of the object (blocks live in `blocks`).
    pub(crate) object: Option<Object>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) load_error: Option<String>,
    /// The live text of every text block (the web's contenteditable DOM).
    pub(crate) texts: HashMap<String, TextState>,
    /// The block holding the caret.
    pub(crate) active: Option<String>,
    /// Caret to place once the block exists (the web's focusRequest).
    pub(crate) focus_request: Option<(String, usize)>,
    /// The editor itself (not a text) should take the keyboard: a block selection.
    pub(crate) focus_self: bool,
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
    pub(crate) sel_rect: Option<[f32; 4]>,
    // Popups.
    pub(crate) toolbar: Option<Toolbar>,
    /// The selection the toolbar was dismissed for (Escape): it stays away until it changes.
    pub(crate) toolbar_dismissed: Option<Toolbar>,
    pub(crate) slash: Option<Slash>,
    pub(crate) block_menu: Option<OpenBlockMenu>,
    pub(crate) link_picker: Option<LinkPickerState>,
    pub(crate) link_prompt: Option<LinkPromptState>,
    pub(crate) property_suggest: Option<PropertySuggestOpen>,
    pub(crate) turn_into: Option<TurnIntoOpen>,
    pub(crate) paste_menu: Option<PasteMenu>,
    pub(crate) spell_menu: Option<SpellMenu>,
    pub(crate) file_notice: String,
    // Drag & drop.
    pub(crate) dragging: Option<String>,
    pub(crate) drop_hint: Option<DropHint>,
    pub(crate) last_valid_drop: Option<DropHint>,
    pub(crate) drag_rects: Vec<DropRect>,
    pub(crate) drag_point: (f32, f32),
    /// The handle was dragged, not clicked (its release must not open the block menu).
    pub(crate) handle_dragged: bool,
    /// An OS file drag is over the editor.
    pub(crate) file_drag: bool,
    /// The viewport is ≤720pt wide (BlockNode.svelte `@media (max-width: 720px)`).
    pub(crate) narrow: bool,
    // Geometry of the last layout.
    pub(crate) rows: HashMap<String, RowGeom>,
    pub(crate) bot_strips: HashMap<String, RowGeom>,
    /// The editor's own rect, in window points.
    pub(crate) bounds: [f32; 4],
    /// The editor's top with its page scrolled to the top (window points): with `bounds`
    /// it says how far the page is scrolled.
    pub(crate) unscrolled_top: f32,
    /// The window's height.
    pub(crate) window_h: f32,
    /// Each root block's height as last laid out: long pages build only the
    /// blocks in and near the window, and these size the space for the rest.
    pub(crate) block_heights: HashMap<String, f32>,
    /// Embed stills and titles by URL (`None` while the fetch runs).
    pub(crate) posters: HashMap<String, Option<crate::embed::Poster>>,
    // Spellcheck and Tool code lint.
    pub(crate) misspelled: HashMap<String, Vec<std::ops::Range<usize>>>,
    pub(crate) spell_task: Option<Task<()>>,
    pub(crate) lint: HashMap<String, Vec<LintProblem>>,
    pub(crate) lint_task: Option<Task<()>>,
    pub(crate) lint_run: u64,
    /// Code blocks whose "Copy" just ran (label shows "Copied").
    pub(crate) copied: HashSet<String>,
    pub(crate) file_blocks: HashMap<String, FileBlock>,
    /// The object's present, non-hidden properties for the slash menu: (key, name, format).
    pub(crate) present_relations: Vec<(String, String, String)>,
    refresh_pending: bool,
    refresh_again: bool,
}

/// The "New property" picker the slash menu opened on `block`.
pub(crate) struct PropertySuggestOpen {
    pub block: String,
    /// Window point it opened at.
    pub at: (f32, f32),
    pub state: roostr_properties::PropertySuggestState,
}

/// The "Turn into object" type picker over a selection.
pub(crate) struct TurnIntoOpen {
    pub at: Toolbar,
    /// Window point under the selection.
    pub pos: (f32, f32),
    pub state: roostr_properties::TypeSuggestState,
}

/// Run `f` on the open editor of `object_id` (taken out of its slot for the call).
pub(crate) fn with_editor<R>(
    s: &mut State,
    object_id: &str,
    f: impl FnOnce(&mut Editor, &mut State) -> R,
) -> Option<R> {
    let mut editor = s.slot::<Editors>().map.remove(object_id)?;
    let out = f(&mut editor, s);
    s.slot::<Editors>()
        .map
        .insert(object_id.to_string(), editor);
    Some(out)
}

/// [`with_editor`] from async code; `None` once the editor closed.
pub(crate) fn update_editor<R>(
    cx: &Cx,
    object_id: &str,
    f: impl FnOnce(&mut Editor, &mut State) -> R,
) -> Option<R> {
    cx.update(|s| with_editor(s, object_id, f))
}

/// The open editor of `object_id`, read-only.
pub(crate) fn editor<'a>(s: &'a State, object_id: &str) -> Option<&'a Editor> {
    s.get::<Editors>()?.map.get(object_id)
}

/// The `TextEdit` Rune edits for block `block` of `object_id`'s editor.
pub(crate) fn edit_of<'a>(s: &'a mut State, object_id: &str, block: &str) -> &'a mut TextEdit {
    let editors = s.slot::<Editors>();
    let present = editors
        .map
        .get(object_id)
        .is_some_and(|e| e.texts.contains_key(block));
    if present {
        &mut editors
            .map
            .get_mut(object_id)
            .and_then(|e| e.texts.get_mut(block))
            .expect("checked above")
            .edit
    } else {
        editors.scratch = TextEdit::new("");
        &mut editors.scratch
    }
}

/// `StoreEvent`s: a commit to an open body (or a reconnect) re-reads it once its writes settle.
pub(crate) fn on_store(s: &mut State, event: &StoreEvent) {
    let ids: Vec<String> = match event {
        StoreEvent::Committed(id) => s
            .get::<Editors>()
            .filter(|e| e.map.contains_key(id))
            .map(|_| vec![id.clone()])
            .unwrap_or_default(),
        StoreEvent::Reconnected => s
            .get::<Editors>()
            .map(|e| e.map.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for id in ids {
        with_editor(s, &id, |e, s| e.refresh_when_settled(s));
    }
}

impl Editor {
    /// The editor for `object_id`'s body. `read_only` shows the body and never edits it.
    pub(crate) fn new(object_id: String, read_only: bool, s: &mut State) -> Self {
        let writes = WriteQueue::new(s);
        let editor = Self {
            object_id,
            read_only,
            object: None,
            blocks: Vec::new(),
            load_error: None,
            texts: HashMap::new(),
            active: None,
            focus_request: None,
            focus_self: false,
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
            toolbar: None,
            toolbar_dismissed: None,
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
            drag_point: (0., 0.),
            handle_dragged: false,
            file_drag: false,
            narrow: false,
            rows: HashMap::new(),
            bot_strips: HashMap::new(),
            bounds: [0.; 4],
            unscrolled_top: 0.,
            window_h: 900.,
            block_heights: HashMap::new(),
            posters: HashMap::new(),
            misspelled: HashMap::new(),
            spell_task: None,
            lint: HashMap::new(),
            lint_task: None,
            lint_run: 0,
            copied: HashSet::new(),
            file_blocks: HashMap::new(),
            present_relations: Vec::new(),
            refresh_pending: false,
            refresh_again: false,
        };
        editor.load(s);
        crate::spellcheck::ensure_loaded(s);
        editor
    }

    /// The editor is going away: save typing (the web's pagehide / unmount
    /// flush); the writes already queued still land.
    pub(crate) fn close(self, s: &mut State) {
        for id in &self.dirty {
            if let Some(content) = self.unsaved_content(id) {
                api::note::block_update(&self.object_id, id, &content, s).detach_and_log_err(&s.rt);
            }
        }
        self.writes.finish();
    }

    /// Put the caret at the start of the body (Enter in the title): into the
    /// first line when it is an empty text line, else into a fresh empty
    /// line added above whatever comes first (nothing existing is edited).
    pub(crate) fn focus_start(&mut self, s: &mut State) {
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
            self.focus_block(&first.id, 0, s);
            return;
        }
        let id = new_id();
        let block = text_block(&id, "", style::PARAGRAPH);
        let (target, position) = match &first {
            Some(f) => (f.id.clone(), pos::TOP),
            None => (String::new(), pos::BOTTOM),
        };
        let add = self.w_block_add(block, &target, position, s);
        self.focus_request = Some((id, 0));
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let _ = add.await;
            update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── data

    fn load(&self, s: &mut State) {
        let fetch = s.store.fetch_object(&self.object_id);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let result = fetch.await;
            update_editor(&cx, &oid, |e, s| match result {
                Ok(object) => e.adopt(object, s),
                Err(error) => e.load_error = Some(error.to_string()),
            });
        })
        .detach();
    }

    /// Fold a fresh copy of the object into the one on screen (reconcile.ts):
    /// blocks are replaced, but the focused block keeps its live text.
    pub(crate) fn adopt(&mut self, fresh: Object, s: &mut State) {
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
        self.update_present_relations(s);
        self.sync_file_blocks(s);
        self.ensure_posters(s);
        self.schedule_spell(s);
        self.schedule_lint(s);
    }

    /// The block whose text has the keyboard right now.
    pub(crate) fn editing(&self) -> Option<&str> {
        self.active
            .as_deref()
            .filter(|id| self.texts.get(*id).is_some_and(TextState::focused))
    }

    /// Bring every text's live state in line with the block list
    /// ([`sync_live_texts`]); the block holding the caret, while it has the
    /// keyboard, is the one being edited.
    pub(crate) fn sync_texts(&mut self) {
        let editing = self.editing().map(str::to_string);
        sync_live_texts(&self.blocks, &mut self.texts, editing.as_deref());
    }

    /// One more read of the object once typing has saved and writes landed
    /// (`refreshWhenSettled`): deferred, never dropped, bursts collapse.
    pub(crate) fn refresh_when_settled(&mut self, s: &mut State) {
        if self.refresh_pending {
            self.refresh_again = true;
            return;
        }
        self.refresh_pending = true;
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            loop {
                if update_editor(&cx, &oid, |e, _| e.refresh_again = false).is_none() {
                    return;
                }
                // settled(): no typing waiting to save and every write landed.
                loop {
                    let Some(idle) = update_editor(&cx, &oid, |e, s| e.writes.idle(s)) else {
                        return;
                    };
                    idle.await;
                    let Some(dirty) = update_editor(&cx, &oid, |e, _| !e.dirty.is_empty()) else {
                        return;
                    };
                    if !dirty {
                        break;
                    }
                    cx.timer(Duration::from_millis(150)).await;
                }
                let Some(raced) = update_editor(&cx, &oid, |e, s| e.read_unless_raced(s)) else {
                    return;
                };
                let ok = raced.await;
                let Some(again) = update_editor(&cx, &oid, |e, _| {
                    if !ok {
                        e.refresh_again = true;
                    }
                    e.refresh_again
                }) else {
                    return;
                };
                if !again {
                    break;
                }
            }
            update_editor(&cx, &oid, |e, _| e.refresh_pending = false);
        })
        .detach();
    }

    /// Writes issued so far (unsaved typing counts as one more): a read taken
    /// before this changes is stale.
    pub(crate) fn write_mark(&self) -> usize {
        self.writes.issued() + usize::from(!self.dirty.is_empty())
    }

    /// Re-read the object; false when the read raced a newer local edit and was dropped.
    fn read_unless_raced(&mut self, s: &mut State) -> Task<bool> {
        let mark = self.write_mark();
        let fetch = s.store.fetch_object(&self.object_id);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let Ok(fresh) = fetch.await else { return true };
            update_editor(&cx, &oid, |e, s| {
                if e.write_mark() != mark {
                    return false;
                }
                e.adopt(fresh, s);
                true
            })
            .unwrap_or(true)
        })
    }

    /// Persist every pending edit, wait for the writes, re-read server state,
    /// then honor a pending caret request (`refresh`).
    pub(crate) fn refresh(&mut self, s: &mut State) -> Task<()> {
        self.flush_all(s);
        let idle = self.writes.idle(s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            idle.await;
            let Some(read) = update_editor(&cx, &oid, |e, s| e.read_unless_raced(s)) else {
                return;
            };
            read.await;
            update_editor(&cx, &oid, |e, s| {
                if let Some((id, offset)) = e.focus_request.take() {
                    e.focus_block(&id, offset, s);
                }
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
            Some(t) => (t.text().to_string(), t.spans.clone()),
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

    pub(crate) fn space_id(&self, s: &State) -> String {
        roostr_core::relations::object_space_id(&s.store, &self.object_fields())
    }

    /// The object's present, non-hidden properties (the slash menu lists them).
    pub(crate) fn update_present_relations(&mut self, s: &State) {
        let fields = self.object_fields();
        let space = roostr_core::relations::object_space_id(&s.store, &fields);
        self.present_relations =
            roostr_core::relations::space_relations(&s.store.relations, &space)
                .into_iter()
                .filter(|r| {
                    !r.hidden
                        && !roostr_core::relations::is_reserved_key(&r.key)
                        && fields.contains_key(&r.key)
                })
                .map(|r| (r.key.clone(), r.name.clone(), r.format.clone()))
                .collect();
    }

    // ───────────────────────────────────────────── commit & undo

    /// One user action: apply its ops now, remember how to undo it, persist
    /// the same ops in order. A failed write re-reads server truth.
    pub(crate) fn commit(&mut self, ops: Vec<Op>, record: bool, s: &mut State) -> Vec<Op> {
        let inverse = doc::apply_all(&mut self.blocks, &ops);
        if record {
            self.history.record(inverse.clone());
        }
        for op in ops {
            self.persist(op, s);
        }
        self.sync_texts();
        self.sync_file_blocks(s);
        self.ensure_posters(s);
        self.schedule_spell(s);
        self.schedule_lint(s);
        inverse
    }

    /// Write `op` in the lane; on failure re-read what the server has.
    fn persist(&mut self, op: Op, s: &mut State) {
        let write = self.op_write(op, s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            if let Err(error) = write.await {
                log::error!("[editor] write failed, restoring server state: {error:#}");
                update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
            }
        })
        .detach();
    }

    /// Persist `op` through the write queue (`opWrite`).
    fn op_write(&self, op: Op, s: &mut State) -> Task<Result<JsonMap>> {
        match op {
            Op::Content { id, content } => self.w_block_update(&id, content, s),
            Op::Insert { block, at } => {
                if at.target.is_empty() {
                    self.w_block_add(block, "", pos::NONE, s)
                } else {
                    self.w_block_add(block, &at.target, at.position, s)
                }
            }
            Op::Remove { id } => self.w_block_remove(&id, s),
            Op::Move { id, to } => {
                if to.target.is_empty() {
                    self.w_block_move(&id, "", pos::BOTTOM, s)
                } else {
                    self.w_block_move(&id, &to.target, to.position, s)
                }
            }
        }
    }

    /// Undo (or redo) the last action: pending typing first becomes its own step.
    pub(crate) fn undo_redo(&mut self, redo: bool, s: &mut State) {
        self.flush_all(s);
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
            self.persist(op, s);
        }
        if redo {
            self.history.push_undo(inverse);
        } else {
            self.history.push_redo(inverse);
        }
        self.sync_texts();
        self.sync_file_blocks(s);
        self.ensure_posters(s);
        if !caret_block.is_empty() && self.texts.contains_key(&caret_block) {
            let len = self.texts[&caret_block].text().len();
            self.focus_block(&caret_block, len, s);
        }
    }

    // ───────────────────────────────────────────── saves

    /// Supersede any pending debounced save (structural mutations write their own truth).
    pub(crate) fn cancel_pending(&mut self, id: &str) {
        self.save_timers.remove(id);
        self.dirty.remove(id);
    }

    pub(crate) fn schedule_save(&mut self, id: &str, s: &mut State) {
        self.dirty.insert(id.to_string());
        let block = id.to_string();
        let oid = self.object_id.clone();
        let timer = s.rt.spawn(async move |cx| {
            cx.timer(SAVE_DEBOUNCE).await;
            update_editor(&cx, &oid, |e, s| e.flush_save(&block, s));
        });
        self.save_timers.insert(id.to_string(), timer);
    }

    /// Save every block with unsaved typing now.
    pub(crate) fn flush_all(&mut self, s: &mut State) {
        let ids: Vec<String> = self.dirty.iter().cloned().collect();
        for id in ids {
            self.flush_save(&id, s);
        }
    }

    /// What a dirty block would save, if it differs from state.
    fn unsaved_content(&self, id: &str) -> Option<BlockContent> {
        let live = self.texts.get(id)?;
        let cur = self.text_of(id)?;
        let marks = marks::marks_from_spans(live.text(), &live.spans);
        let stored = marks::normalized_marks(&cur.text, cur.marks.as_deref().unwrap_or(&[]));
        if cur.text == live.text() && stored == marks {
            return None;
        }
        Some(self.content_for(id, live.text(), &live.spans))
    }

    /// The typing since the last save is one undo step; state follows the live text.
    pub(crate) fn flush_save(&mut self, id: &str, s: &mut State) {
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
                s,
            );
        }
    }

    // ───────────────────────────────────────────── caret

    /// Move the caret into `id` at byte `offset` (`focusSync`): its field
    /// takes the keyboard on the next frame.
    pub(crate) fn focus_block(&mut self, id: &str, offset: usize, s: &mut State) {
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
            self.flush_save(&old, s);
        }
        for (bid, t) in self.texts.iter_mut() {
            if bid != id {
                t.edit.focused = false;
            }
        }
        self.active = Some(id.to_string());
        self.focus_self = false;
        if let Some(t) = self.texts.get_mut(id) {
            t.move_to(offset);
            t.edit.focused = true;
        }
        self.last_focused = id.to_string();
    }

    /// Take the keyboard from every text (a block selection, Escape): the
    /// editor itself listens for keys.
    pub(crate) fn blur_texts(&mut self, s: &mut State) {
        if let Some(a) = self.active.take() {
            self.flush_save(&a, s);
        }
        for t in self.texts.values_mut() {
            t.edit.focused = false;
        }
        self.focus_self = true;
    }

    // ───────────────────────────────────────────── queued writes

    pub(crate) fn w_block_add(
        &self,
        block: Block,
        target: &str,
        position: u32,
        s: &State,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let target = target.to_string();
        self.writes.run(s, move |s| {
            api::note::block_add(&oid, &block, &target, position, s)
        })
    }

    pub(crate) fn w_block_update(
        &self,
        id: &str,
        content: BlockContent,
        s: &State,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes
            .run(s, move |s| api::note::block_update(&oid, &id, &content, s))
    }

    pub(crate) fn w_block_move(
        &self,
        id: &str,
        target: &str,
        position: u32,
        s: &State,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        let target = target.to_string();
        self.writes.run(s, move |s| {
            api::note::block_move(&oid, &id, &target, position, s)
        })
    }

    pub(crate) fn w_block_remove(&self, id: &str, s: &State) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes
            .run(s, move |s| api::note::block_remove(&oid, &id, s))
    }

    pub(crate) fn w_set_attrs(
        &self,
        id: &str,
        align: Option<u32>,
        background: Option<String>,
        s: &State,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let id = id.to_string();
        self.writes.run(s, move |s| {
            api::note::block_set_attrs(&oid, &id, align, background.as_deref(), s)
        })
    }

    pub(crate) fn w_set_field(&self, key: &str, value: Value, s: &State) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let key = key.to_string();
        self.writes
            .run(s, move |s| api::note::set_field(&oid, &key, value, s))
    }

    /// `table.create` in the write lane; resolves to the result map (`id`).
    pub(crate) fn w_table_create(
        &self,
        target: &str,
        position: u32,
        rows: u32,
        cols: u32,
        s: &State,
    ) -> Task<Result<JsonMap>> {
        let oid = self.object_id.clone();
        let target = target.to_string();
        self.writes.run(s, move |s| {
            let task = api::table::create(&oid, &target, position, rows, cols, s);
            s.rt.spawn(async move |_| {
                let id = task.await?;
                let mut map = JsonMap::new();
                map.insert("id".into(), serde_json::Value::String(id));
                Ok(map)
            })
        })
    }

    /// Await `tasks` in order, then `refresh`.
    pub(crate) fn await_then_refresh(&self, tasks: Vec<Task<Result<JsonMap>>>, s: &State) {
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            for t in tasks {
                if let Err(error) = t.await {
                    log::error!("[editor] write failed: {error:#}");
                }
            }
            update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
        })
        .detach();
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
pub(crate) fn sync_live_texts(
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
                if live.text() != t.text || live.spans != spans {
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
    use super::*;

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
        assert_eq!(texts["a"].text(), "alpha");
        assert_eq!(texts["b"].text(), "beta");

        // The caret is in "a" and the person has typed there (unsaved);
        // another device edits both blocks and adds a third.
        let typed = texts.get_mut("a").unwrap();
        typed.set_content("alpha, typed here".into(), Vec::new());
        typed.move_to("alpha, typed here".len());
        set_text(&mut blocks, "a", "alpha, edited elsewhere");
        set_text(&mut blocks, "b", "beta, edited elsewhere");
        blocks.push(text_block("c", "gamma", style::PARAGRAPH));
        sync_live_texts(&blocks, &mut texts, Some("a"));
        assert_eq!(
            texts["a"].text(),
            "alpha, typed here",
            "the focused block keeps its text"
        );
        assert_eq!(
            texts["a"].cursor(),
            "alpha, typed here".len(),
            "and its caret"
        );
        assert_eq!(texts["b"].text(), "beta, edited elsewhere");
        assert_eq!(texts["c"].text(), "gamma");

        // Once the block loses focus it shows what is stored.
        sync_live_texts(&blocks, &mut texts, None);
        assert_eq!(texts["a"].text(), "alpha, edited elsewhere");

        // A block removed elsewhere goes, even the one that was focused.
        blocks.retain(|b| b.id != "a");
        sync_live_texts(&blocks, &mut texts, Some("a"));
        assert!(!texts.contains_key("a"));
        assert_eq!(texts.len(), 2);
    }
}
