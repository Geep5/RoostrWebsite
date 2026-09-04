/**
 * DAG replay — computeObject(changes) → ObjectJSON, matching the Odin
 * server (glonOdin/src/dag.odin compute_state + jsonx.odin serializers)
 * byte-for-byte on the JSON it serves from /api/objects/:id.
 *
 * Kahn toposort with lexicographic hex-id tie-break, Anytype-style block
 * tree ops with deterministic layout ids (r-/ct-/cd-<changeHex16>-<opIdx>),
 * normalize pass (drop empty layouts, unwrap 1-col rows, reset widths on
 * column-count change), snapshot fast-path, soft delete.
 */
import type { ObjectJSON, ValueJSON, BlockJSON } from "$lib/types";
import type { ChangeJSON, OpJSON, ReplayApi } from "./contracts";

// ── Wire shapes (decoded proto, see proto.ts DECODE_OPTS) ───────────

interface ValueWire {
	stringValue?: string;
	intValue?: number;
	floatValue?: number;
	boolValue?: boolean;
	bytesValue?: string;
	listValue?: { values?: string[] };
	mapValue?: { entries?: Record<string, ValueWire> };
	valuesValue?: { items?: ValueWire[] };
	linkValue?: { targetId?: string; relationKey?: string };
}

interface ContentWire {
	text?: { text?: string; style?: number; marks?: MarkWire[]; checked?: boolean; color?: string };
	custom?: { contentType?: string; data?: string; meta?: Record<string, string> };
	layout?: { style?: number };
	table?: Record<string, never>;
	tableColumn?: Record<string, never>;
	tableRow?: { isHeader?: boolean };
}

interface MarkWire {
	from?: number;
	to?: number;
	type?: number;
	param?: string;
}

interface BlockNode {
	id: string;
	childrenIds: string[];
	content: ContentWire | null | undefined;
	fields?: { entries?: Record<string, ValueWire> } | null;
	align?: number;
	backgroundColor?: string;
}

interface SnapshotWire {
	id?: string;
	typeKey?: string;
	fields?: Record<string, ValueWire>;
	content?: string; // deprecated primary content, base64
	blocks?: BlockNode[];
	deleted?: boolean;
	createdAt?: number;
	updatedAt?: number;
}

// ── Layout constants (glon.LayoutStyle / glon.Position) ─────────────

const LAYOUT_ROW = 0;
const LAYOUT_COLUMN = 1;
const LAYOUT_DIV = 2;

const POS_NONE = 0;
const POS_TOP = 1;
const POS_BOTTOM = 2;
const POS_LEFT = 3;
const POS_RIGHT = 4;
const POS_INNER = 5;
const POS_REPLACE = 6;
const POS_INNER_FIRST = 7;

// ── Topological sort (Kahn, hex-id tie-break) ───────────────────────

function topoSort(changes: ChangeJSON[]): ChangeJSON[] {
	const byHex = new Map<string, ChangeJSON>();
	const inDegree = new Map<string, number>();
	const children = new Map<string, string[]>();

	for (const c of changes) {
		byHex.set(c.id, c);
		inDegree.set(c.id, 0);
	}
	for (const c of changes) {
		let deg = 0;
		for (const phex of c.parentIds) {
			if (!byHex.has(phex)) continue;
			deg++;
			const list = children.get(phex);
			if (list) list.push(c.id);
			else children.set(phex, [c.id]);
		}
		inDegree.set(c.id, deg);
	}

	const queue: string[] = [];
	for (const [hex, deg] of inDegree) if (deg === 0) queue.push(hex);
	queue.sort();

	const result: ChangeJSON[] = [];
	let head = 0;
	while (head < queue.length) {
		const hex = queue[head++];
		result.push(byHex.get(hex)!);
		const deps = children.get(hex);
		if (!deps) continue;
		const freed: string[] = [];
		for (const child of deps) {
			const d = inDegree.get(child)! - 1;
			inDegree.set(child, d);
			if (d === 0) freed.push(child);
		}
		freed.sort();
		for (const f of freed) queue.push(f);
	}
	return result;
}

