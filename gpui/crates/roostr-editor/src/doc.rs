//! The editor's document model (`src/lib/editor/doc.ts`): one list of block
//! edits, applied in one place.
//!
//! Every structural change the editor makes - split, merge, indent, style
//! flip, divider, insert, remove, move - is an [`Op`]. [`apply_op`] changes
//! the page's block list in place and returns the ops that undo it; the
//! editor persists the same op through its write queue. Local state, the
//! saved DAG and undo are three readings of the same op.
//!
//! Tree rules mirror the engine: a block's children are its `children_ids`
//! in order; roots are blocks no one lists, in `blocks` order.

use std::collections::{HashMap, HashSet};

use roostr_core::types::{Block, BlockContent, pos, style};

/// Where a block sits, as an insert target: the engine's own position words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// Block the position is relative to; "" = append at the root.
    pub target: String,
    pub position: u32,
}

impl Place {
    pub fn new(target: impl Into<String>, position: u32) -> Self {
        Self {
            target: target.into(),
            position,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Content { id: String, content: BlockContent },
    Insert { block: Block, at: Place },
    Remove { id: String },
    Move { id: String, to: Place },
}

pub fn parent_of<'a>(blocks: &'a [Block], id: &str) -> Option<&'a Block> {
    blocks
        .iter()
        .find(|b| b.children_ids.iter().any(|c| c == id))
}

/// Root ids in order: blocks nobody lists as a child.
pub fn root_order(blocks: &[Block]) -> Vec<String> {
    let listed: HashSet<&str> = blocks
        .iter()
        .flat_map(|b| b.children_ids.iter().map(String::as_str))
        .collect();
    blocks
        .iter()
        .filter(|b| !listed.contains(b.id.as_str()))
        .map(|b| b.id.clone())
        .collect()
}

/// The block's current place, expressed so that inserting there puts it back exactly.
pub fn place_of(blocks: &[Block], id: &str) -> Place {
    let parent = parent_of(blocks, id);
    let siblings = match parent {
        Some(p) => p.children_ids.clone(),
        None => root_order(blocks),
    };
    let i = siblings.iter().position(|s| s == id);
    if let Some(i) = i
        && i > 0
    {
        return Place::new(siblings[i - 1].clone(), pos::BOTTOM);
    }
    if let Some(p) = parent {
        return Place::new(p.id.clone(), pos::INNER_FIRST);
    }
    match i.and_then(|i| siblings.get(i + 1)) {
        Some(next) => Place::new(next.clone(), pos::TOP),
        None => Place::new("", pos::BOTTOM),
    }
}

/// Every block in the subtree rooted at `id`, parent before children.
fn subtree(blocks: &[Block], id: &str) -> Vec<Block> {
    let by_id: HashMap<&str, &Block> = blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut out = Vec::new();
    fn visit(by_id: &HashMap<&str, &Block>, id: &str, out: &mut Vec<Block>) {
        let Some(b) = by_id.get(id) else { return };
        out.push((*b).clone());
        for c in &b.children_ids {
            visit(by_id, c, out);
        }
    }
    visit(&by_id, id, &mut out);
    out
}

fn detach(blocks: &mut [Block], id: &str) {
    for b in blocks.iter_mut() {
        if let Some(j) = b.children_ids.iter().position(|c| c == id) {
            b.children_ids.remove(j);
        }
    }
}

