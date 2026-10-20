//! Block operations behind menus, paste and drop (`Editor.svelte`): slash
//! picks, files, URLs, relation/link blocks, the block action menu,
//! duplicate, turn into object, copy/cut/paste of a block selection,
//! spellcheck and Tool code lint.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::*;
use roostr_core::types::{Block, BlockContent, CustomContent, layout, mark, pos, style};
use roostr_core::{Store, api};

use crate::block_menu::{BlockMenu, MenuAction, MenuEvent};
use crate::doc::{self, Op, Place};
use crate::editor::{
    Editor, OpenBlockMenu, PasteMenu, SpellMenu, custom_content, new_id, place, text_block,
    text_content,
};
use crate::embed;
use crate::files::{self, PickedFile};
use crate::link_picker::{LinkPicker, LinkPickerEvent, LinkPrompt, LinkPromptEvent};
use crate::marks::{self, Span};
use crate::slash_menu::SlashPick;
use crate::toggles;
use crate::{Copy, Cut, Paste};

/// The clipboard metadata carrying copied blocks between Roostr pages (`BLOCKS_MIME`).
#[derive(serde::Serialize, serde::Deserialize)]
struct ClipboardBlocks {
    roostr_blocks: Vec<Block>,
}

impl Editor {
    // ───────────────────────────────────────────── slash