// ── Block tree ──────────────────────────────────────────────────────

interface BlockTree {
	byId: Map<string, BlockNode>;
	parent: Map<string, string>;
	rootIds: string[];
}

function cloneBlock(b: BlockNode): BlockNode {
	return { ...b, childrenIds: [...(b.childrenIds ?? [])] };
}

function buildTree(blocks: BlockNode[]): BlockTree {
	const tree: BlockTree = { byId: new Map(), parent: new Map(), rootIds: [] };
	for (const b of blocks) {
		if (tree.byId.has(b.id)) continue;
		tree.byId.set(b.id, cloneBlock(b));
	}
	for (const b of tree.byId.values()) {
		for (const cid of b.childrenIds) {
			if (tree.byId.has(cid)) tree.parent.set(cid, b.id);
		}
	}
	for (const b of blocks) {
		if (!tree.parent.has(b.id) && !tree.rootIds.includes(b.id)) tree.rootIds.push(b.id);
	}
	return tree;
}

function serializeTree(tree: BlockTree): BlockNode[] {
	const out: BlockNode[] = [];
	const visit = (id: string) => {
		const b = tree.byId.get(id);
		if (!b) return;
		out.push(b);
		for (const cid of b.childrenIds) visit(cid);
	};
	for (const id of tree.rootIds) visit(id);
	return out;
}

function isLayout(b: BlockNode | undefined, style?: number): b is BlockNode {
	const l = b?.content?.layout;
	if (!l) return false;
	return style === undefined || (l.style ?? 0) === style;
}

function unlink(tree: BlockTree, blockId: string): void {
	const pid = tree.parent.get(blockId);
	if (pid !== undefined) {
		const p = tree.byId.get(pid);
		if (p) {
			const idx = p.childrenIds.indexOf(blockId);
			if (idx !== -1) p.childrenIds.splice(idx, 1);
		}
		tree.parent.delete(blockId);
	} else {
		const idx = tree.rootIds.indexOf(blockId);
		if (idx !== -1) tree.rootIds.splice(idx, 1);
	}
}

function linkChild(tree: BlockTree, parent: BlockNode, childId: string, index: number): void {
	const idx = Math.max(0, Math.min(index, parent.childrenIds.length));
	parent.childrenIds.splice(idx, 0, childId);
	tree.parent.set(childId, parent.id);
}

function inSubtree(tree: BlockTree, rootId: string, candidateId: string): boolean {
	let cur: string | undefined = candidateId;
	while (cur !== undefined) {
		if (cur === rootId) return true;
		cur = tree.parent.get(cur);
	}
	return false;
}

function makeLayoutBlock(tree: BlockTree, id: string, style: number): BlockNode {
	const b: BlockNode = { id, childrenIds: [], content: { layout: { style } } };
	tree.byId.set(id, b);
	return b;
}

/** Deterministic id, suffixed on the (rare) replayed-twice collision. */
function freshId(tree: BlockTree, id: string): string {
	let out = id;
	while (tree.byId.has(out)) out += "x";
	return out;
}

/** Put newId into target's slot (parent childrenIds entry or root position). */
function replaceSlot(tree: BlockTree, targetId: string, newId: string): void {
	const pid = tree.parent.get(targetId);
	if (pid !== undefined) {
		const p = tree.byId.get(pid)!;
		const idx = p.childrenIds.indexOf(targetId);
		if (idx !== -1) p.childrenIds[idx] = newId;
		tree.parent.delete(targetId);
		tree.parent.set(newId, pid);
	} else {
		const idx = tree.rootIds.indexOf(targetId);
		if (idx !== -1) tree.rootIds[idx] = newId;
		else tree.rootIds.push(newId);
	}
}

function removeSubtree(tree: BlockTree, blockId: string): void {
	if (!tree.byId.has(blockId)) return;
	unlink(tree, blockId);
	const stack = [blockId];
	while (stack.length > 0) {
		const id = stack.pop()!;
		const b = tree.byId.get(id);
		if (!b) continue;
		for (const cid of b.childrenIds) stack.push(cid);
		tree.parent.delete(id);
		tree.byId.delete(id);
	}
}