/// Put an already-present-or-new block at `at` (block must not be listed anywhere yet).
fn place(blocks: &mut Vec<Block>, block: Block, at: &Place) {
    if let Some(existing) = blocks.iter().position(|b| b.id == block.id) {
        blocks.remove(existing);
    }
    let ref_idx = if at.target.is_empty() {
        None
    } else {
        blocks.iter().position(|b| b.id == at.target)
    };
    let Some(ref_idx) = ref_idx else {
        blocks.push(block);
        return;
    };
    if at.position == pos::INNER || at.position == pos::INNER_FIRST {
        if at.position == pos::INNER {
            blocks[ref_idx].children_ids.push(block.id.clone());
        } else {
            blocks[ref_idx].children_ids.insert(0, block.id.clone());
        }
        blocks.insert(ref_idx + 1, block);
        return;
    }
    let before = at.position == pos::TOP;
    let id = block.id.clone();
    blocks.insert(if before { ref_idx } else { ref_idx + 1 }, block);
    if let Some(parent_idx) = blocks
        .iter()
        .position(|b| b.children_ids.contains(&at.target))
        && blocks[parent_idx].id != id
    {
        let parent = &mut blocks[parent_idx];
        let at_ix = parent
            .children_ids
            .iter()
            .position(|c| *c == at.target)
            .unwrap_or(0);
        parent.children_ids.insert(at_ix + usize::from(!before), id);
    }
}

/// Apply `op` to `blocks` in place; return the ops that undo it (in the
/// order to apply them). Unknown ids are a no-op with no inverse, so an op
/// racing a remote delete never fails mid-keystroke.
pub fn apply_op(blocks: &mut Vec<Block>, op: &Op) -> Vec<Op> {
    match op {
        Op::Content { id, content } => {
            let Some(b) = blocks.iter_mut().find(|x| x.id == *id) else {
                return vec![];
            };
            let before = std::mem::replace(&mut b.content, content.clone());
            vec![Op::Content {
                id: id.clone(),
                content: before,
            }]
        }
        Op::Insert { block, at } => {
            if blocks.iter().any(|b| b.id == block.id) {
                return vec![];
            }
            place(blocks, block.clone(), at);
            vec![Op::Remove {
                id: block.id.clone(),
            }]
        }
        Op::Remove { id } => {
            if !blocks.iter().any(|b| b.id == *id) {
                return vec![];
            }
            let at = place_of(blocks, id);
            let gone = subtree(blocks, id);
            detach(blocks, id);
            let ids: HashSet<&str> = gone.iter().map(|b| b.id.as_str()).collect();
            blocks.retain(|b| !ids.contains(b.id.as_str()));
            // Undo rebuilds the subtree: the root back at its place, each child
            // appended under its parent in order.
            let mut undo = vec![Op::Insert {
                block: Block {
                    children_ids: vec![],
                    ..gone[0].clone()
                },
                at,
            }];
            for b in &gone {
                for c in &b.children_ids {
                    if let Some(child) = gone.iter().skip(1).find(|x| x.id == *c) {
                        undo.push(Op::Insert {
                            block: Block {
                                children_ids: vec![],
                                ..child.clone()
                            },
                            at: Place::new(b.id.clone(), pos::INNER),
                        });
                    }
                }
            }
            undo
        }
        Op::Move { id, to } => {
            if to.target == *id {
                return vec![];
            }
            let Some(b) = blocks.iter().find(|x| x.id == *id).cloned() else {
                return vec![];
            };
            let from = place_of(blocks, id);
            detach(blocks, id);
            place(blocks, b, to);
            vec![Op::Move {
                id: id.clone(),
                to: from,
            }]
        }
    }
}

/// Apply a transaction; the returned ops undo all of it (already in apply order).
pub fn apply_all(blocks: &mut Vec<Block>, ops: &[Op]) -> Vec<Op> {
    let mut inverses: Vec<Vec<Op>> = ops.iter().map(|op| apply_op(blocks, op)).collect();
    inverses.reverse();
    inverses.into_iter().flatten().collect()
}

/// Undo/redo over transactions (one user action = one transaction of ops).
/// Pushing a new transaction clears redo, as every editor does.
pub struct History {
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: 200,
        }
    }
}

