//! Block operations behind menus, paste and drop (`Editor.svelte`): slash
//! picks, files, URLs, relation/link blocks, turn into object, copy/cut/paste
//! of a block selection, checkboxes and toggles, spellcheck and Tool code
//! lint, and dragging blocks by their handle.

use std::time::Duration;

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::types::{Block, BlockContent, layout, mark, pos, style};
use roostr_rt::Task;
use rune::{ClipboardContent, Tx};

use crate::doc::{self, Op, Place};
use crate::editor::{
    DropHint, DropRect, Editor, PasteMenu, PropertySuggestOpen, SpellMenu, Toolbar, TurnIntoOpen,
    custom_content, new_id, place, text_block, text_content, update_editor,
};
use crate::embed;
use crate::files::{self, PickedFile};
use crate::marks::{self, Span};
use crate::slash_menu::SlashPick;
use crate::toggles;

/// The blocks a block selection last copied, with the plain text it wrote: a
/// paste of exactly that text is a paste of these blocks (GPUI kept them as
/// the clipboard item's metadata; Rune's clipboard carries text, HTML and RTF).
static COPIED: Mutex<Option<(String, Vec<Block>)>> = Mutex::new(None);

/// The copied blocks, when the clipboard still holds what the copy wrote.
fn clipboard_blocks(content: &ClipboardContent) -> Option<Vec<Block>> {
    let copied = COPIED.lock();
    let (text, blocks) = copied.as_ref()?;
    (content.text() == text && !blocks.is_empty()).then(|| blocks.clone())
}

impl Editor {
    // ───────────────────────────────────────────── slash

    /// Apply a slash pick: strip "/filter" from the block, then act.
    pub(crate) fn apply_slash(&mut self, pick: SlashPick, s: &mut State) {
        let Some(slash) = self.slash.take() else {
            return;
        };
        let id = slash.block.clone();
        let (text, spans) = self.live(&id);
        let end = (slash.start + 1 + slash.filter.len()).min(text.len());
        let mut clean_spans = spans.clone();
        marks::adjust_delete(&mut clean_spans, slash.start..end);
        let clean = format!("{}{}", &text[..slash.start.min(text.len())], &text[end..]);
        self.set_live(&id, clean.clone(), clean_spans.clone());
        if let Some(t) = self.texts.get_mut(&id) {
            t.move_to(slash.start);
        }
        self.cancel_pending(&id);
        let cur = self.text_of(&id).cloned();
        match pick {
            SlashPick::Table => {
                // Anytype default 3x3: replace an empty block, else keep the text and add below.
                let mut tasks = Vec::new();
                if clean.is_empty() {
                    tasks.push(self.w_table_create(&id, pos::REPLACE, 3, 3, s));
                } else {
                    tasks.push(self.w_block_update(
                        &id,
                        self.content_for(&id, &clean, &clean_spans),
                        s,
                    ));
                    tasks.push(self.w_table_create(&id, pos::BOTTOM, 3, 3, s));
                }
                self.await_then_refresh(tasks, s);
            }
            SlashPick::Divider(kind) => {
                let divider = custom_content("divider", &[("style", kind)]);
                let mut tasks = Vec::new();
                if clean.is_empty() {
                    let para = new_id();
                    tasks.push(self.w_block_update(&id, divider, s));
                    tasks.push(self.w_block_add(
                        text_block(&para, "", style::PARAGRAPH),
                        &id,
                        pos::BOTTOM,
                        s,
                    ));
                    self.focus_request = Some((para, 0));
                } else {
                    tasks.push(self.w_block_update(
                        &id,
                        self.content_for(&id, &clean, &clean_spans),
                        s,
                    ));
                    let block = Block {
                        id: new_id(),
                        content: divider,
                        ..Default::default()
                    };
                    tasks.push(self.w_block_add(block, &id, pos::BOTTOM, s));
                    self.focus_request = Some((id.clone(), slash.start));
                }
                self.await_then_refresh(tasks, s);
            }
            SlashPick::File => {
                let pick = files::pick_file(s);
                let oid = self.object_id.clone();
                s.rt.spawn(async move |cx| {
                    if let Some(file) = pick.await {
                        let Some(task) = update_editor(&cx, &oid, |e, s| {
                            e.insert_file(&id, file, clean, clean_spans, s)
                        }) else {
                            return;
                        };
                        task.await;
                    }
                    update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
                })
                .detach();
            }
            SlashPick::Relation(key) => {
                let task = self.insert_relation_block(&id, &clean, &clean_spans, &key, s);
                let oid = self.object_id.clone();
                s.rt.spawn(async move |cx| {
                    task.await;
                    update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
                })
                .detach();
            }
            SlashPick::LinkObject => {
                // Anytype: "Link to existing page" opens the searchObject menu.
                let at = self.anchor_under(&id);
                self.open_link_picker(&id, at, s);
            }
            SlashPick::PropertyAdd => {
                // Anytype: "New relation" opens the relationSuggest menu.
                let at = self.anchor_under(&id);
                self.open_property_suggest(&id, at, s);
            }
            SlashPick::Style(value) => {
                let content = text_content(
                    &clean,
                    &clean_spans,
                    value,
                    cur.as_ref().and_then(|t| t.checked).unwrap_or(false),
                    cur.as_ref()
                        .and_then(|t| t.color.clone())
                        .unwrap_or_default(),
                );
                let task = self.w_block_update(&id, content, s);
                self.focus_request = Some((id.clone(), slash.start));
                self.await_then_refresh(vec![task], s);
            }
        }
    }