    /// Apply a slash pick: strip "/filter" from the block, then act.
    pub(crate) fn apply_slash(
        &mut self,
        pick: SlashPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slash) = self.slash.take() else {
            return;
        };
        let id = slash.block.clone();
        let anchor = slash.pos.unwrap_or_default();
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
                    tasks.push(self.w_table_create(&id, pos::REPLACE, 3, 3, cx));
                } else {
                    tasks.push(self.w_block_update(
                        &id,
                        self.content_for(&id, &clean, &clean_spans),
                        cx,
                    ));
                    tasks.push(self.w_table_create(&id, pos::BOTTOM, 3, 3, cx));
                }
                self.await_then_refresh(tasks, cx);
            }
            SlashPick::Divider(kind) => {
                let divider = custom_content("divider", &[("style", kind)]);
                let mut tasks = Vec::new();
                if clean.is_empty() {
                    let para = new_id();
                    tasks.push(self.w_block_update(&id, divider, cx));
                    tasks.push(self.w_block_add(
                        text_block(&para, "", style::PARAGRAPH),
                        &id,
                        pos::BOTTOM,
                        cx,
                    ));
                    self.focus_request = Some((para, 0));
                } else {
                    tasks.push(self.w_block_update(
                        &id,
                        self.content_for(&id, &clean, &clean_spans),
                        cx,
                    ));
                    let block = Block {
                        id: new_id(),
                        content: divider,
                        ..Default::default()
                    };
                    tasks.push(self.w_block_add(block, &id, pos::BOTTOM, cx));
                    self.focus_request = Some((id.clone(), slash.start));
                }
                self.await_then_refresh(tasks, cx);
            }
            SlashPick::File => {
                let pick = files::pick_file(cx);
                cx.spawn(async move |this, cx| {
                    if let Some(file) = pick.await {
                        let Ok(task) = this.update(cx, |this, cx| {
                            this.insert_file(&id, file, clean, clean_spans, cx)
                        }) else {
                            return;
                        };
                        task.await;
                    }
                    let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
                })
                .detach();
            }
            SlashPick::Relation(key) => {
                let task = self.insert_relation_block(&id, &clean, &clean_spans, &key, cx);
                cx.spawn(async move |this, cx| {
                    task.await;
                    let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
                })
                .detach();
            }
            SlashPick::LinkObject => {
                // Anytype: "Link to existing page" opens the searchObject menu.
                self.open_link_picker(&id, anchor, window, cx);
            }
            SlashPick::PropertyAdd => {
                // Anytype: "New relation" opens the relationSuggest menu.
                self.open_property_suggest(&id, anchor, window, cx);
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
                let task = self.w_block_update(&id, content, cx);
                self.focus_request = Some((id.clone(), slash.start));
                self.await_then_refresh(vec![task], cx);
            }
        }
        cx.notify();
    }

    /// Await `tasks` in order, then `refresh`.
    pub(crate) fn await_then_refresh(
        &mut self,
        tasks: Vec<Task<anyhow::Result<roostr_core::JsonMap>>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            for t in tasks {
                if let Err(error) = t.await {
                    log::error!("[editor] write failed: {error:#}");
                }
            }
            let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── files

    /// Upload `file` and build the block content that points at it; `None`
    /// (after saying why) when the upload fails.
    pub(crate) fn file_content(
        &mut self,
        file: PickedFile,
        cx: &mut Context<Self>,
    ) -> Task<Option<BlockContent>> {
        let name = file.name.clone();
        let mime = file.mime.clone();
        let upload = files::upload_file(file, self.space_id(cx), cx);
        cx.spawn(async move |this, cx| {
            let result = upload.await;
            this.update(cx, |this, cx| match result {
                Ok(up) => {
                    this.file_notice.clear();
                    cx.notify();
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
                    this.file_notice = files::add_failure_text(&err, &name);
                    cx.notify();
                    None
                }
            })
            .ok()
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
        cx: &mut Context<Self>,
    ) -> Task<String> {
        let name = file.name.clone();
        let content = self.file_content(file, cx);
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let Some(block) = content.await else { return String::new() };
            let Ok((tasks, file_id)) = this.update(cx, |this, cx| {
                if clean.is_empty() {
                    let para = new_id();
                    let tasks = vec![
                        this.w_block_update(&id, block, cx),
                        this.w_block_add(text_block(&para, "", style::PARAGRAPH), &id, pos::BOTTOM, cx),
                    ];
                    this.focus_request = Some((para, 0));
                    (tasks, id.clone())
                } else {
                    let file_id = new_id();
                    let tasks = vec![
                        this.w_block_update(&id, this.content_for(&id, &clean, &spans), cx),
                        this.w_block_add(Block { id: file_id.clone(), content: block, ..Default::default() }, &id, pos::BOTTOM, cx),
                    ];
                    (tasks, file_id)
                }
            }) else {
                return String::new();
            };
            for t in tasks {
                if let Err(err) = t.await {
                    // The bytes are a File object by now; placing it on the page failed: say so.
                    let _ = this.update(cx, |this, cx| {
                        this.file_notice = format!(
                            "{name} was uploaded (it's a File in this space), but it couldn't be placed on this page: {err:#}. Reload and try again."
                        );
                        cx.notify();
                    });
                    return String::new();
                }
            }
            file_id
        })
    }

    /// Pasted image files become File blocks (the first in place of an empty line).
    pub(crate) fn paste_files(&mut self, id: &str, files: Vec<PickedFile>, cx: &mut Context<Self>) {
        let (text, spans) = self.live(id);
        self.cancel_pending(id);
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let mut files = files.into_iter();
            let Some(first) = files.next() else { return };
            let Ok(task) =
                this.update(cx, |this, cx| this.insert_file(&id, first, text, spans, cx))
            else {
                return;
            };
            let mut last = task.await;
            for file in files {
                if last.is_empty() {
                    break;
                }
                let Ok(content) = this.update(cx, |this, cx| this.file_content(file, cx)) else {
                    return;
                };
                let Some(block) = content.await else { break };
                let next = new_id();
                let Ok(add) = this.update(cx, |this, cx| {
                    this.w_block_add(
                        Block {
                            id: next.clone(),
                            content: block,
                            ..Default::default()
                        },
                        &last,
                        pos::BOTTOM,
                        cx,
                    )
                }) else {
                    return;
                };
                let _ = add.await;
                last = next;
            }
            let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
        })
        .detach();
    }

    /// The OS file drag over the editor was released: read the files and
    /// drop them at the hinted line (else after the last root block).
    pub(crate) fn drop_external(&mut self, cx: &mut Context<Self>) {
        let hint = self.drop_hint.clone().or(self.last_valid_drop.clone());
        let paths = self.file_drag.take().unwrap_or_default();
        self.drag_rects.clear();
        self.drop_hint = None;
        self.last_valid_drop = None;
        cx.notify();
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
        // A browser drop names no readable paths.
        #[cfg(target_family = "wasm")]
        let read = {
            let _ = paths;
            gpui_kit::Task::ready(Vec::new())
        };
        #[cfg(not(target_family = "wasm"))]
        let read = cx.background_spawn(async move {
            paths
                .iter()
                .filter_map(|p| crate::files::read_path(p).ok())
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let files = read.await;
            let _ = this.update(cx, |this, cx| this.drop_files(files, target, position, cx));
        })
        .detach();
    }

    /// Dropped files become File blocks at the drop line, in order; a file never nests inside a text block.
    pub(crate) fn drop_files(
        &mut self,
        files: Vec<PickedFile>,
        target: String,
        position: u32,
        cx: &mut Context<Self>,
    ) {
        let at = if position == pos::INNER_FIRST || position == pos::INNER {
            pos::BOTTOM
        } else {
            position
        };
        cx.spawn(async move |this, cx| {
            let mut prev = String::new();
            for file in files {
                let Ok(content) = this.update(cx, |this, cx| this.file_content(file, cx)) else {
                    return;
                };
                let Some(block) = content.await else { break };
                let id = new_id();
                let (t, p) = if prev.is_empty() {
                    (target.clone(), at)
                } else {
                    (prev.clone(), pos::BOTTOM)
                };
                let Ok(add) = this.update(cx, |this, cx| {
                    this.w_block_add(
                        Block {
                            id: id.clone(),
                            content: block,
                            ..Default::default()
                        },
                        &t,
                        p,
                        cx,
                    )
                }) else {
                    return;
                };
                let _ = add.await;
                prev = id;
            }
            let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
        })
        .detach();
    }

    // ───────────────────────────────────────────── copy / cut / paste

    pub(crate) fn on_copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_or_cut(false, window, cx);
    }

    pub(crate) fn on_cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy_or_cut(true, window, cx);
    }

    fn copy_or_cut(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            cx.propagate();
            return;
        }
        if let Some(id) = self.active.clone() {
            let Some(t) = self.texts.get(&id) else { return };
            if t.sel.is_empty() {
                return;
            }
            cx.write_to_clipboard(ClipboardItem::new_string(t.text[t.sel.clone()].to_string()));
            if cut && !self.read_only {
                let range = t.sel.clone();
                self.edit(&id, range, "", cx);
            }
            return;
        }
        // A block selection writes itself: plain text for other apps, the exact blocks for Roostr.
        if self.selected.is_empty() {
            return;
        }
        let tops = self.topmost_selected();
        let slice = doc::copy_slice(&self.blocks, &tops);
        if slice.is_empty() {
            return;
        }
        let text = doc::slice_text(&slice);
        cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
            text,
            ClipboardBlocks {
                roostr_blocks: slice,
            },
        ));
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
            cx,
        );
    }

    pub(crate) fn on_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) || self.read_only {
            cx.propagate();
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        self.paste_item(item, window, cx);
    }

    /// A paste of `item` (`onPasteText`, Editor.svelte's window paste): into
    /// the block being edited, else - copied blocks, with blocks selected -
    /// after the selection.
    pub(crate) fn paste_item(
        &mut self,
        item: ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slice = clipboard_blocks(&item);
        if let Some(id) = self.active.clone() {
            self.paste_into(&id, item, slice, window, cx);
            return;
        }
        // Paste with blocks selected and no caret: the copy lands after the selection.
        if let Some(slice) = slice
            && let Some(last) = self.topmost_selected().last().cloned()
        {
            self.paste_blocks(slice, &last, cx);
        }
    }

    /// `onPasteText`: copied blocks paste as blocks, images as File blocks,
    /// one URL opens "Paste as" (or links the selection); anything else is text.
    fn paste_into(
        &mut self,
        id: &str,
        item: ClipboardItem,
        slice: Option<Vec<Block>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cell = self.is_cell(id);
        if !cell {
            if let Some(slice) = slice {
                self.paste_blocks(slice, id, cx);
                return;
            }
            let images: Vec<PickedFile> = item
                .entries()
                .iter()
                .filter_map(|e| match e {
                    ClipboardEntry::Image(image) => Some(files::pasted_image(image)),
                    _ => None,
                })
                .collect();
            if !images.is_empty() {
                self.paste_files(id, images, cx);
                return;
            }
        }
        let text = item.text().unwrap_or_default();
        if !cell && embed::is_single_url(&text) {
            let url = text.trim().to_string();
            let Some(t) = self.texts.get(id) else { return };
            let (from, to) = (t.sel.start, t.sel.end);
            // Pasting over a selection link-marks it directly, no menu.
            if to > from {
                self.link_range(id, &url, from, to, cx);
                return;
            }
            let anchor = {
                let layouts = self.layouts.borrow();
                layouts.get(id).map(|l| {
                    let p = l.position_for_index(from);
                    point(p.x, p.y + l.line_height + px(6.))
                })
            }
            .unwrap_or(point(px(120.), px(126.)));
            self.paste_menu = Some(PasteMenu {
                block: id.to_string(),
                processor: embed::processor_for_url(&url),
                url,
                pos: anchor,
                at: from,
            });
            cx.notify();
            return;
        }
        if text.is_empty() {
            return;
        }
        let _ = window;
        self.insert_text(None, &text, cx);
    }

    /// Paste copied blocks after `id` - in its place when it is an empty
    /// line. One undo step; the pasted blocks come out selected.
    pub(crate) fn paste_blocks(&mut self, slice: Vec<Block>, id: &str, cx: &mut Context<Self>) {
        let empty = self
            .block(id)
            .is_some_and(|b| b.content.text.is_some() && b.children_ids.is_empty())
            && self.live(id).0.is_empty();
        if empty {
            self.cancel_pending(id);
        } else {
            self.flush_save(id, cx);
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
        self.commit(ops, true, cx);
        if let Some(a) = self.active.take() {
            self.flush_save(&a, cx);
        }
        self.selected = selected;
        cx.notify();
    }

    fn link_range(&mut self, id: &str, url: &str, from: usize, to: usize, cx: &mut Context<Self>) {
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
        let task = self.w_block_update(id, self.content_for(id, &text, &spans), cx);
        self.await_then_refresh(vec![task], cx);
    }

    /// Insert the URL as text at the caret; optionally link-marked.
    pub(crate) fn paste_as_text(&mut self, with_mark: bool, cx: &mut Context<Self>) {
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
        let task = self.w_block_update(&id, self.content_for(&id, &value, &shifted), cx);
        self.await_then_refresh(vec![task], cx);
    }

    /// Create an embed/bookmark block: replace an empty block, else below.
    pub(crate) fn paste_as_block(&mut self, embed_kind: bool, cx: &mut Context<Self>) {
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
            cx,
        );
        self.await_then_refresh(vec![task], cx);
    }

    // ───────────────────────────────────────────── relation & link blocks

    /// Insert an inline relation block (empty text block → replace, else below)
    /// and initialize the field so it shows everywhere.
    pub(crate) fn insert_relation_block(
        &mut self,
        id: &str,
        clean: &str,
        spans: &[Span],
        key: &str,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let rel = Block {
            id: new_id(),
            content: custom_content("relation", &[("key", key)]),
            ..Default::default()
        };
        let mut tasks = Vec::new();
        if clean.is_empty() {
            tasks.push(self.w_block_add(rel, id, pos::REPLACE, cx));
        } else {
            tasks.push(self.w_block_update(id, self.content_for(id, clean, spans), cx));
            tasks.push(self.w_block_add(rel, id, pos::BOTTOM, cx));
        }
        if !self.object_fields().contains_key(key) {
            let store = Store::global(cx).read(cx);
            let space = roostr_core::relations::object_space_id(store, &self.object_fields());
            let format = roostr_core::relations::space_relations(&store.relations, &space)
                .into_iter()
                .find(|r| r.key == key)
                .map(|r| r.format.clone())
                .unwrap_or_else(|| "shorttext".into());
            tasks.push(self.w_set_field(key, roostr_core::relations::empty_value_for(&format), cx));
        }
        cx.background_spawn(async move {
            for t in tasks {
                if let Err(error) = t.await {
                    log::error!("[editor] relation block write failed: {error:#}");
                }
            }
        })
    }

    pub(crate) fn open_link_picker(
        &mut self,
        id: &str,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|cx| LinkPicker::new(self.object_id.clone(), window, cx));
        let block = id.to_string();
        let sub = cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &LinkPickerEvent, window, cx| {
                match event {
                    LinkPickerEvent::Picked(target) => this.insert_link_block(&block, target, cx),
                    LinkPickerEvent::Close => {}
                }
                this.link_picker = None;
                window.focus(&this.focus_handle, cx);
                cx.notify();
            },
        );
        self.link_picker = Some((id.to_string(), picker, at, sub));
    }

    /// Create the link block: replace the empty slash block, else below.
    fn insert_link_block(&mut self, id: &str, target: &str, cx: &mut Context<Self>) {
        let empty = self.live(id).0.is_empty();
        let block = Block {
            id: new_id(),
            content: custom_content("link", &[("target", target)]),
            ..Default::default()
        };
        let task = self.w_block_add(
            block,
            id,
            if empty { pos::REPLACE } else { pos::BOTTOM },
            cx,
        );
        self.await_then_refresh(vec![task], cx);
    }

    pub(crate) fn open_property_suggest(
        &mut self,
        id: &str,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self.space_id(cx);
        let suggest = cx.new(|cx| {
            roostr_properties::PropertySuggest::new(space, Vec::new(), Vec::new(), window, cx)
        });
        let block = id.to_string();
        let sub = cx.subscribe_in(
            &suggest,
            window,
            move |this,
                  _,
                  event: &roostr_properties::SuggestEvent<roostr_properties::PropertyPick>,
                  window,
                  cx| {
                if let roostr_properties::SuggestEvent::Picked(
                    roostr_properties::PropertyPick::Relation(def),
                ) = event
                {
                    let (text, spans) = this.live(&block);
                    this.cancel_pending(&block);
                    let task = this.insert_relation_block(&block, &text, &spans, &def.key, cx);
                    cx.spawn(async move |this, cx| {
                        task.await;
                        let _ = this.update(cx, |this, cx| this.refresh(cx).detach());
                    })
                    .detach();
                }
                this.property_suggest = None;
                window.focus(&this.focus_handle, cx);
                cx.notify();
            },
        );
        window.focus(&suggest.focus_handle(cx), cx);
        self.property_suggest = Some((id.to_string(), suggest.into(), at, sub));
    }

    // ───────────────────────────────────────────── marks toolbar actions

    /// "🔗" in the toolbar: ask for the URL (the web's `prompt("Link URL:")`).
    pub(crate) fn add_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tb) = self.toolbar.take() else {
            return;
        };
        let prompt = cx.new(|cx| LinkPrompt::new(window, cx));
        let sub = cx.subscribe_in(
            &prompt,
            window,
            |this, _, event: &LinkPromptEvent, window, cx| {
                let Some((tb, _, _)) = this.link_prompt.take() else {
                    return;
                };
                window.focus(&this.focus_handle, cx);
                if let LinkPromptEvent::Submit(url) = event
                    && !url.is_empty()
                {
                    this.toolbar = Some(tb.clone());
                    this.apply_mark(&tb.block, mark::LINK, Some(url.clone()), cx);
                }
                cx.notify();
            },
        );
        self.link_prompt = Some((tb, prompt, sub));
        cx.notify();
    }

    /// "↗ Object": pick a type for the highlighted text.
    pub(crate) fn open_turn_into(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tb) = self.toolbar.take() else {
            return;
        };
        let space = self.space_id(cx);
        let suggest = cx.new(|cx| {
            roostr_properties::TypeSuggest::new(
                space,
                Vec::new(),
                "Turn into… (search types)".into(),
                window,
                cx,
            )
        });
        let sub = cx.subscribe_in(
            &suggest,
            window,
            |this, _, event: &roostr_properties::SuggestEvent<String>, window, cx| {
                let Some((at, _, _)) = this.turn_into.take() else {
                    return;
                };
                window.focus(&this.focus_handle, cx);
                if let roostr_properties::SuggestEvent::Picked(type_key) = event {
                    this.turn_selection_into(at, type_key.clone(), cx);
                }
                cx.notify();
            },
        );
        window.focus(&suggest.focus_handle(cx), cx);
        self.turn_into = Some((tb, suggest.into(), sub));
        cx.notify();
    }

    /// Anytype's Turn into object on a text selection: a new object named
    /// after the highlighted text; that text becomes a mention of it.
    fn turn_selection_into(
        &mut self,
        at: crate::editor::Toolbar,
        type_key: String,
        cx: &mut Context<Self>,
    ) {
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
        let create = api::note::create(&name, Some(&type_key), fields, cx);
        cx.spawn(async move |this, cx| {
            let new_id = match create.await {
                Ok(id) => id,
                Err(error) => {
                    log::error!("[editor] turn into object failed: {error:#}");
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                let (text, spans) = this.live(&at.block);
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
                this.cancel_pending(&at.block);
                let content = this.content_for(&at.block, &text, &next);
                this.commit(
                    vec![Op::Content {
                        id: at.block.clone(),
                        content,
                    }],
                    true,
                    cx,
                );
                this.set_live(&at.block, text, next);
                // The mention's title resolves through the summaries.
                Store::global(cx).update(cx, |s, cx| s.refresh_all(cx));
            });
        })
        .detach();
    }

    // ───────────────────────────────────────────── checkbox / toggles / misc

    pub(crate) fn toggle_checked(&mut self, id: &str, checked: bool, cx: &mut Context<Self>) {
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
        let task = self.w_block_update(id, content, cx);
        self.await_then_refresh(vec![task], cx);
    }

    /// Empty-toggle placeholder click: create + focus the first child.
    pub(crate) fn on_empty_toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let inner = new_id();
        let task = self.w_block_add(
            text_block(&inner, "", style::PARAGRAPH),
            id,
            pos::INNER_FIRST,
            cx,
        );
        self.focus_request = Some((inner, 0));
        self.await_then_refresh(vec![task], cx);
    }

    pub(crate) fn flip_toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let open = !toggles::is_toggle_open(&self.object_id, id, cx);
        toggles::set_toggle_open(&self.object_id, id, open, cx);
        cx.notify();
    }

    pub(crate) fn remove_block_by_id(&mut self, id: &str, cx: &mut Context<Self>) -> Task<()> {
        self.cancel_pending(id);
        let task = self.w_block_remove(id, cx);
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let Ok(refresh) = this.update(cx, |this, cx| this.refresh(cx)) else {
                return;
            };
            refresh.await;
        })
    }

    /// Copy a Code block's live text.
    pub(crate) fn copy_code(&mut self, id: &str, cx: &mut Context<Self>) {
        let text = self.live(id).0;
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied.insert(id.to_string());
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied.remove(&id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ───────────────────────────────────────────── block menu

    pub(crate) fn open_block_menu(
        &mut self,
        id: &str,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.toolbar = None;
        self.slash = None;
        // Opened on a block inside a multi-selection: styling applies to the whole selection.
        let group = (self.selected.len() > 1 && self.selected.iter().any(|s| s == id))
            .then(|| self.selected.clone());
        // Opened outside the selection: retarget it to the clicked block.
        if group.is_none() && !self.selected.is_empty() && !self.selected.iter().any(|s| s == id) {
            self.selected = vec![id.to_string()];
        }
        let Some(block) = self.block(id).cloned() else {
            return;
        };
        if let Some(a) = self.active.clone() {
            self.flush_save(&a, cx);
        }
        let count = group.as_ref().map_or(1, Vec::len);
        let menu = cx.new(|cx| BlockMenu::new(block, at, count, window, cx));
        let sub = cx.subscribe_in(&menu, window, |this, _, event: &MenuEvent, window, cx| {
            match event {
                MenuEvent::Action(action) => this.on_menu_action(action.clone(), window, cx),
                MenuEvent::Close => {}
            }
            this.block_menu = None;
            cx.notify();
        });
        self.block_menu = Some(OpenBlockMenu {
            block: id.to_string(),
            group,
            menu,
            _sub: sub,
        });
        cx.notify();
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
        let find = |s: u32| {
            kids.iter()
                .find(|b| b.content.layout.as_ref().map(|l| l.style) == Some(s))
                .map(|b| b.children_ids.clone())
                .unwrap_or_default()
        };
        (find(layout::TABLE_COLUMNS), find(layout::TABLE_ROWS))
    }

    /// Duplicate a block's subtree below it (fresh ids, preserved order).
    fn duplicate_block(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(src) = self.block(id).cloned() else {
            return;
        };
        self.cancel_pending(id);
        if src.content.table.is_some() {
            // Cells are addressed "<rowId>-<colId>": create a same-size table, then copy cells.
            let (cols, rows) = self.table_shape(id);
            let create =
                self.w_table_create(id, pos::BOTTOM, rows.len() as u32, cols.len() as u32, cx);
            let cells: Vec<Vec<Option<roostr_core::types::TextContent>>> = rows
                .iter()
                .map(|r| {
                    cols.iter()
                        .map(|c| self.text_of(&format!("{r}-{c}")).cloned())
                        .collect()
                })
                .collect();
            let oid = self.object_id.clone();
            cx.spawn(async move |this, cx| {
                let Ok(result) = create.await else { return };
                let new_table = result
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let Ok(fetch) = this.update(cx, |_, cx| {
                    Store::global(cx).read(cx).fetch_object(&oid, cx)
                }) else {
                    return;
                };
                let Ok(fresh) = fetch.await else { return };
                let Ok(tasks) = this.update(cx, |this, cx| {
                    let kids: Vec<&Block> = fresh
                        .block(&new_table)
                        .map(|t| {
                            t.children_ids
                                .iter()
                                .filter_map(|i| fresh.block(i))
                                .collect()
                        })
                        .unwrap_or_default();
                    let find = |s: u32| {
                        kids.iter()
                            .find(|b| b.content.layout.as_ref().map(|l| l.style) == Some(s))
                            .map(|b| b.children_ids.clone())
                            .unwrap_or_default()
                    };
                    let (ncols, nrows) = (find(layout::TABLE_COLUMNS), find(layout::TABLE_ROWS));
                    let mut tasks = Vec::new();
                    for (r, row) in cells.iter().enumerate() {
                        for (c, cell) in row.iter().enumerate() {
                            let Some(cell) = cell else { continue };
                            if cell.text.is_empty() && cell.marks.as_ref().is_none_or(Vec::is_empty)
                            {
                                continue;
                            }
                            let (Some(nr), Some(nc)) = (nrows.get(r), ncols.get(c)) else {
                                continue;
                            };
                            tasks.push(this.w_block_update(
                                &format!("{nr}-{nc}"),
                                BlockContent {
                                    text: Some(cell.clone()),
                                    ..Default::default()
                                },
                                cx,
                            ));
                        }
                    }
                    tasks
                }) else {
                    return;
                };
                let _ = this.update(cx, |this, cx| this.await_then_refresh(tasks, cx));
            })
            .detach();
            return;
        }
        let mut tasks = Vec::new();
        fn clone_into(
            this: &Editor,
            src: &str,
            target: &str,
            position: u32,
            tasks: &mut Vec<Task<anyhow::Result<roostr_core::JsonMap>>>,
            cx: &mut Context<Editor>,
        ) {
            let Some(s) = this.block(src).cloned() else {
                return;
            };
            let nid = new_id();
            tasks.push(this.w_block_add(
                Block {
                    id: nid.clone(),
                    children_ids: vec![],
                    ..s.clone()
                },
                target,
                position,
                cx,
            ));
            for c in &s.children_ids {
                clone_into(this, c, &nid, pos::INNER, tasks, cx);
            }
        }
        clone_into(self, id, id, pos::BOTTOM, &mut tasks, cx);
        self.await_then_refresh(tasks, cx);
    }

    fn on_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.block_menu.as_ref() else {
            return;
        };
        let id = open.block.clone();
        // Styling from a multi-selection hits every selected block.
        let ids = open.group.clone().unwrap_or_else(|| vec![id.clone()]);
        let text_ids: Vec<String> = ids
            .iter()
            .filter(|b| self.text_of(b).is_some())
            .cloned()
            .collect();
        // Group styling snapshots reads before any write and refreshes once.
        let group_text = |this: &mut Self,
                          make: &dyn Fn(
            &roostr_core::types::TextContent,
            &str,
            &[Span],
        ) -> roostr_core::types::TextContent,
                          cx: &mut Context<Self>| {
            let payloads: Vec<(String, BlockContent)> = text_ids
                .iter()
                .filter_map(|b| {
                    let cur = this.text_of(b)?.clone();
                    let (text, spans) = this.live(b);
                    Some((
                        b.clone(),
                        BlockContent {
                            text: Some(make(&cur, &text, &spans)),
                            ..Default::default()
                        },
                    ))
                })
                .collect();
            let mut tasks = Vec::new();
            for (b, content) in payloads {
                this.cancel_pending(&b);
                tasks.push(this.w_block_update(&b, content, cx));
            }
            tasks
        };
        match action {
            MenuAction::Style(value) => {
                if ids.len() == 1 {
                    self.apply_style(&id, value, cx);
                    return;
                }
                let tasks = group_text(
                    self,
                    &|cur, text, spans| roostr_core::types::TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        style: value,
                        checked: Some(cur.checked.unwrap_or(false)),
                        color: Some(cur.color.clone().unwrap_or_default()),
                    },
                    cx,
                );
                self.await_then_refresh(tasks, cx);
            }
            MenuAction::Align(value) => {
                let tasks = ids
                    .iter()
                    .map(|b| self.w_set_attrs(b, Some(value), None, cx))
                    .collect();
                self.await_then_refresh(tasks, cx);
            }
            MenuAction::Color(value) => {
                let tasks = group_text(
                    self,
                    &|cur, text, spans| roostr_core::types::TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        color: Some(value.clone()),
                        ..cur.clone()
                    },
                    cx,
                );
                self.await_then_refresh(tasks, cx);
            }
            MenuAction::Background(value) => {
                let tasks = ids
                    .iter()
                    .map(|b| self.w_set_attrs(b, None, Some(value.clone()), cx))
                    .collect();
                self.await_then_refresh(tasks, cx);
            }
            MenuAction::Duplicate => self.duplicate_block(&id, cx),
            MenuAction::LinkStyle(value) => {
                if let Some(custom) = self.block(&id).and_then(|b| b.content.custom.clone())
                    && custom.content_type == "link"
                {
                    let mut meta = custom.meta.clone().unwrap_or_default();
                    meta.insert("style".into(), value);
                    let content = BlockContent {
                        custom: Some(CustomContent {
                            content_type: "link".into(),
                            meta: Some(meta),
                            data: None,
                        }),
                        ..Default::default()
                    };
                    let task = self.w_block_update(&id, content, cx);
                    self.await_then_refresh(vec![task], cx);
                }
            }
            MenuAction::TurnIntoObject(type_key) => self.turn_into_object(&id, type_key, cx),
            MenuAction::ClearStyle => {
                let mut tasks = group_text(
                    self,
                    &|cur, text, spans| roostr_core::types::TextContent {
                        text: text.to_string(),
                        marks: Some(marks::marks_from_spans(text, spans)),
                        color: Some(String::new()),
                        ..cur.clone()
                    },
                    cx,
                );
                for b in &ids {
                    tasks.push(self.w_set_attrs(b, Some(0), Some(String::new()), cx));
                }
                self.await_then_refresh(tasks, cx);
            }
            MenuAction::Delete => {
                if ids.len() > 1 {
                    let set: HashSet<String> = ids.iter().cloned().collect();
                    let tops = self.topmost_of(&set);
                    self.selected.clear();
                    let mut tasks = Vec::new();
                    for b in tops {
                        self.cancel_pending(&b);
                        tasks.push(self.w_block_remove(&b, cx));
                    }
                    self.await_then_refresh(tasks, cx);
                } else {
                    self.remove_block_by_id(&id, cx).detach();
                }
            }
        }
        let _ = window;
    }

    /// Turn-into from the block action menu.
    fn apply_style(&mut self, id: &str, value: u32, cx: &mut Context<Self>) {
        let cur = self.text_of(id).cloned();
        let (text, spans) = self.live(id);
        self.cancel_pending(id);
        let content = text_content(
            &text,
            &spans,
            value,
            cur.as_ref().and_then(|t| t.checked).unwrap_or(false),
            cur.as_ref()
                .and_then(|t| t.color.clone())
                .unwrap_or_default(),
        );
        let task = self.w_block_update(id, content, cx);
        self.focus_request = Some((id.to_string(), text.len()));
        self.await_then_refresh(vec![task], cx);
    }

    /// Anytype's Turn into object: the block's text becomes a new object's
    /// name, its children that object's content, and a link block takes its place.
    fn turn_into_object(&mut self, id: &str, type_key: String, cx: &mut Context<Self>) {
        self.flush_all(cx);
        let typed = self.live(id).0;
        let id = id.to_string();
        let refresh = self.refresh(cx);
        cx.spawn(async move |this, cx| {
            refresh.await;
            let Ok(Some((src, ch))) = this.update(cx, |this, _| {
                let src = this.block(&id).cloned()?;
                let ch = this
                    .object_fields()
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some((src, ch))
            }) else {
                return;
            };
            let stored = src
                .content
                .text
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let base = if stored.is_empty() { typed } else { stored };
            let name: String = base.trim().chars().take(120).collect();
            let name = if name.is_empty() {
                "Untitled".to_string()
            } else {
                name
            };
            let fields = (!ch.is_empty()).then(|| {
                let mut f = roostr_core::types::Fields::new();
                f.insert("channel".into(), roostr_core::types::Value::string(ch));
                f
            });
            let Ok(create) = this.update(cx, |_, cx| {
                api::note::create(&name, Some(&type_key), fields, cx)
            }) else {
                return;
            };
            let new_obj = match create.await {
                Ok(id) => id,
                Err(error) => {
                    log::error!("[editor] turn into object failed: {error:#}");
                    return;
                }
            };
            let Ok(tasks) = this.update(cx, |this, cx| {
                let mut tasks = Vec::new();
                fn move_into(
                    this: &Editor,
                    src: &str,
                    new_obj: &str,
                    target: &str,
                    position: u32,
                    tasks: &mut Vec<Task<anyhow::Result<roostr_core::JsonMap>>>,
                    cx: &mut Context<Editor>,
                ) {
                    let Some(s) = this.block(src).cloned() else {
                        return;
                    };
                    let nid = new_id();
                    let block = Block {
                        id: nid.clone(),
                        children_ids: vec![],
                        ..s.clone()
                    };
                    let (obj, target) = (new_obj.to_string(), target.to_string());
                    tasks.push(this.writes.run(cx, move |cx| {
                        api::note::block_add(&obj, &block, &target, position, cx)
                    }));
                    for c in &s.children_ids {
                        move_into(this, c, new_obj, &nid, pos::INNER, tasks, cx);
                    }
                }
                for c in &src.children_ids {
                    move_into(this, c, &new_obj, "", pos::NONE, &mut tasks, cx);
                }
                let link = Block {
                    id: new_id(),
                    content: custom_content("link", &[("target", &new_obj)]),
                    ..Default::default()
                };
                tasks.push(this.w_block_add(link, &id, pos::BOTTOM, cx));
                tasks
            }) else {
                return;
            };
            for t in tasks {
                let _ = t.await;
            }
            let Ok(remove) = this.update(cx, |this, cx| this.remove_block_by_id(&id, cx)) else {
                return;
            };
            remove.await;
            let _ = this.update(cx, |_, cx| {
                Store::global(cx).update(cx, |s, cx| s.refresh_all(cx))
            });
        })
        .detach();
    }

    // ───────────────────────────────────────────── spell & lint

    pub(crate) fn schedule_spell(&mut self, cx: &mut Context<Self>) {
        // The web only checks editable text: a read-only body gets no spell marks.
        if self.read_only {
            return;
        }
        self.spell_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            let Ok(Some((dict, texts, ignored))) = this.update(cx, |this, cx| {
                let dict = crate::spellcheck::dictionary();
                if dict.is_none() {
                    // Not loaded yet: come back once it is.
                    this.schedule_spell_later(cx);
                    return None;
                }
                // Code isn't prose: Code blocks get no spell marks.
                let texts: Vec<(String, String)> = this
                    .texts
                    .iter()
                    .filter(|(id, _)| this.style_of(id) != Some(style::CODE))
                    .map(|(id, t)| (id.clone(), t.text.clone()))
                    .collect();
                Some((dict?, texts, crate::spellcheck::ignored(cx)))
            }) else {
                return;
            };
            let found = cx
                .background_spawn(
                    async move { crate::spellcheck::check_all(&dict, &texts, &ignored) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.misspelled = found
                    .into_iter()
                    .filter(|(_, m)| !m.is_empty())
                    .map(|(id, m)| (id, m.into_iter().map(|x| x.from..x.to).collect()))
                    .collect();
                cx.notify();
            });
        }));
    }

    fn schedule_spell_later(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| this.schedule_spell(cx));
        })
        .detach();
    }

    /// Right-click on a misspelled word: suggestions menu. True when one opened.
    pub(crate) fn open_spell_menu(
        &mut self,
        id: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || self.style_of(id) == Some(style::CODE) {
            return false;
        }
        let Some(dict) = crate::spellcheck::dictionary() else {
            return false;
        };
        let index = self
            .layouts
            .borrow()
            .get(id)
            .map(|l| l.index_for_point(position));
        let Some(index) = index else { return false };
        let text = self.live(id).0;
        let ignored = crate::spellcheck::ignored(cx);
        for miss in dict.check_text(&text, &ignored) {
            if index >= miss.from && index <= miss.to {
                let options = dict.suggestions(&miss.word, 5, &ignored);
                self.spell_menu = Some(SpellMenu {
                    block: id.to_string(),
                    word: miss.word,
                    from: miss.from,
                    to: miss.to,
                    pos: position,
                    options,
                });
                cx.notify();
                return true;
            }
        }
        false
    }

    /// Replace the misspelled word in place; marks around it stay.
    pub(crate) fn replace_misspelling(&mut self, replacement: &str, cx: &mut Context<Self>) {
        let Some(menu) = self.spell_menu.take() else {
            return;
        };
        if !self.texts.contains_key(&menu.block) {
            return;
        }
        if self.active.as_deref() != Some(menu.block.as_str()) {
            self.focus_block(&menu.block, menu.from, cx);
        }
        self.edit(&menu.block, menu.from..menu.to, replacement, cx);
    }

    pub(crate) fn spell_add_word(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.spell_menu.take() else {
            return;
        };
        crate::spellcheck::add_to_dictionary(&menu.word, cx);
        self.schedule_spell(cx);
        cx.notify();
    }

    /// A Tool's Code blocks are the code its harness runs: lint them (350ms debounce).
    pub(crate) fn schedule_lint(&mut self, cx: &mut Context<Self>) {
        let lints = self.object.as_ref().is_some_and(|o| o.type_key == "tool") && !self.read_only;
        if !lints {
            if !self.lint.is_empty() {
                self.lint.clear();
                cx.notify();
            }
            return;
        }
        self.lint_run += 1;
        let run = self.lint_run;
        self.lint_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            let Ok(blocks) = this.update(cx, |this, _| {
                this.flat_ids()
                    .into_iter()
                    .filter(|id| this.style_of(id) == Some(style::CODE) && !this.is_cell(id))
                    .map(|id| {
                        let text = this.live(&id).0;
                        (id, text)
                    })
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            let texts: Vec<String> = blocks.iter().map(|(_, t)| t.clone()).collect();
            let problems = cx
                .background_spawn(async move { crate::codelint::lint_tool_code(&texts) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if run != this.lint_run {
                    return;
                }
                this.lint = blocks
                    .into_iter()
                    .zip(problems)
                    .filter(|(_, p)| !p.is_empty())
                    .map(|((id, _), p)| (id, p))
                    .collect();
                cx.notify();
            });
        }));
    }

    // ───────────────────────────────────────────── drag & drop of blocks

    /// Mouse dragstart: snapshot every drop target.
    pub(crate) fn begin_drag(&mut self, id: &str) {
        self.dragging = Some(id.to_string());
        self.build_drag_rects();
    }

    /// provider.tsx initData/getNodeRect: snapshot every target once.
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
        for (id, g) in self.bot_strips.borrow().iter() {
            if g.bounds.size.height > px(0.) {
                rects.push(crate::editor::DropRect {
                    id: id.clone(),
                    bounds: g.bounds,
                    inner: false,
                    bot: true,
                    doc_end: false,
                });
            }
        }
        let rows = self.rows.borrow().clone();
        for id in self.flat_ids() {
            if self.is_cell(&id) {
                continue;
            }
            let Some(g) = rows.get(&id) else { continue };
            if g.bounds.size.height <= px(0.) {
                continue;
            }
            // Their DropTarget covers the content only: measure from the gutter's right edge.
            let left = if g.gutter {
                g.bounds.left() + self.gutter_w()
            } else {
                g.bounds.left()
            };
            let inner = self.style_of(&id).is_some_and(|s| DROP_INNER.contains(&s));
            rects.push(crate::editor::DropRect {
                id,
                bounds: Bounds::from_corners(
                    point(left, g.bounds.top() - px(1.) - px(2.)),
                    point(
                        g.bounds.right().max(left + px(1.)),
                        g.bounds.bottom() + px(1.),
                    ),
                ),
                inner,
                bot: false,
                doc_end: false,
            });
        }
        let host = self.bounds.get();
        let bottom = rects
            .iter()
            .map(|r| r.bounds.bottom())
            .fold(host.top(), Pixels::max);
        if let Some(last) = self.root_ids().last().cloned() {
            rects.push(crate::editor::DropRect {
                id: last,
                bounds: Bounds::new(
                    point(host.left(), bottom),
                    size(
                        host.size.width.max(px(1.)),
                        (host.bottom() - bottom).max(px(150.)),
                    ),
                ),
                inner: false,
                bot: false,
                doc_end: true,
            });
        }
        self.drag_rects = rects;
    }

    /// provider.tsx checkNodes: hit-test the cache, then the band math.
    pub(crate) fn resolve_drop(&self, p: Point<Pixels>) -> Option<crate::editor::DropHint> {
        const OFFSET: Pixels = px(100.);
        const COL_BAND: Pixels = px(12.);
        for r in &self.drag_rects {
            let b = r.bounds;
            if p.x < b.left() - OFFSET
                || p.x > b.right() + OFFSET
                || p.y < b.top()
                || p.y > b.bottom()
            {
                continue;
            }
            if self.drop_forbidden(&r.id) {
                continue;
            }
            if r.doc_end {
                return Some(crate::editor::DropHint {
                    id: r.id.clone(),
                    position: pos::BOTTOM,
                    bot: false,
                });
            }
            if r.bot {
                return Some(crate::editor::DropHint {
                    id: r.id.clone(),
                    position: pos::BOTTOM,
                    bot: true,
                });
            }
            let h = b.size.height;
            let mut position = if p.x <= b.left() - COL_BAND {
                pos::LEFT
            } else if p.x > b.right() {
                pos::RIGHT
            } else if p.y <= b.top() + h * 0.3 {
                pos::TOP
            } else if p.y >= b.top() + h * 0.7 {
                pos::BOTTOM
            } else {
                pos::INNER_FIRST
            };
            // A block that cannot hold children splits 50/50.
            if position == pos::INNER_FIRST && !r.inner {
                position = if p.y <= b.top() + h * 0.5 {
                    pos::TOP
                } else {
                    pos::BOTTOM
                };
            }
            return Some(crate::editor::DropHint {
                id: r.id.clone(),
                position,
                bot: false,
            });
        }
        None
    }

    /// The dragged blocks (the whole selection when dragging a selected block).
    fn drag_group(&self, dragged: &str) -> Vec<String> {
        if self.selected.len() > 1 && self.selected.iter().any(|s| s == dragged) {
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

    /// provider.tsx onDragMove: retarget for the pointer, keep the edge crawl alive.
    pub(crate) fn drag_move_to(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_point = p;
        let hit = self.resolve_drop(p);
        if let Some(h) = &hit {
            self.last_valid_drop = Some(h.clone());
        }
        if hit != self.drop_hint {
            self.drop_hint = hit;
            cx.notify();
        }
        if self.drag_scroll.is_none() {
            self.drag_scroll = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    let Ok(Some((p, area, touch))) = this.update(cx, |this, _| {
                        let area = this.visible.get();
                        this.dragging
                            .as_ref()
                            .map(|_| (this.drag_point, area, this.touch_lift.is_some()))
                    }) else {
                        return;
                    };
                    let scrolled = cx
                        .update(|window, cx| {
                            // scrollOnMove: 20px edge, 10px max step, speed /100. A
                            // finger covers ~40px and can't reach the last 20px under
                            // a phone's bezel, so a touch drag gets a wider, faster
                            // band: 64px, 16px, /4.
                            let h = window.viewport_size().height;
                            let (edge, step, speed) = if touch {
                                (px(64.), 16., 4.)
                            } else {
                                (px(20.), 10., 100.)
                            };
                            let dy = if p.y < edge {
                                (f32::from(edge - p.y) / speed).ceil().min(step)
                            } else if p.y > h - edge {
                                -(f32::from(p.y - (h - edge)) / speed).ceil().min(step)
                            } else {
                                0.
                            };
                            if dy != 0. {
                                crate::keys::scroll_at(p, dy, area, window, cx);
                            }
                            dy != 0.
                        })
                        .unwrap_or(false);
                    if scrolled {
                        let _ = this.update(cx, |this, cx| {
                            // The snapshot is refreshed after the page moves.
                            this.build_drag_rects();
                            this.drop_hint = this.resolve_drop(this.drag_point);
                            cx.notify();
                        });
                    }
                }
            }));
        }
    }

    /// Shared release: clear the targeting state, then move (or cancel).
    pub(crate) fn finish_drag(
        &mut self,
        hint: Option<crate::editor::DropHint>,
        cx: &mut Context<Self>,
    ) {
        self.drag_scroll = None;
        self.drag_rects.clear();
        self.drop_hint = None;
        self.last_valid_drop = None;
        let dragged = self.dragging.take();
        if let (Some(hint), Some(dragged)) = (hint, dragged) {
            self.dragging = Some(dragged);
            self.on_drop(&hint.id, hint.position, cx);
        }
        cx.notify();
    }

    // ───────────────────────────── touch drag (long-press lift)
    // A touch never starts the handle's drag, so a row is HELD to lift it:
    // the platform's long press, then the finger drives the same targeting as
    // the mouse and the release commits. A row whose text is being edited
    // keeps the long press on its text and lifts only from outside it
    // (gutter, marker, row padding); any other row lifts from anywhere.
    // Readonly pages never lift.

    /// A long press began at `start` (the finger is at `at` now): lift the row
    /// under it. False when there is no row to lift there.
    pub(crate) fn touch_lift_at(
        &mut self,
        start: Point<Pixels>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A fresh hold while lifted means the old release never reached the
        // editor: drop that lift.
        if self.touch_lift.is_some() {
            self.touch_drop(None, cx);
        }
        if self.read_only || self.dragging.is_some() {
            return false;
        }
        // The innermost row with a drag rail under the finger.
        let area = |b: &Bounds<Pixels>| f32::from(b.size.width) * f32::from(b.size.height);
        let row = self
            .rows
            .borrow()
            .iter()
            .filter(|(_, g)| g.gutter && g.bounds.contains(&start))
            .min_by(|a, b| area(&a.1.bounds).total_cmp(&area(&b.1.bounds)))
            .map(|(id, g)| (id.clone(), g.bounds));
        let Some((id, bounds)) = row else {
            return false;
        };
        let editing = self.has_focus && self.active.as_deref() == Some(id.as_str());
        if editing
            && self
                .layouts
                .borrow()
                .get(&id)
                .is_some_and(|l| l.bounds.contains(&start))
        {
            return false;
        }
        // The hold is ours now: no selection under it.
        if editing && let Some(t) = self.texts.get_mut(&id) {
            let c = t.cursor();
            t.sel = c..c;
        }
        let label = self
            .blocks
            .iter()
            .find(|b| b.id == id)
            .map(|b| crate::render::drag_label(&b.content))
            .unwrap_or_default();
        self.touch_lift = Some(crate::editor::TouchLift {
            label,
            row: bounds,
            x0: start.x,
            at,
        });
        self.begin_drag(&id);
        self.drag_move_to(at, window, cx);
        cx.notify();
        true
    }

    /// The lifting finger moved.
    pub(crate) fn touch_move(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lift) = &mut self.touch_lift else {
            return;
        };
        lift.at = at;
        self.drag_move_to(at, window, cx);
        cx.notify();
    }

    /// The finger let go: over a target (`at`) commits; anywhere else, or a
    /// cancelled touch (`None`), drops nothing.
    pub(crate) fn touch_drop(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if self.touch_lift.take().is_none() {
            return;
        }
        let hint = at.and_then(|p| self.resolve_drop(p));
        self.finish_drag(hint, cx);
    }

    fn on_drop(&mut self, target: &str, position: u32, cx: &mut Context<Self>) {
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
        self.flush_all(cx);
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
            tasks.push(self.w_block_move(id, &t, p, cx));
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
            toggles::set_toggle_open(&self.object_id, target, true, cx);
        }
        self.await_then_refresh(tasks, cx);
    }
}

/// Copied blocks on the clipboard, or `None` when it holds something else.
fn clipboard_blocks(item: &ClipboardItem) -> Option<Vec<Block>> {
    item.entries().iter().find_map(|e| match e {
        ClipboardEntry::String(s) => s
            .metadata_json::<ClipboardBlocks>()
            .map(|c| c.roostr_blocks)
            .filter(|b| !b.is_empty()),
        _ => None,
    })
}