impl History {
    /// Record the inverse of an action the user just did.
    pub fn record(&mut self, inverse: Vec<Op>) {
        if inverse.is_empty() {
            return;
        }
        self.undo.push(inverse);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn take_undo(&mut self) -> Option<Vec<Op>> {
        self.undo.pop()
    }

    pub fn take_redo(&mut self) -> Option<Vec<Op>> {
        self.redo.pop()
    }

    /// Applying an undo produced `inverse`: it is what redo replays.
    pub fn push_redo(&mut self, inverse: Vec<Op>) {
        if !inverse.is_empty() {
            self.redo.push(inverse);
        }
    }

    /// Applying a redo produced `inverse`: it goes back on the undo stack.
    pub fn push_undo(&mut self, inverse: Vec<Op>) {
        if !inverse.is_empty() {
            self.undo.push(inverse);
        }
    }
}

// ── Copy and paste of selected blocks ───────────────────────────────

/// The selected blocks' subtrees (selection's topmost ids, in page order), as a self-contained copy.
pub fn copy_slice(blocks: &[Block], top_ids: &[String]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for id in top_ids {
        for b in subtree(blocks, id) {
            if seen.insert(b.id.clone()) {
                out.push(b);
            }
        }
    }
    out
}

/// The slice's roots: blocks no other block in it lists.
fn slice_roots(slice: &[Block]) -> Vec<&Block> {
    let listed: HashSet<&str> = slice
        .iter()
        .flat_map(|b| b.children_ids.iter().map(String::as_str))
        .collect();
    slice
        .iter()
        .filter(|b| !listed.contains(b.id.as_str()))
        .collect()
}

fn text_prefix(s: u32) -> &'static str {
    match s {
        style::HEADER1 => "# ",
        style::HEADER2 => "## ",
        style::HEADER3 => "### ",
        style::QUOTE | style::TOGGLE => "> ",
        style::BULLET => "- ",
        _ => "",
    }
}

/// The slice as plain text for other apps: one line per block, children
/// indented, checkboxes as `- [ ]` / `- [x]`, numbered lists counted.
pub fn slice_text(slice: &[Block]) -> String {
    let by_id: HashMap<&str, &Block> = slice.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut lines = Vec::new();
    fn walk(by_id: &HashMap<&str, &Block>, ids: &[String], depth: usize, lines: &mut Vec<String>) {
        let mut n = 0;
        for id in ids {
            let Some(b) = by_id.get(id.as_str()) else {
                continue;
            };
            let t = b.content.text.as_ref();
            let s = t.map_or(0, |t| t.style);
            n = if s == style::NUMBERED { n + 1 } else { 0 };
            let prefix = if s == style::CHECKBOX {
                if t.and_then(|t| t.checked).unwrap_or(false) {
                    "- [x] ".to_string()
                } else {
                    "- [ ] ".to_string()
                }
            } else if s == style::NUMBERED {
                format!("{n}. ")
            } else {
                text_prefix(s).to_string()
            };
            let body = match t {
                Some(t) => t.text.clone(),
                None => b
                    .content
                    .custom
                    .as_ref()
                    .and_then(|c| c.meta.as_ref())
                    .and_then(|m| m.get("url").or_else(|| m.get("name")).cloned())
                    .unwrap_or_default(),
            };
            if t.is_some() || !body.is_empty() {
                lines.push(format!("{}{prefix}{body}", "  ".repeat(depth)));
            }
            walk(by_id, &b.children_ids, depth + 1, lines);
        }
    }
    let roots: Vec<String> = slice_roots(slice).iter().map(|b| b.id.clone()).collect();
    walk(&by_id, &roots, 0, &mut lines);
    lines.join("\n")
}