/**
 * Anytype's moveFromSide: dropping a block LEFT/RIGHT of a target creates
 * (or reuses) a Row(Column(...)) wrapper with deterministic ids seeded
 * from opKey.
 */
function moveFromSide(tree: BlockTree, target: BlockNode, block: BlockNode, left: boolean, opKey: string): void {
	let column: BlockNode | undefined;
	let row: BlockNode | undefined;

	const pid = tree.parent.get(target.id);
	const parentBlock = pid !== undefined ? tree.byId.get(pid) : undefined;

	if (isLayout(target, LAYOUT_COLUMN) && isLayout(parentBlock, LAYOUT_ROW)) {
		column = target;
		row = parentBlock;
	} else if (isLayout(parentBlock, LAYOUT_COLUMN)) {
		const gpid = tree.parent.get(parentBlock.id);
		const gp = gpid !== undefined ? tree.byId.get(gpid) : undefined;
		if (isLayout(gp, LAYOUT_ROW)) {
			column = parentBlock;
			row = gp;
		}
	}

	if (!row || !column) {
		const rowBlock = makeLayoutBlock(tree, freshId(tree, `r-${opKey}`), LAYOUT_ROW);
		const colBlock = makeLayoutBlock(tree, freshId(tree, `ct-${opKey}`), LAYOUT_COLUMN);
		replaceSlot(tree, target.id, rowBlock.id);
		linkChild(tree, rowBlock, colBlock.id, 0);
		linkChild(tree, colBlock, target.id, 0);
		row = rowBlock;
		column = colBlock;
	}

	const newCol = makeLayoutBlock(tree, freshId(tree, `cd-${opKey}`), LAYOUT_COLUMN);
	linkChild(tree, newCol, block.id, 0);

	const colPos = row.childrenIds.indexOf(column.id);
	linkChild(tree, row, newCol.id, left ? colPos : colPos + 1);
}

/** Place a registered, currently-unlinked block relative to target. */
function insertTo(tree: BlockTree, block: BlockNode, targetId: string, position: number, opKey: string): void {
	const target = targetId ? tree.byId.get(targetId) : undefined;
	if (!target) {
		tree.rootIds.push(block.id); // degraded: content is never lost
		return;
	}

	switch (position) {
		case POS_INNER:
			linkChild(tree, target, block.id, target.childrenIds.length);
			break;
		case POS_INNER_FIRST:
			linkChild(tree, target, block.id, 0);
			break;
		case POS_TOP:
		case POS_BOTTOM: {
			const before = position === POS_TOP;
			const pid = tree.parent.get(target.id);
			if (pid === undefined) {
				const idx = tree.rootIds.indexOf(target.id);
				tree.rootIds.splice(before ? idx : idx + 1, 0, block.id);
			} else {
				const p = tree.byId.get(pid)!;
				const idx = p.childrenIds.indexOf(target.id);
				linkChild(tree, p, block.id, before ? idx : idx + 1);
			}
			break;
		}
		case POS_REPLACE:
			replaceSlot(tree, target.id, block.id);
			if (block.childrenIds.length === 0) {
				// Inherit the replaced block's children.
				block.childrenIds = [...target.childrenIds];
				for (const cid of block.childrenIds) tree.parent.set(cid, block.id);
				target.childrenIds = [];
				tree.byId.delete(target.id);
			} else {
				removeSubtree(tree, target.id);
			}
			break;
		case POS_LEFT:
		case POS_RIGHT:
			moveFromSide(tree, target, block, position === POS_LEFT, opKey);
			break;
		default:
			insertTo(tree, block, targetId, POS_BOTTOM, opKey);
	}
}

