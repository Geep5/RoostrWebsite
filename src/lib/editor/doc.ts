/**
 * The editor's document model: one list of block edits, applied in one place.
 *
 * Every structural change the editor makes - split, merge, indent, style
 * flip, divider, insert, remove, move - is an `Op`. `applyOp` changes the
 * page's block list in place (the same array the screen renders from) and
 * returns the op that undoes it; `opWrite` is the engine mutation that
 * persists it. So local state, the saved DAG and undo can never disagree:
 * they are three readings of the same op.
 *
 * Tree rules mirror the engine (core/dag.odin insert_to): a block's
 * children are its `childrenIds` in order; roots are blocks no one lists,
 * in `blocks` array order.
 *
 * Framework-free and DOM-free, so it is unit-tested with plain `bun test`.
 */
import { Pos, type BlockJSON } from "../types";

export type Content = BlockJSON["content"];

/** Where a block sits, as an insert target: the engine's own position words. */
export interface Place {
	/** Block the position is relative to; "" = append at the root. */
	target: string;
	position: number;
}

export type Op =
	| { kind: "content"; id: string; content: Content }
	| { kind: "insert"; block: BlockJSON; at: Place }
	| { kind: "remove"; id: string }
	| { kind: "move"; id: string; to: Place };

/** An op's engine write: `note`-shaped, so it runs through the editor's write queue. */
export interface OpWriter {
	blockUpdate(objectId: string, blockId: string, content: Content): Promise<unknown>;
	blockAdd(objectId: string, block: Partial<BlockJSON>, targetId?: string, position?: number): Promise<unknown>;
	blockRemove(objectId: string, blockId: string): Promise<unknown>;
	blockMove(objectId: string, blockId: string, targetId: string, position: number): Promise<unknown>;
}

/** A plain deep copy (Svelte `$state` proxies do not survive structuredClone; JSON does). */
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export function parentOf(blocks: BlockJSON[], id: string): BlockJSON | undefined {
	return blocks.find((b) => b.childrenIds.includes(id));
}

/** Root ids in order: blocks nobody lists as a child. */
export function rootOrder(blocks: BlockJSON[]): string[] {
	const listed = new Set(blocks.flatMap((b) => b.childrenIds));
	return blocks.filter((b) => !listed.has(b.id)).map((b) => b.id);
}

/** The block's current place, expressed so that inserting there puts it back exactly. */
export function placeOf(blocks: BlockJSON[], id: string): Place {
	const parent = parentOf(blocks, id);
	const siblings = parent ? parent.childrenIds : rootOrder(blocks);
	const i = siblings.indexOf(id);
	if (i > 0) return { target: siblings[i - 1], position: Pos.BOTTOM };
	if (parent) return { target: parent.id, position: Pos.INNER_FIRST };
	const next = siblings[i + 1];
	return next ? { target: next, position: Pos.TOP } : { target: "", position: Pos.BOTTOM };
}

/** Every block in the subtree rooted at `id`, parent before children. */
function subtree(blocks: BlockJSON[], id: string): BlockJSON[] {
	const byId = new Map(blocks.map((b) => [b.id, b]));
	const out: BlockJSON[] = [];
	const visit = (bid: string) => {
		const b = byId.get(bid);
		if (!b) return;
		out.push(b);
		for (const c of b.childrenIds) visit(c);
	};
	visit(id);
	return out;
}

function detach(blocks: BlockJSON[], id: string): void {
	for (const b of blocks) {
		const j = b.childrenIds.indexOf(id);
		if (j >= 0) b.childrenIds.splice(j, 1);
	}
}

/** Put an already-present-or-new block at `at` (block must not be listed anywhere yet). */
function place(blocks: BlockJSON[], block: BlockJSON, at: Place): void {
	const existing = blocks.findIndex((b) => b.id === block.id);
	if (existing >= 0) blocks.splice(existing, 1);
	const ref = at.target ? blocks.find((b) => b.id === at.target) : undefined;
	if (!ref) {
		blocks.push(block);
		return;
	}
	const refIdx = blocks.indexOf(ref);
	if (at.position === Pos.INNER || at.position === Pos.INNER_FIRST) {
		if (at.position === Pos.INNER) ref.childrenIds.push(block.id);
		else ref.childrenIds.unshift(block.id);
		blocks.splice(refIdx + 1, 0, block);
		return;
	}
	const before = at.position === Pos.TOP;
	blocks.splice(before ? refIdx : refIdx + 1, 0, block);
	const parent = parentOf(blocks, at.target);
	if (parent && parent.id !== block.id) parent.childrenIds.splice(parent.childrenIds.indexOf(at.target) + (before ? 0 : 1), 0, block.id);
}