/// The inserts that paste `slice` at `at`: fresh ids, roots one after
/// another from `at`, each child under its parent in order. One
/// transaction, so one undo removes the whole paste.
pub fn paste_ops(slice: &[Block], at: Place, new_id: &mut dyn FnMut() -> String) -> Vec<Op> {
    let by_id: HashMap<&str, &Block> = slice.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut ops = Vec::new();
    fn add(
        by_id: &HashMap<&str, &Block>,
        b: &Block,
        place: Place,
        ops: &mut Vec<Op>,
        new_id: &mut dyn FnMut() -> String,
    ) -> String {
        let id = new_id();
        ops.push(Op::Insert {
            block: Block {
                id: id.clone(),
                children_ids: vec![],
                ..b.clone()
            },
            at: place,
        });
        for c in &b.children_ids {
            if let Some(child) = by_id.get(c.as_str()) {
                add(
                    by_id,
                    child,
                    Place::new(id.clone(), pos::INNER),
                    ops,
                    new_id,
                );
            }
        }
        id
    }
    let mut place = at;
    for root in slice_roots(slice) {
        let id = add(&by_id, root, place, &mut ops, new_id);
        place = Place::new(id, pos::BOTTOM);
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use roostr_core::types::TextContent;

    fn tb(id: &str, kids: &[&str]) -> Block {
        Block {
            id: id.into(),
            children_ids: kids.iter().map(|s| s.to_string()).collect(),
            content: BlockContent {
                text: Some(TextContent {
                    text: id.into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn ids(blocks: &[Block]) -> Vec<String> {
        root_order(blocks)
    }

    #[test]
    fn insert_remove_undo() {
        let mut blocks = vec![tb("a", &[]), tb("b", &["c"]), tb("c", &[])];
        let inv = apply_all(
            &mut blocks,
            &[Op::Insert {
                block: tb("x", &[]),
                at: Place::new("a", pos::BOTTOM),
            }],
        );
        assert_eq!(ids(&blocks), vec!["a", "x", "b"]);
        apply_all(&mut blocks, &inv);
        assert_eq!(ids(&blocks), vec!["a", "b"]);
        let inv = apply_all(&mut blocks, &[Op::Remove { id: "b".into() }]);
        assert_eq!(ids(&blocks), vec!["a"]);
        apply_all(&mut blocks, &inv);
        assert_eq!(ids(&blocks), vec!["a", "b"]);
        assert_eq!(
            blocks.iter().find(|b| b.id == "b").unwrap().children_ids,
            vec!["c"]
        );
    }

    #[test]
    fn move_into_and_back() {
        let mut blocks = vec![tb("a", &[]), tb("b", &[])];
        let inv = apply_all(
            &mut blocks,
            &[Op::Move {
                id: "b".into(),
                to: Place::new("a", pos::INNER),
            }],
        );
        assert_eq!(ids(&blocks), vec!["a"]);
        assert_eq!(blocks[0].children_ids, vec!["b"]);
        apply_all(&mut blocks, &inv);
        assert_eq!(ids(&blocks), vec!["a", "b"]);
    }

    #[test]
    fn slice_text_and_paste() {
        let blocks = vec![tb("a", &["b"]), tb("b", &[])];
        let slice = copy_slice(&blocks, &["a".to_string()]);
        assert_eq!(slice_text(&slice), "a\n  b");
        let mut n = 0;
        let ops = paste_ops(&slice, Place::new("z", pos::BOTTOM), &mut || {
            n += 1;
            format!("n{n}")
        });
        assert_eq!(ops.len(), 2);
    }

    fn text_block(id: &str, text: &str, style: u32, checked: Option<bool>, kids: &[&str]) -> Block {
        Block {
            id: id.into(),
            children_ids: kids.iter().map(|s| s.to_string()).collect(),
            content: BlockContent {
                text: Some(TextContent {
                    text: text.into(),
                    style,
                    checked,
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn t(id: &str, text: &str, kids: &[&str]) -> Block {
        text_block(id, text, style::PARAGRAPH, None, kids)
    }

    fn check(id: &str, text: &str, checked: bool, kids: &[&str]) -> Block {
        text_block(id, text, style::CHECKBOX, Some(checked), kids)
    }

    fn text(text: &str) -> BlockContent {
        BlockContent {
            text: Some(TextContent {
                text: text.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// a, b(c, d), e
    fn page() -> Vec<Block> {
        vec![
            t("a", "A", &[]),
            t("b", "B", &["c", "d"]),
            t("c", "C", &[]),
            t("d", "D", &[]),
            t("e", "E", &[]),
        ]
    }

    /// The page as text: roots in order, children in parentheses.
    fn shape(blocks: &[Block]) -> String {
        fn draw(blocks: &[Block], id: &str) -> String {
            let b = blocks
                .iter()
                .find(|b| b.id == id)
                .expect("listed block exists");
            let text = b.content.text.as_ref().map_or("", |t| t.text.as_str());
            if b.children_ids.is_empty() {
                text.to_string()
            } else {
                let kids: Vec<String> = b.children_ids.iter().map(|c| draw(blocks, c)).collect();
                format!("{text}({})", kids.join(","))
            }
        }
        root_order(blocks)
            .iter()
            .map(|id| draw(blocks, id))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn sorted_ids(blocks: &[Block]) -> Vec<String> {
        let mut ids: Vec<String> = blocks.iter().map(|b| b.id.clone()).collect();
        ids.sort();
        ids
    }

    /// Apply, check the result, undo, check the page is exactly as it was.
    fn round_trip(ops: &[Op], expected: &str) {
        let mut blocks = page();
        let inverse = apply_all(&mut blocks, ops);
        assert_eq!(shape(&blocks), expected);
        apply_all(&mut blocks, &inverse);
        assert_eq!(shape(&blocks), "A B(C,D) E");
        assert_eq!(sorted_ids(&blocks), sorted_ids(&page()));
    }

    fn insert(at: &str, position: u32) -> Op {
        Op::Insert {
            block: t("n", "N", &[]),
            at: Place::new(at, position),
        }
    }

    #[test]
    fn insert_at_every_position_and_undo_removes_it() {
        round_trip(&[insert("a", pos::BOTTOM)], "A N B(C,D) E");
        round_trip(&[insert("a", pos::TOP)], "N A B(C,D) E");
        round_trip(&[insert("c", pos::BOTTOM)], "A B(C,N,D) E");
        round_trip(&[insert("b", pos::INNER_FIRST)], "A B(N,C,D) E");
        round_trip(&[insert("b", pos::INNER)], "A B(C,D,N) E");
        round_trip(&[insert("", pos::BOTTOM)], "A B(C,D) E N");
    }

    #[test]
    fn split_and_merge_undo_exactly() {
        let split = [
            Op::Content {
                id: "a".into(),
                content: text("A1"),
            },
            Op::Insert {
                block: t("a2", "A2", &[]),
                at: Place::new("a", pos::BOTTOM),
            },
        ];
        round_trip(&split, "A1 A2 B(C,D) E");
        let merge = [
            Op::Content {
                id: "c".into(),
                content: text("CD"),
            },
            Op::Remove { id: "d".into() },
        ];
        round_trip(&merge, "A B(CD) E");
    }

    #[test]
    fn removing_a_block_with_children_then_undo_rebuilds_the_subtree_in_order() {
        round_trip(&[Op::Remove { id: "b".into() }], "A E");
    }

    #[test]
    fn indent_and_outdent_both_reverse() {
        let mv = |id: &str, target: &str, position: u32| Op::Move {
            id: id.into(),
            to: Place::new(target, position),
        };
        round_trip(&[mv("e", "b", pos::INNER)], "A B(C,D,E)");
        round_trip(&[mv("c", "b", pos::BOTTOM)], "A B(D) C E");
        round_trip(&[mv("d", "a", pos::INNER)], "A(D) B(C) E");
    }

    #[test]
    fn place_of_names_a_place_that_puts_the_block_back() {
        let blocks = page();
        assert_eq!(place_of(&blocks, "c"), Place::new("b", pos::INNER_FIRST));
        assert_eq!(place_of(&blocks, "d"), Place::new("c", pos::BOTTOM));
        assert_eq!(place_of(&blocks, "a"), Place::new("b", pos::TOP));
    }

    #[test]
    fn an_op_on_a_block_a_remote_edit_removed_is_a_harmless_no_op() {
        let mut blocks = page();
        let content = Op::Content {
            id: "gone".into(),
            content: text("x"),
        };
        assert_eq!(apply_op(&mut blocks, &content), vec![]);
        let mv = Op::Move {
            id: "gone".into(),
            to: Place::new("a", pos::BOTTOM),
        };
        assert_eq!(apply_op(&mut blocks, &mv), vec![]);
        assert_eq!(shape(&blocks), "A B(C,D) E");
    }

    #[test]
    fn history_undo_then_redo_replays_and_a_new_action_clears_redo() {
        let mut blocks = page();
        let mut h = History::default();
        h.record(apply_all(
            &mut blocks,
            &[Op::Content {
                id: "a".into(),
                content: text("A!"),
            }],
        ));
        let undo = h.take_undo().expect("undo");
        h.push_redo(apply_all(&mut blocks, &undo));
        assert_eq!(shape(&blocks), "A B(C,D) E");
        let redo = h.take_redo().expect("redo");
        h.push_undo(apply_all(&mut blocks, &redo));
        assert_eq!(shape(&blocks), "A! B(C,D) E");
        let undo = h.take_undo().expect("undo");
        h.push_redo(apply_all(&mut blocks, &undo));
        h.record(apply_all(&mut blocks, &[Op::Remove { id: "e".into() }]));
        assert_eq!(h.take_redo(), None);
    }

    #[test]
    fn copied_blocks_paste_as_new_blocks_keeping_order_nesting_and_ticks() {
        let source = [
            check("x", "Buy milk", true, &["y"]),
            check("y", "2%", false, &[]),
            check("z", "Call Sam", false, &[]),
            t("w", "not copied", &[]),
        ];
        let slice = copy_slice(&source, &["x".into(), "z".into()]);
        let ids: Vec<&str> = slice.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["x", "y", "z"]);
        let mut target = page();
        let mut n = 0;
        let ops = paste_ops(&slice, Place::new("a", pos::BOTTOM), &mut || {
            n += 1;
            format!("n{n}")
        });
        apply_all(&mut target, &ops);
        assert_eq!(shape(&target), "A Buy milk(2%) Call Sam B(C,D) E");
        let checked = |text: &str| {
            target
                .iter()
                .find_map(|b| b.content.text.as_ref().filter(|t| t.text == text))
                .and_then(|t| t.checked)
        };
        assert_eq!(checked("Buy milk"), Some(true));
        assert_eq!(checked("2%"), Some(false));
        assert!(
            !target
                .iter()
                .any(|b| ["x", "y", "z"].contains(&b.id.as_str()))
        );
    }

    #[test]
    fn one_undo_removes_a_whole_paste() {
        let mut blocks = page();
        let before = shape(&blocks);
        let slice = copy_slice(
            &[
                check("x", "One", false, &["y"]),
                check("y", "Two", true, &[]),
            ],
            &["x".into()],
        );
        let mut n = 0;
        let ops = paste_ops(&slice, Place::new("e", pos::BOTTOM), &mut || {
            n += 1;
            format!("p{n}")
        });
        let inverse = apply_all(&mut blocks, &ops);
        assert_eq!(shape(&blocks), "A B(C,D) E One(Two)");
        apply_all(&mut blocks, &inverse);
        assert_eq!(shape(&blocks), before);
    }

    #[test]
    fn copied_blocks_as_plain_text_tick_count_and_indent() {
        let slice = [
            check("x", "Buy milk", true, &["y"]),
            check("y", "2%", false, &[]),
            text_block("h", "Steps", style::HEADER2, None, &[]),
            text_block("n1", "First", style::NUMBERED, None, &[]),
            text_block("n2", "Second", style::NUMBERED, None, &[]),
        ];
        assert_eq!(
            slice_text(&slice),
            "- [x] Buy milk\n  - [ ] 2%\n## Steps\n1. First\n2. Second"
        );
    }
}