function applyBlockAdd(
	tree: BlockTree,
	op: NonNullable<OpJSON["blockAdd"]>,
	opKey: string,
): void {
	const wire = op.block as unknown as BlockNode | null | undefined;
	if (!wire?.id || tree.byId.has(wire.id)) return;
	const block = cloneBlock(wire);
	tree.byId.set(block.id, block);

	const position = op.position ?? POS_NONE;
	if (position === POS_NONE && !op.targetId) {
		// Legacy semantics: append; parent_id nests, after_id ignored.
		const parent = op.parentId ? tree.byId.get(op.parentId) : undefined;
		if (parent) linkChild(tree, parent, block.id, parent.childrenIds.length);
		else tree.rootIds.push(block.id);
		return;
	}
	insertTo(tree, block, op.targetId ?? "", position, opKey);
}

function applyBlockMove(
	tree: BlockTree,
	op: NonNullable<OpJSON["blockMove"]>,
	opKey: string,
): void {
	const block = tree.byId.get(op.blockId);
	if (!block) return;

	const position = op.position ?? POS_NONE;
	if (position === POS_NONE && !op.targetId) {
		// Parent/after addressing.
		const parentId = op.newParentId ?? "";
		if (parentId && (parentId === op.blockId || inSubtree(tree, op.blockId, parentId))) return;
		unlink(tree, op.blockId);
		if (!parentId) {
			const idx = op.afterId ? tree.rootIds.indexOf(op.afterId) : -1;
			if (idx !== -1) tree.rootIds.splice(idx + 1, 0, op.blockId);
			else if (op.afterId) tree.rootIds.push(op.blockId);
			else tree.rootIds.unshift(op.blockId); // "" = first
		} else {
			const parent = tree.byId.get(parentId);
			if (!parent) {
				tree.rootIds.push(op.blockId); // degraded: keep reachable
				return;
			}
			const idx = op.afterId ? parent.childrenIds.indexOf(op.afterId) : -1;
			linkChild(tree, parent, op.blockId, idx === -1 ? 0 : idx + 1);
		}
		return;
	}

	const targetId = op.targetId ?? "";
	if (!targetId || !tree.byId.has(targetId)) return;
	if (inSubtree(tree, op.blockId, targetId)) return; // cycle guard

	unlink(tree, op.blockId);
	insertTo(tree, block, targetId, position, opKey);
}

// ── Normalize ───────────────────────────────────────────────────────

function captureRowCounts(tree: BlockTree): Map<string, number> {
	const counts = new Map<string, number>();
	for (const b of tree.byId.values()) {
		if (isLayout(b, LAYOUT_ROW)) counts.set(b.id, b.childrenIds.length);
	}
	return counts;
}

function normalize(tree: BlockTree, beforeCounts: Map<string, number>): void {
	let dirty = true;
	while (dirty) {
		dirty = false;

		// 1. Empty structural layouts removed.
		for (const b of [...tree.byId.values()]) {
			if (!isLayout(b)) continue;
			const style = b.content!.layout!.style ?? 0;
			if (style !== LAYOUT_ROW && style !== LAYOUT_COLUMN && style !== LAYOUT_DIV) continue;
			if (b.childrenIds.length !== 0) continue;
			unlink(tree, b.id);
			tree.byId.delete(b.id);
			dirty = true;
		}

		// 2. Single-column rows unwrap.
		for (const row of [...tree.byId.values()]) {
			if (!isLayout(row, LAYOUT_ROW) || row.childrenIds.length !== 1) continue;
			const col = tree.byId.get(row.childrenIds[0]);
			if (!isLayout(col, LAYOUT_COLUMN)) continue;

			const hoisted = [...col.childrenIds];
			const pid = tree.parent.get(row.id);
			if (pid !== undefined) {
				const p = tree.byId.get(pid)!;
				const idx = p.childrenIds.indexOf(row.id);
				p.childrenIds.splice(idx, 1, ...hoisted);
				for (const cid of hoisted) tree.parent.set(cid, pid);
				tree.parent.delete(row.id);
			} else {
				const idx = tree.rootIds.indexOf(row.id);
				tree.rootIds.splice(idx === -1 ? tree.rootIds.length : idx, 1, ...hoisted);
				for (const cid of hoisted) tree.parent.delete(cid);
			}
			tree.parent.delete(col.id);
			tree.byId.delete(row.id);
			tree.byId.delete(col.id);
			dirty = true;
		}
	}

	// 3. Width reset on rows whose column count changed.
	for (const row of tree.byId.values()) {
		if (!isLayout(row, LAYOUT_ROW)) continue;
		if (beforeCounts.get(row.id) === row.childrenIds.length) continue;
		for (const cid of row.childrenIds) {
			const col = tree.byId.get(cid);
			if (col?.fields?.entries && "width" in col.fields.entries) {
				delete col.fields.entries["width"];
			}
		}
	}
}