    /// A window point just under block `id`'s row, for a picker opened on it.
    pub(crate) fn anchor_under(&self, id: &str) -> (f32, f32) {
        match self.rows.get(id) {
            Some(g) => (
                g.bounds[0] + if g.gutter { self.gutter_w() } else { 0. },
                g.bounds[1] + g.bounds[3] + 4.,
            ),
            None => (120., 126.),
        }
    }

    /// The drag rail's width: 48, 16 at ≤720pt.
    pub(crate) fn gutter_w(&self) -> f32 {
        if self.narrow { 16. } else { crate::GUTTER_W }
    }

    // ───────────────────────────────────────────── files

    /// Upload `file` and build the block content that points at it; `None`
    /// (after saying why) when the upload fails.
    pub(crate) fn file_content(
        &mut self,
        file: PickedFile,
        s: &mut State,
    ) -> Task<Option<BlockContent>> {
        let name = file.name.clone();
        let mime = file.mime.clone();
        let upload = files::upload_file(file, self.space_id(s), s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let result = upload.await;
            update_editor(&cx, &oid, |e, _| match result {
                Ok(up) => {
                    e.file_notice.clear();
                    Some(custom_content(
                        "file",
                        &[
                            ("fileId", &up.id),
                            ("hash", &up.hash),
                            ("mime", &mime),
                            ("name", &name),
                        ],
                    ))
                }
                Err(err) => {
                    e.file_notice = files::add_failure_text(&err, &name);
                    None
                }
            })
            .flatten()
        })
    }

    /// Bytes become a File object; the block points at it. An empty block
    /// becomes the file (a fresh paragraph below keeps somewhere to type);
    /// a block with text keeps it and the file lands below. Resolves to the
    /// file block's id ("" when the upload failed).
    pub(crate) fn insert_file(
        &mut self,
        id: &str,
        file: PickedFile,
        clean: String,
        spans: Vec<Span>,
        s: &mut State,
    ) -> Task<String> {
        let name = file.name.clone();
        let content = self.file_content(file, s);
        let id = id.to_string();
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let Some(block) = content.await else {
                return String::new();
            };
            let Some((tasks, file_id)) = update_editor(&cx, &oid, |e, s| {
                if clean.is_empty() {
                    let para = new_id();
                    let tasks = vec![
                        e.w_block_update(&id, block, s),
                        e.w_block_add(text_block(&para, "", style::PARAGRAPH), &id, pos::BOTTOM, s),
                    ];
                    e.focus_request = Some((para, 0));
                    (tasks, id.clone())
                } else {
                    let file_id = new_id();
                    let tasks = vec![
                        e.w_block_update(&id, e.content_for(&id, &clean, &spans), s),
                        e.w_block_add(
                            Block {
                                id: file_id.clone(),
                                content: block,
                                ..Default::default()
                            },
                            &id,
                            pos::BOTTOM,
                            s,
                        ),
                    ];
                    (tasks, file_id)
                }
            }) else {
                return String::new();
            };
            for t in tasks {
                if let Err(err) = t.await {
                    // The bytes are a File object by now; placing it on the page failed: say so.
                    update_editor(&cx, &oid, |e, _| {
                        e.file_notice = format!(
                            "{name} was uploaded (it's a File in this space), but it couldn't be placed on this page: {err:#}. Reload and try again."
                        );
                    });
                    return String::new();
                }
            }
            file_id
        })
    }

    /// Pasted image files become File blocks (the first in place of an empty line).
    pub(crate) fn paste_files(&mut self, id: &str, files: Vec<PickedFile>, s: &mut State) {
        let (text, spans) = self.live(id);
        self.cancel_pending(id);
        let id = id.to_string();
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let mut files = files.into_iter();
            let Some(first) = files.next() else { return };
            let Some(task) =
                update_editor(&cx, &oid, |e, s| e.insert_file(&id, first, text, spans, s))
            else {
                return;
            };
            let mut last = task.await;
            for file in files {
                if last.is_empty() {
                    break;
                }
                let Some(content) = update_editor(&cx, &oid, |e, s| e.file_content(file, s)) else {
                    return;
                };
                let Some(block) = content.await else { break };
                let next = new_id();
                let Some(add) = update_editor(&cx, &oid, |e, s| {
                    e.w_block_add(
                        Block {
                            id: next.clone(),
                            content: block,
                            ..Default::default()
                        },
                        &last,
                        pos::BOTTOM,
                        s,
                    )
                }) else {
                    return;
                };
                let _ = add.await;
                last = next;
            }
            update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
        })
        .detach();
    }

    /// Files dropped from another app: read them and drop them at the hinted
    /// line (else after the last root block).
    pub(crate) fn drop_external(&mut self, paths: Vec<std::path::PathBuf>, s: &mut State) {
        let hint = self.drop_hint.clone().or(self.last_valid_drop.clone());
        self.file_drag = false;
        self.drag_rects.clear();
        self.drop_hint = None;
        self.last_valid_drop = None;
        if self.read_only || paths.is_empty() {
            return;
        }
        let Some(target) = hint
            .as_ref()
            .map(|h| h.id.clone())
            .or_else(|| self.root_ids().last().cloned())
        else {
            return;
        };
        let position = hint.map_or(pos::BOTTOM, |h| h.position);
        #[cfg(target_family = "wasm")]
        let read = {
            let _ = paths;
            Task::ready(Vec::new())
        };
        #[cfg(not(target_family = "wasm"))]
        let read = s.rt.background(async move {
            paths
                .iter()
                .filter_map(|p| files::read_path(p).ok())
                .collect::<Vec<_>>()
        });
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let files = read.await;
            update_editor(&cx, &oid, |e, s| e.drop_files(files, target, position, s));
        })
        .detach();
    }

    /// Dropped files become File blocks at the drop line, in order; a file never nests inside a text block.
    pub(crate) fn drop_files(
        &mut self,
        files: Vec<PickedFile>,
        target: String,
        position: u32,
        s: &mut State,
    ) {
        let at = if position == pos::INNER_FIRST || position == pos::INNER {
            pos::BOTTOM
        } else {
            position
        };
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let mut prev = String::new();
            for file in files {
                let Some(content) = update_editor(&cx, &oid, |e, s| e.file_content(file, s)) else {
                    return;
                };
                let Some(block) = content.await else { break };
                let id = new_id();
                let (t, p) = if prev.is_empty() {
                    (target.clone(), at)
                } else {
                    (prev.clone(), pos::BOTTOM)
                };
                let Some(add) = update_editor(&cx, &oid, |e, s| {
                    e.w_block_add(
                        Block {
                            id: id.clone(),
                            content: block,
                            ..Default::default()
                        },
                        &t,
                        p,
                        s,
                    )
                }) else {
                    return;
                };
                let _ = add.await;
                prev = id;
            }
            update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── copy / cut / paste

    /// ⌘C / ⌘X with blocks selected: plain text for other apps, the exact blocks for Roostr.
    pub(crate) fn copy_blocks(&mut self, cut: bool, s: &mut State, tx: &mut Tx) {
        if self.selected.is_empty() {
            return;
        }
        let tops = self.topmost_selected();
        let slice = doc::copy_slice(&self.blocks, &tops);
        if slice.is_empty() {
            return;
        }
        let text = doc::slice_text(&slice);
        tx.copy(text.clone());
        *COPIED.lock() = Some((text, slice));
        if !cut || self.read_only {
            return;
        }
        self.selected.clear();
        for id in &tops {
            self.cancel_pending(id);
        }
        self.commit(
            tops.into_iter().map(|id| Op::Remove { id }).collect(),
            true,
            s,
        );
    }

    /// Paste with blocks selected and no caret: copied blocks land after the selection.
    pub(crate) fn paste_after_selection(&mut self, content: &ClipboardContent, s: &mut State) {
        if self.read_only {
            return;
        }
        if let Some(slice) = clipboard_blocks(content)
            && let Some(last) = self.topmost_selected().last().cloned()
        {
            self.paste_blocks(slice, &last, s);
        }
    }

    /// `onPasteText` into block `id`'s field: copied blocks paste as blocks,
    /// images as File blocks, one URL opens "Paste as" (or links the
    /// selection). True when taken; anything else Rune pastes as text.
    pub(crate) fn paste_into(
        &mut self,
        id: &str,
        content: &ClipboardContent,
        s: &mut State,
        _tx: &mut Tx,
    ) -> bool {
        if self.read_only {
            return true;
        }
        let cell = self.is_cell(id);
        if !cell {
            if let Some(slice) = clipboard_blocks(content) {
                self.paste_blocks(slice, id, s);
                return true;
            }
            if let Some(image) = content.image() {
                self.paste_files(id, vec![files::pasted_image(image)], s);
                return true;
            }
        }
        let text = content.text();
        if !cell && embed::is_single_url(text) {
            let url = text.trim().to_string();
            let Some(t) = self.texts.get(id) else {
                return true;
            };
            let sel = t.sel();
            // Pasting over a selection link-marks it directly, no menu.
            if sel.end > sel.start {
                self.link_range(id, &url, sel.start, sel.end, s);
                return true;
            }
            self.paste_menu = Some(PasteMenu {
                block: id.to_string(),
                processor: embed::processor_for_url(&url),
                url,
                at: sel.start,
            });
            return true;
        }
        false
    }

    /// Paste copied blocks after `id` - in its place when it is an empty
    /// line. One undo step; the pasted blocks come out selected.
    pub(crate) fn paste_blocks(&mut self, slice: Vec<Block>, id: &str, s: &mut State) {
        let empty = self
            .block(id)
            .is_some_and(|b| b.content.text.is_some() && b.children_ids.is_empty())
            && self.live(id).0.is_empty();
        if empty {
            self.cancel_pending(id);
        } else {
            self.flush_save(id, s);
        }
        let mut ops = doc::paste_ops(&slice, Place::new(id, pos::BOTTOM), &mut new_id);
        let selected: Vec<String> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Insert { block, at } if at.position != pos::INNER => Some(block.id.clone()),
                _ => None,
            })
            .collect();
        if empty {
            ops.push(Op::Remove { id: id.to_string() });
        }
        self.commit(ops, true, s);
        self.selected = selected;
        self.blur_texts(s);
    }

    fn link_range(&mut self, id: &str, url: &str, from: usize, to: usize, s: &mut State) {
        let (text, mut spans) = self.live(id);
        spans.push(Span {
            from,
            to,
            kind: mark::LINK,
            param: Some(url.to_string()),
        });
        let spans = marks::merge_adjacent(spans);
        self.set_live(id, text.clone(), spans.clone());
        if let Some(t) = self.texts.get_mut(id) {
            t.move_to(to);
        }
        self.cancel_pending(id);
        let task = self.w_block_update(id, self.content_for(id, &text, &spans), s);
        self.await_then_refresh(vec![task], s);
    }

    /// Insert the URL as text at the caret; optionally link-marked.
    pub(crate) fn paste_as_text(&mut self, with_mark: bool, s: &mut State) {
        let Some(menu) = self.paste_menu.take() else {
            return;
        };
        let id = menu.block;
        let (text, spans) = self.live(&id);
        let at = menu.at.min(text.len());
        let value = format!("{}{} {}", &text[..at], menu.url, &text[at..]);
        let n = menu.url.len() + 1;
        let mut shifted: Vec<Span> = spans
            .into_iter()
            .map(|m| {
                if m.from >= at {
                    Span {
                        from: m.from + n,
                        to: m.to + n,
                        ..m
                    }
                } else {
                    m
                }
            })
            .collect();
        if with_mark {
            shifted.push(Span {
                from: at,
                to: at + menu.url.len(),
                kind: mark::LINK,
                param: Some(menu.url.clone()),
            });
        }
        let shifted = marks::merge_adjacent(shifted);
        self.set_live(&id, value.clone(), shifted.clone());
        if let Some(t) = self.texts.get_mut(&id) {
            t.move_to(at + n);
        }
        self.cancel_pending(&id);
        let task = self.w_block_update(&id, self.content_for(&id, &value, &shifted), s);
        self.await_then_refresh(vec![task], s);
    }

    /// Create an embed/bookmark block: replace an empty block, else below.
    pub(crate) fn paste_as_block(&mut self, embed_kind: bool, s: &mut State) {
        let Some(menu) = self.paste_menu.take() else {
            return;
        };
        let id = menu.block;
        let text = self.live(&id).0;
        let content = match (embed_kind, menu.processor) {
            (true, Some(p)) => custom_content(
                "embed",
                &[
                    ("processor", p.key()),
                    ("url", &menu.url),
                    ("src", &embed::embed_url(p, &menu.url)),
                ],
            ),
            (true, None) => custom_content("embed", &[("url", &menu.url)]),
            (false, _) => custom_content("bookmark", &[("url", &menu.url)]),
        };
        self.cancel_pending(&id);
        let task = self.w_block_add(
            Block {
                id: new_id(),
                content,
                ..Default::default()
            },
            &id,
            if text.is_empty() {
                pos::REPLACE
            } else {
                pos::BOTTOM
            },
            s,
        );
        self.await_then_refresh(vec![task], s);
    }

    // ───────────────────────────────────────────── relation blocks

    /// Insert an inline relation block (empty text block → replace, else below)
    /// and initialize the field so it shows everywhere.
    pub(crate) fn insert_relation_block(
        &mut self,
        id: &str,
        clean: &str,
        spans: &[Span],
        key: &str,
        s: &mut State,
    ) -> Task<()> {
        let rel = Block {
            id: new_id(),
            content: custom_content("relation", &[("key", key)]),
            ..Default::default()
        };
        let mut tasks = Vec::new();
        if clean.is_empty() {
            tasks.push(self.w_block_add(rel, id, pos::REPLACE, s));
        } else {
            tasks.push(self.w_block_update(id, self.content_for(id, clean, spans), s));
            tasks.push(self.w_block_add(rel, id, pos::BOTTOM, s));
        }
        if !self.object_fields().contains_key(key) {
            let space = roostr_core::relations::object_space_id(&s.store, &self.object_fields());
            let format = roostr_core::relations::space_relations(&s.store.relations, &space)
                .into_iter()
                .find(|r| r.key == key)
                .map(|r| r.format.clone())
                .unwrap_or_else(|| "shorttext".into());
            tasks.push(self.w_set_field(key, roostr_core::relations::empty_value_for(&format), s));
        }
        s.rt.spawn(async move |_| {
            for t in tasks {
                if let Err(error) = t.await {
                    log::error!("[editor] relation block write failed: {error:#}");
                }
            }
        })
    }

    /// "New property" in the slash menu: the space's property picker (PropertySuggest).
    pub(crate) fn open_property_suggest(&mut self, id: &str, at: (f32, f32), s: &mut State) {
        let space = self.space_id(s);
        self.property_suggest = Some(PropertySuggestOpen {
            block: id.to_string(),
            at,
            state: roostr_properties::PropertySuggestState::new(space, Vec::new(), Vec::new()),
        });
    }

    /// The property picker answered: a relation block for the picked property.
    pub(crate) fn property_picked(
        &mut self,
        ev: roostr_properties::SuggestEvent<roostr_properties::PropertyPick>,
        s: &mut State,
    ) {
        let Some(open) = self.property_suggest.take() else {
            return;
        };
        if let roostr_properties::SuggestEvent::Picked(roostr_properties::PropertyPick::Relation(
            def,
        )) = ev
        {
            let block = open.block;
            let (text, spans) = self.live(&block);
            self.cancel_pending(&block);
            let task = self.insert_relation_block(&block, &text, &spans, &def.key, s);
            let oid = self.object_id.clone();
            s.rt.spawn(async move |cx| {
                task.await;
                update_editor(&cx, &oid, |e, s| e.refresh(s).detach());
            })
            .detach();
        } else {
            let len = self.texts.get(&open.block).map_or(0, |t| t.text().len());
            self.focus_block(&open.block, len, s);
        }
    }

    // ───────────────────────────────────────────── turn into object

    /// "↗ Object": pick a type for the highlighted text.
    pub(crate) fn open_turn_into(&mut self, pos: (f32, f32), s: &mut State) {
        let Some(tb) = self.toolbar.take() else {
            return;
        };
        let space = self.space_id(s);
        self.turn_into = Some(TurnIntoOpen {
            at: tb,
            pos,
            state: roostr_properties::TypeSuggestState::new(
                space,
                Vec::new(),
                "Turn into… (search types)".into(),
            ),
        });
    }

    /// The type picker answered.
    pub(crate) fn turn_into_picked(
        &mut self,
        ev: roostr_properties::SuggestEvent<String>,
        s: &mut State,
    ) {
        let Some(open) = self.turn_into.take() else {
            return;
        };
        if let roostr_properties::SuggestEvent::Picked(type_key) = ev {
            self.turn_selection_into(open.at, type_key, s);
        } else {
            self.focus_block(&open.at.block, open.at.to, s);
        }
    }

    /// Anytype's Turn into object on a text selection: a new object named
    /// after the highlighted text; that text becomes a mention of it.
    fn turn_selection_into(&mut self, at: Toolbar, type_key: String, s: &mut State) {
        let (text, _) = self.live(&at.block);
        let slice = text.get(at.from..at.to).unwrap_or("").trim();
        let name: String = slice.chars().take(120).collect();
        let name = if name.is_empty() {
            "Untitled".to_string()
        } else {
            name
        };
        let ch = self
            .object_fields()
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let fields = (!ch.is_empty()).then(|| {
            let mut f = roostr_core::types::Fields::new();
            f.insert("channel".into(), roostr_core::types::Value::string(ch));
            f
        });
        let create =
            roostr_core::create::from_type(&name, &type_key, fields.unwrap_or_default(), s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let new_id = match create.await {
                Ok(id) => id,
                Err(error) => {
                    log::error!("[editor] turn into object failed: {error:#}");
                    return;
                }
            };
            update_editor(&cx, &oid, |e, s| {
                let (text, spans) = e.live(&at.block);
                let mut next: Vec<Span> = spans
                    .into_iter()
                    .filter(|m| m.kind != mark::MENTION || m.to <= at.from || m.from >= at.to)
                    .collect();
                next.push(Span {
                    from: at.from,
                    to: at.to,
                    kind: mark::MENTION,
                    param: Some(new_id.clone()),
                });
                let next = marks::merge_adjacent(next);
                e.cancel_pending(&at.block);
                let content = e.content_for(&at.block, &text, &next);
                e.commit(
                    vec![Op::Content {
                        id: at.block.clone(),
                        content,
                    }],
                    true,
                    s,
                );
                e.set_live(&at.block, text, next);
                // The mention's title resolves through the summaries.
                s.store.refresh_all();
            });
        })
        .detach();
    }

    // ───────────────────────────────────────────── checkbox / toggles / misc

    pub(crate) fn toggle_checked(&mut self, id: &str, checked: bool, s: &mut State) {
        if self.read_only {
            return;
        }
        let Some(cur) = self.text_of(id).cloned() else {
            return;
        };
        let (text, spans) = self.live(id);
        self.cancel_pending(id);
        let content = BlockContent {
            text: Some(roostr_core::types::TextContent {
                text: text.clone(),
                marks: Some(marks::marks_from_spans(&text, &spans)),
                checked: Some(checked),
                ..cur
            }),
            ..Default::default()
        };
        let task = self.w_block_update(id, content, s);
        self.await_then_refresh(vec![task], s);
    }

    /// Empty-toggle placeholder click: create + focus the first child.
    pub(crate) fn on_empty_toggle(&mut self, id: &str, s: &mut State) {
        if self.read_only {
            return;
        }
        let inner = new_id();
        let task = self.w_block_add(
            text_block(&inner, "", style::PARAGRAPH),
            id,
            pos::INNER_FIRST,
            s,
        );
        self.focus_request = Some((inner, 0));
        self.await_then_refresh(vec![task], s);
    }

    pub(crate) fn flip_toggle(&mut self, id: &str, s: &mut State) {
        let open = !toggles::is_toggle_open(&self.object_id, id, &s.ui);
        toggles::set_toggle_open(&self.object_id, id, open, &mut s.ui);
    }

    pub(crate) fn remove_block_by_id(&mut self, id: &str, s: &mut State) -> Task<()> {
        self.cancel_pending(id);
        let task = self.w_block_remove(id, s);
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            let _ = task.await;
            let Some(refresh) = update_editor(&cx, &oid, |e, s| e.refresh(s)) else {
                return;
            };
            refresh.await;
        })
    }

    /// Copy a Code block's live text.
    pub(crate) fn copy_code(&mut self, id: &str, s: &mut State, tx: &mut Tx) {
        let text = self.live(id).0;
        tx.copy(text);
        self.copied.insert(id.to_string());
        let id = id.to_string();
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            cx.timer(Duration::from_millis(1200)).await;
            update_editor(&cx, &oid, |e, _| {
                e.copied.remove(&id);
            });
        })
        .detach();
    }

    /// Column/row id lists of a table block, from the two marker layouts.
    pub(crate) fn table_shape(&self, table: &str) -> (Vec<String>, Vec<String>) {
        let Some(t) = self.block(table) else {
            return (vec![], vec![]);
        };
        let kids: Vec<&Block> = t
            .children_ids
            .iter()
            .filter_map(|i| self.block(i))
            .collect();
        let find = |st: u32| {
            kids.iter()
                .find(|b| b.content.layout.as_ref().map(|l| l.style) == Some(st))
                .map(|b| b.children_ids.clone())
                .unwrap_or_default()
        };
        (find(layout::TABLE_COLUMNS), find(layout::TABLE_ROWS))
    }

    pub(crate) fn table_row_add(&mut self, table: &str, s: &mut State) {
        let (oid, t) = (self.object_id.clone(), table.to_string());
        let task = self
            .writes
            .run(s, move |s| roostr_core::api::table::row_add(&oid, &t, s));
        self.await_then_refresh(vec![task], s);
    }

    pub(crate) fn table_col_add(&mut self, table: &str, s: &mut State) {
        let (oid, t) = (self.object_id.clone(), table.to_string());
        let task = self
            .writes
            .run(s, move |s| roostr_core::api::table::col_add(&oid, &t, s));
        self.await_then_refresh(vec![task], s);
    }

    pub(crate) fn table_row_remove(&mut self, row: &str, s: &mut State) {
        let task = self.w_block_remove(row, s);
        self.await_then_refresh(vec![task], s);
    }

    pub(crate) fn table_col_remove(&mut self, table: &str, col: &str, s: &mut State) {
        let (oid, t, c) = (self.object_id.clone(), table.to_string(), col.to_string());
        let task = self.writes.run(s, move |s| {
            roostr_core::api::table::col_remove(&oid, &t, &c, s)
        });
        self.await_then_refresh(vec![task], s);
    }

    // ───────────────────────────────────────────── spellcheck

    pub(crate) fn schedule_spell(&mut self, s: &mut State) {
        // Roostr's bundled dictionary and the user's ignore list mark misspellings on every
        // platform, as GPUI did; the fields' own (system) spellcheck stays off. Read-only
        // bodies get no spell marks.
        if self.read_only {
            return;
        }
        let oid = self.object_id.clone();
        self.spell_task = Some(s.rt.spawn(async move |cx| {
            cx.timer(Duration::from_millis(350)).await;
            let Some(Some((dict, texts, ignored))) = update_editor(&cx, &oid, |e, s| {
                let dict = crate::spellcheck::dictionary();
                if dict.is_none() {
                    // Not loaded yet: come back once it is.
                    e.schedule_spell_later(s);
                    return None;
                }
                // Code isn't prose: Code blocks get no spell marks.
                let texts: Vec<(String, String)> = e
                    .texts
                    .iter()
                    .filter(|(id, _)| e.style_of(id) != Some(style::CODE))
                    .map(|(id, t)| (id.clone(), t.text().to_string()))
                    .collect();
                Some((dict?, texts, crate::spellcheck::ignored(&s.ui)))
            }) else {
                return;
            };
            let found = cx
                .background(async move { crate::spellcheck::check_all(&dict, &texts, &ignored) })
                .await;
            update_editor(&cx, &oid, |e, _| {
                e.misspelled = found
                    .into_iter()
                    .filter(|(_, m)| !m.is_empty())
                    .map(|(id, m)| (id, m.into_iter().map(|x| x.from..x.to).collect()))
                    .collect();
            });
        }));
    }

    fn schedule_spell_later(&mut self, s: &mut State) {
        let oid = self.object_id.clone();
        s.rt.spawn(async move |cx| {
            cx.timer(Duration::from_millis(500)).await;
            update_editor(&cx, &oid, |e, s| e.schedule_spell(s));
        })
        .detach();
    }

    /// Right-click in a block's text with the caret in a misspelled word: suggestions menu.
    /// True when one opened.
    pub(crate) fn open_spell_menu(&mut self, id: &str, s: &mut State) -> bool {
        if self.read_only || self.style_of(id) == Some(style::CODE) {
            return false;
        }
        let Some(dict) = crate::spellcheck::dictionary() else {
            return false;
        };
        let Some(index) = self.texts.get(id).map(|t| t.cursor()) else {
            return false;
        };
        let text = self.live(id).0;
        let ignored = crate::spellcheck::ignored(&s.ui);
        for miss in dict.check_text(&text, &ignored) {
            if index >= miss.from && index <= miss.to {
                let options = dict.suggestions(&miss.word, 5, &ignored);
                self.spell_menu = Some(SpellMenu {
                    block: id.to_string(),
                    word: miss.word,
                    from: miss.from,
                    to: miss.to,
                    options,
                });
                return true;
            }
        }
        false
    }

    /// Replace the misspelled word in place; marks around it stay.
    pub(crate) fn replace_misspelling(&mut self, replacement: &str, s: &mut State) {
        let Some(menu) = self.spell_menu.take() else {
            return;
        };
        if !self.texts.contains_key(&menu.block) {
            return;
        }
        if self.active.as_deref() != Some(menu.block.as_str()) {
            self.focus_block(&menu.block, menu.from, s);
        }
        self.edit(&menu.block, menu.from..menu.to, replacement, s);
        if let Some(t) = self.texts.get_mut(&menu.block) {
            t.edit.focused = true;
        }
    }

    pub(crate) fn spell_add_word(&mut self, s: &mut State) {
        let Some(menu) = self.spell_menu.take() else {
            return;
        };
        crate::spellcheck::add_to_dictionary(&menu.word, &mut s.ui);
        self.schedule_spell(s);
    }

    // ───────────────────────────────────────────── code lint

    /// A Tool's Code blocks are the code its harness runs: lint them (350ms debounce).
    pub(crate) fn schedule_lint(&mut self, s: &mut State) {
        let lints = self.object.as_ref().is_some_and(|o| o.type_key == "tool") && !self.read_only;
        if !lints {
            self.lint.clear();
            return;
        }
        self.lint_run += 1;
        let run = self.lint_run;
        let oid = self.object_id.clone();
        self.lint_task = Some(s.rt.spawn(async move |cx| {
            cx.timer(Duration::from_millis(350)).await;
            let Some(blocks) = update_editor(&cx, &oid, |e, _| {
                e.flat_ids()
                    .into_iter()
                    .filter(|id| e.style_of(id) == Some(style::CODE) && !e.is_cell(id))
                    .map(|id| {
                        let text = e.live(&id).0;
                        (id, text)
                    })
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            let texts: Vec<String> = blocks.iter().map(|(_, t)| t.clone()).collect();
            let problems = cx
                .background(async move { crate::codelint::lint_tool_code(&texts) })
                .await;
            update_editor(&cx, &oid, |e, _| {
                if run != e.lint_run {
                    return;
                }
                e.lint = blocks
                    .into_iter()
                    .zip(problems)
                    .filter(|(_, p)| !p.is_empty())
                    .map(|((id, _), p)| (id, p))
                    .collect();
            });
        }));
    }

    // ───────────────────────────────────────────── drag & drop of blocks

    /// The handle's drag started.
    pub(crate) fn begin_drag(&mut self, id: &str) {
        self.dragging = Some(id.to_string());
        self.build_drag_rects();
    }

    /// provider.tsx initData/getNodeRect: every drop target, from the rows as
    /// last laid out (window points).
    pub(crate) fn build_drag_rects(&mut self) {
        const DROP_INNER: [u32; 7] = [
            style::PARAGRAPH,
            style::BULLET,
            style::NUMBERED,
            style::CHECKBOX,
            style::TOGGLE,
            style::CALLOUT,
            style::QUOTE,
        ];
        let mut rects = Vec::new();
        // targetBot strips first: a hit there means "after this whole block".
        for (id, g) in &self.bot_strips {
            if g.bounds[3] > 0. {
                rects.push(DropRect {
                    id: id.clone(),
                    bounds: g.bounds,
                    inner: false,
                    bot: true,
                    doc_end: false,
                });
            }
        }
        for id in self.flat_ids() {
            if self.is_cell(&id) {
                continue;
            }
            let Some(g) = self.rows.get(&id).copied() else {
                continue;
            };
            let [x, y, w, h] = g.bounds;
            if h <= 0. {
                continue;
            }
            // Their DropTarget covers the content only: measure from the gutter's right edge.
            let left = if g.gutter { x + self.gutter_w() } else { x };
            let right = (x + w).max(left + 1.);
            let inner = self
                .style_of(&id)
                .is_some_and(|st| DROP_INNER.contains(&st));
            rects.push(DropRect {
                id,
                bounds: [left, y - 1. - 2., right - left, h + 1. + 1. + 2.],
                inner,
                bot: false,
                doc_end: false,
            });
        }
        let host = self.bounds;
        let bottom = rects
            .iter()
            .map(|r| r.bounds[1] + r.bounds[3])
            .fold(host[1], f32::max);
        if let Some(last) = self.root_ids().last().cloned() {
            rects.push(DropRect {
                id: last,
                bounds: [
                    host[0],
                    bottom,
                    host[2].max(1.),
                    (host[1] + host[3] - bottom).max(150.),
                ],
                inner: false,
                bot: false,
                doc_end: true,
            });
        }
        self.drag_rects = rects;
    }

    /// provider.tsx checkNodes: hit-test the targets, then the band math.
    pub(crate) fn resolve_drop(&self, p: (f32, f32)) -> Option<DropHint> {
        const OFFSET: f32 = 100.;
        const COL_BAND: f32 = 12.;
        for r in &self.drag_rects {
            let [x, y, w, h] = r.bounds;
            if p.0 < x - OFFSET || p.0 > x + w + OFFSET || p.1 < y || p.1 > y + h {
                continue;
            }
            if self.drop_forbidden(&r.id) {
                continue;
            }
            if r.doc_end {
                return Some(DropHint {
                    id: r.id.clone(),
                    position: pos::BOTTOM,
                    bot: false,
                });
            }
            if r.bot {
                return Some(DropHint {
                    id: r.id.clone(),
                    position: pos::BOTTOM,
                    bot: true,
                });
            }
            let mut position = if p.0 <= x - COL_BAND {
                pos::LEFT
            } else if p.0 > x + w {
                pos::RIGHT
            } else if p.1 <= y + h * 0.3 {
                pos::TOP
            } else if p.1 >= y + h * 0.7 {
                pos::BOTTOM
            } else {
                pos::INNER_FIRST
            };
            // A block that cannot hold children splits 50/50.
            if position == pos::INNER_FIRST && !r.inner {
                position = if p.1 <= y + h * 0.5 {
                    pos::TOP
                } else {
                    pos::BOTTOM
                };
            }
            return Some(DropHint {
                id: r.id.clone(),
                position,
                bot: false,
            });
        }
        None
    }

    /// The dragged blocks (the whole selection when dragging a selected block).
    fn drag_group(&self, dragged: &str) -> Vec<String> {
        if self.selected.len() > 1 && self.selected.iter().any(|x| x == dragged) {
            self.topmost_selected()
        } else {
            vec![dragged.to_string()]
        }
    }

    /// checkParentIds: never target the dragged blocks or their descendants.
    pub(crate) fn drop_forbidden(&self, target: &str) -> bool {
        let Some(dragging) = &self.dragging else {
            return target.is_empty();
        };
        if target.is_empty() {
            return true;
        }
        let group = self.drag_group(dragging);
        let mut walk = Some(target.to_string());
        while let Some(w) = walk {
            if group.contains(&w) {
                return true;
            }
            walk = doc::parent_of(&self.blocks, &w).map(|b| b.id.clone());
        }
        false
    }

    /// provider.tsx onDragMove: retarget for the pointer (window points).
    pub(crate) fn drag_move_to(&mut self, p: (f32, f32)) {
        self.drag_point = p;
        self.build_drag_rects();
        let hit = self.resolve_drop(p);
        if let Some(h) = &hit {
            self.last_valid_drop = Some(h.clone());
        }
        self.drop_hint = hit;
    }

    /// scrollOnMove: how far to scroll the page for a pointer at window `y`
    /// in a window `h` tall (20pt edge, 10pt max step, speed /100); 0 = none.
    pub(crate) fn edge_scroll(y: f32, h: f32) -> f32 {
        let (edge, step, speed) = (20., 10., 100.);
        if y < edge {
            -((edge - y) / speed).ceil().min(step)
        } else if y > h - edge {
            ((y - (h - edge)) / speed).ceil().min(step)
        } else {
            0.
        }
    }

    /// Shared release: clear the targeting state, then move (or cancel).
    pub(crate) fn finish_drag(&mut self, hint: Option<DropHint>, s: &mut State) {
        self.drag_rects.clear();
        self.drop_hint = None;
        self.last_valid_drop = None;
        let dragged = self.dragging.take();
        if let (Some(hint), Some(dragged)) = (hint, dragged) {
            self.dragging = Some(dragged);
            self.on_drop(&hint.id, hint.position, s);
        }
    }

    fn on_drop(&mut self, target: &str, position: u32, s: &mut State) {
        let Some(dragged) = self.dragging.take() else {
            return;
        };
        if dragged == target {
            return;
        }
        let group = self.drag_group(&dragged);
        let mut walk = Some(target.to_string());
        while let Some(w) = walk {
            if group.contains(&w) {
                return;
            }
            walk = doc::parent_of(&self.blocks, &w).map(|b| b.id.clone());
        }
        self.flush_all(s);
        // First block takes the drop position; the rest chain below it, preserving order.
        let mut tasks = Vec::new();
        let mut prev = String::new();
        let mut ops = Vec::new();
        for id in &group {
            let (t, p) = if prev.is_empty() {
                (target.to_string(), position)
            } else {
                (prev.clone(), pos::BOTTOM)
            };
            tasks.push(self.w_block_move(id, &t, p, s));
            if matches!(p, pos::TOP | pos::BOTTOM | pos::INNER | pos::INNER_FIRST) {
                ops.push(Op::Move {
                    id: id.clone(),
                    to: place(&t, p),
                });
            }
            prev = id.clone();
        }
        // Show the move now; the refresh reconciles (LEFT/RIGHT make columns server-side).
        for op in &ops {
            doc::apply_op(&mut self.blocks, op);
        }
        self.selected.clear();
        // Dropping INSIDE a closed toggle opens it.
        if position == pos::INNER_FIRST && self.style_of(target) == Some(style::TOGGLE) {
            toggles::set_toggle_open(&self.object_id, target, true, &mut s.ui);
        }
        self.await_then_refresh(tasks, s);
    }
}