/**
 * Apply `op` to `blocks` in place; return the ops that undo it (in the
 * order to apply them). Unknown ids are a no-op with no inverse, so an op
 * racing a remote delete never throws mid-keystroke.
 */
export function applyOp(blocks: BlockJSON[], op: Op): Op[] {
	switch (op.kind) {
		case "content": {
			const b = blocks.find((x) => x.id === op.id);
			if (!b) return [];
			const before = clone(b.content);
			b.content = clone(op.content);
			return [{ kind: "content", id: op.id, content: before }];
		}
		case "insert": {
			if (blocks.some((b) => b.id === op.block.id)) return [];
			place(blocks, clone(op.block), op.at);
			return [{ kind: "remove", id: op.block.id }];
		}
		case "remove": {
			if (!blocks.some((b) => b.id === op.id)) return [];
			const at = placeOf(blocks, op.id);
			const gone = subtree(blocks, op.id).map(clone);
			detach(blocks, op.id);
			const ids = new Set(gone.map((b) => b.id));
			for (let i = blocks.length - 1; i >= 0; i--) if (ids.has(blocks[i].id)) blocks.splice(i, 1);
			// Undo rebuilds the subtree: the root back at its place, each child
			// appended under its parent in order.
			const [root, ...rest] = gone;
			const undo: Op[] = [{ kind: "insert", block: { ...root, childrenIds: [] }, at }];
			for (const b of gone) for (const c of b.childrenIds) {
				const child = rest.find((x) => x.id === c)!;
				undo.push({ kind: "insert", block: { ...child, childrenIds: [] }, at: { target: b.id, position: Pos.INNER } });
			}
			return undo;
		}
		case "move": {
			const b = blocks.find((x) => x.id === op.id);
			if (!b || op.to.target === op.id) return [];
			const from = placeOf(blocks, op.id);
			detach(blocks, op.id);
			place(blocks, b, op.to);
			return [{ kind: "move", id: op.id, to: from }];
		}
	}
}

/** Persist `op` through `w` (the editor's queued writer). */
export function opWrite(w: OpWriter, objectId: string, op: Op): Promise<unknown> {
	switch (op.kind) {
		case "content":
			return w.blockUpdate(objectId, op.id, clone(op.content));
		case "insert":
			return op.at.target ? w.blockAdd(objectId, clone(op.block), op.at.target, op.at.position) : w.blockAdd(objectId, clone(op.block));
		case "remove":
			return w.blockRemove(objectId, op.id);
		case "move":
			return op.to.target ? w.blockMove(objectId, op.id, op.to.target, op.to.position) : w.blockMove(objectId, op.id, "", Pos.BOTTOM);
	}
}

/**
 * Undo/redo over transactions (one user action = one transaction of ops).
 * Pushing a new transaction clears redo, as every editor does.
 */
export class History {
	private undoStack: Op[][] = [];
	private redoStack: Op[][] = [];
	constructor(private readonly limit = 200) {}

	/** Record the inverse of an action the user just did. */
	record(inverse: Op[]): void {
		if (inverse.length === 0) return;
		this.undoStack.push(inverse);
		if (this.undoStack.length > this.limit) this.undoStack.shift();
		this.redoStack = [];
	}
	/** The next transaction to apply for undo; call `redone` with the inverse it produced. */
	takeUndo(): Op[] | undefined {
		return this.undoStack.pop();
	}
	takeRedo(): Op[] | undefined {
		return this.redoStack.pop();
	}
	/** Applying an undo produced `inverse`: it is what redo replays. */
	pushRedo(inverse: Op[]): void {
		if (inverse.length > 0) this.redoStack.push(inverse);
	}
	/** Applying a redo produced `inverse`: it goes back on the undo stack. */
	pushUndo(inverse: Op[]): void {
		if (inverse.length > 0) this.undoStack.push(inverse);
	}
}

/** Apply a transaction; the returned ops undo all of it (already in apply order). */
export function applyAll(blocks: BlockJSON[], ops: Op[]): Op[] {
	const inverses: Op[][] = [];
	for (const op of ops) inverses.push(applyOp(blocks, op));
	return inverses.reverse().flat();
}