// ── JSON serialization (mirrors glonOdin jsonx.odin exactly) ────────

function valueToJSON(v: ValueWire | null | undefined): ValueJSON {
	if (v == null) return {};
	if (v.stringValue !== undefined) return { stringValue: v.stringValue };
	if (v.intValue !== undefined) return { intValue: v.intValue };
	if (v.floatValue !== undefined) return { floatValue: v.floatValue };
	if (v.boolValue !== undefined) return { boolValue: v.boolValue };
	if (v.bytesValue !== undefined) return { bytesValue: "" } as ValueJSON; // bytes aren't surfaced to the UI
	if (v.listValue !== undefined) return { listValue: { values: [...(v.listValue?.values ?? [])] } };
	if (v.mapValue !== undefined) {
		const entries: Record<string, ValueJSON> = {};
		for (const [k, e] of Object.entries(v.mapValue?.entries ?? {})) entries[k] = valueToJSON(e);
		return { mapValue: { entries } };
	}
	if (v.valuesValue !== undefined) {
		return { valuesValue: { items: (v.valuesValue?.items ?? []).map(valueToJSON) } };
	}
	if (v.linkValue !== undefined) {
		return {
			linkValue: {
				targetId: v.linkValue?.targetId ?? "",
				relationKey: v.linkValue?.relationKey ?? "",
			},
		};
	}
	return {};
}

function blockToJSON(b: BlockNode): BlockJSON {
	const content: BlockJSON["content"] = {};
	const c = b.content;
	if (c?.text !== undefined) {
		const t = c.text ?? {};
		content.text = {
			text: t.text ?? "",
			style: t.style ?? 0,
			marks: (t.marks ?? []).map((m) => {
				const mark: { from: number; to: number; type: number; param?: string } = {
					from: m.from ?? 0,
					to: m.to ?? 0,
					type: m.type ?? 0,
				};
				if (m.param) mark.param = m.param;
				return mark;
			}),
			checked: t.checked ?? false,
			color: t.color ?? "",
		};
	} else if (c?.custom !== undefined) {
		content.custom = {
			contentType: c.custom?.contentType ?? "",
			meta: { ...(c.custom?.meta ?? {}) },
		};
	} else if (c?.layout !== undefined) {
		content.layout = { style: c.layout?.style ?? 0 };
	} else if (c?.table !== undefined) {
		content.table = {};
	} else if (c?.tableColumn !== undefined) {
		content.tableColumn = {};
	} else if (c?.tableRow !== undefined) {
		content.tableRow = { isHeader: c.tableRow?.isHeader ?? false };
	}

	const out: BlockJSON = { id: b.id, childrenIds: [...b.childrenIds], content };
	const entries = b.fields?.entries;
	if (entries && Object.keys(entries).length > 0) {
		const jsonEntries: Record<string, ValueJSON> = {};
		for (const [k, v] of Object.entries(entries)) jsonEntries[k] = valueToJSON(v);
		out.fields = { entries: jsonEntries };
	}
	if ((b.align ?? 0) !== 0) out.align = b.align;
	if (b.backgroundColor) out.backgroundColor = b.backgroundColor;
	return out;
}

// ── State computation (mirrors dag.odin compute_state) ──────────────

export function computeObject(changes: ChangeJSON[]): ObjectJSON | null {
	if (changes.length === 0) return null;

	const sorted = topoSort(changes);

	let typeKey = "";
	let deleted = false;
	let createdAt = 0;
	const fields = new Map<string, ValueWire>();

	// Most recent snapshot (by timestamp) skips the replay prefix.
	let snapshotIdx = -1;
	let snapshotTs = -1;
	for (let i = 0; i < sorted.length; i++) {
		const snap = sorted[i].snapshot;
		if (snap != null && sorted[i].timestamp > snapshotTs) {
			snapshotIdx = i;
			snapshotTs = sorted[i].timestamp;
		}
	}

	let initialBlocks: BlockNode[] = [];
	let startIdx = 0;
	if (snapshotIdx >= 0) {
		const snap = sorted[snapshotIdx].snapshot as SnapshotWire;
		typeKey = snap.typeKey ?? "";
		deleted = snap.deleted ?? false;
		createdAt = snap.createdAt ?? 0;
		for (const [k, v] of Object.entries(snap.fields ?? {})) fields.set(k, v);
		initialBlocks = snap.blocks ?? [];
		startIdx = snapshotIdx + 1;
	}

	const tree = buildTree(initialBlocks);
	if (snapshotIdx >= 0) {
		const snap = sorted[snapshotIdx].snapshot as SnapshotWire;
		// Deprecated snap.content → __content__ block (stays internal).
		if (snap.content && snap.content.length > 0 && !tree.byId.has("__content__")) {
			const b: BlockNode = {
				id: "__content__",
				childrenIds: [],
				content: { custom: { contentType: "glon/raw", data: snap.content, meta: {} } },
			};
			tree.byId.set(b.id, b);
			tree.rootIds.push(b.id);
		}
	}

	let maxTs = 0;
	for (let i = startIdx; i < sorted.length; i++) {
		const change = sorted[i];
		if (change.timestamp > maxTs) maxTs = change.timestamp;

		const rowCounts = captureRowCounts(tree);
		const prefix = change.id.slice(0, 16);
		let touched = false;

		for (let opIdx = 0; opIdx < change.ops.length; opIdx++) {
			const op = change.ops[opIdx];
			const opKey = `${prefix}-${opIdx}`;
			if (op.objectCreate) {
				typeKey = op.objectCreate.typeKey ?? "";
				createdAt = change.timestamp;
				// A create AFTER a delete is a revival (restore-from-bin).
				deleted = false;
			} else if (op.fieldSet) {
				fields.set(op.fieldSet.key, op.fieldSet.value as ValueWire);
			} else if (op.fieldDelete) {
				fields.delete(op.fieldDelete.key);
			} else if (op.objectDelete) {
				deleted = true;
			} else if (op.blockAdd) {
				applyBlockAdd(tree, op.blockAdd, opKey);
				touched = true;
			} else if (op.blockRemove) {
				removeSubtree(tree, op.blockRemove.blockId);
				touched = true;
			} else if (op.blockUpdate) {
				const b = tree.byId.get(op.blockUpdate.blockId);
				if (b) b.content = op.blockUpdate.content as ContentWire;
			} else if (op.blockMove) {
				applyBlockMove(tree, op.blockMove, opKey);
				touched = true;
			} else if (op.blockSetAlign) {
				const b = tree.byId.get(op.blockSetAlign.blockId);
				if (b) b.align = op.blockSetAlign.align ?? 0;
			} else if (op.blockSetBackground) {
				const b = tree.byId.get(op.blockSetBackground.blockId);
				if (b) b.backgroundColor = op.blockSetBackground.color ?? "";
			}
		}

		if (touched) normalize(tree, rowCounts);
	}

	const fieldsJSON: Record<string, ValueJSON> = {};
	for (const [k, v] of fields) fieldsJSON[k] = valueToJSON(v);

	const blocks: BlockJSON[] = [];
	for (const b of serializeTree(tree)) {
		if (b.id === "__content__") continue; // legacy primary-content block stays internal
		blocks.push(blockToJSON(b));
	}

	return {
		id: changes[0].objectId,
		typeKey,
		fields: fieldsJSON,
		blocks,
		deleted,
		createdAt,
		updatedAt: maxTs,
	};
}

export const replay: ReplayApi = { computeObject };
