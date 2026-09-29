/**
 * The editor's op model: every edit applies in place and its inverse puts
 * the page back exactly - the property undo and write-behind rely on.
 */
import { expect, test } from "bun:test";
import { applyAll, applyOp, History, placeOf, rootOrder, type Op } from "../src/lib/editor/doc";
import { Pos, type BlockJSON } from "../src/lib/types";

const t = (id: string, text: string, childrenIds: string[] = []): BlockJSON => ({ id, childrenIds, content: { text: { text, style: 0 } } });
/** a, b(c, d), e */
const page = (): BlockJSON[] => [t("a", "A"), t("b", "B", ["c", "d"]), t("c", "C"), t("d", "D"), t("e", "E")];
const shape = (blocks: BlockJSON[]): string => {
	const byId = new Map(blocks.map((b) => [b.id, b]));
	const draw = (id: string): string => {
		const b = byId.get(id)!;
		const text = b.content.text?.text ?? "";
		return b.childrenIds.length ? `${text}(${b.childrenIds.map(draw).join(",")})` : text;
	};
	return rootOrder(blocks).map(draw).join(" ");
};
/** Apply, check the result, undo, check the page is exactly as it was. */
function roundTrip(ops: Op[], expected: string): void {
	const blocks = page();
	const before = JSON.stringify(blocks);
	const inverse = applyAll(blocks, ops);
	expect(shape(blocks)).toBe(expected);
	applyAll(blocks, inverse);
	expect(shape(blocks)).toBe("A B(C,D) E");
	expect(JSON.parse(JSON.stringify(blocks)).map((b: BlockJSON) => b.id).sort()).toEqual(JSON.parse(before).map((b: BlockJSON) => b.id).sort());
}

test("insert at every position, and undo removes it", () => {
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "a", position: Pos.BOTTOM } }], "A N B(C,D) E");
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "a", position: Pos.TOP } }], "N A B(C,D) E");
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "c", position: Pos.BOTTOM } }], "A B(C,N,D) E");
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "b", position: Pos.INNER_FIRST } }], "A B(N,C,D) E");
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "b", position: Pos.INNER } }], "A B(C,D,N) E");
	roundTrip([{ kind: "insert", block: t("n", "N"), at: { target: "", position: Pos.BOTTOM } }], "A B(C,D) E N");
});

test("split (edit + insert below) and merge (edit + remove) undo exactly", () => {
	const split: Op[] = [
		{ kind: "content", id: "a", content: { text: { text: "A1", style: 0 } } },
		{ kind: "insert", block: t("a2", "A2"), at: { target: "a", position: Pos.BOTTOM } },
	];
	roundTrip(split, "A1 A2 B(C,D) E");
	const merge: Op[] = [
		{ kind: "content", id: "c", content: { text: { text: "CD", style: 0 } } },
		{ kind: "remove", id: "d" },
	];
	roundTrip(merge, "A B(CD) E");
});

test("removing a block with children, then undo, rebuilds the whole subtree in order", () => {
	roundTrip([{ kind: "remove", id: "b" }], "A E");
});

test("indent under the previous sibling and outdent below the parent both reverse", () => {
	roundTrip([{ kind: "move", id: "e", to: { target: "b", position: Pos.INNER } }], "A B(C,D,E)");
	roundTrip([{ kind: "move", id: "c", to: { target: "b", position: Pos.BOTTOM } }], "A B(D) C E");
	roundTrip([{ kind: "move", id: "d", to: { target: "a", position: Pos.INNER } }], "A(D) B(C) E");
});

test("placeOf names a place that puts the block back", () => {
	const blocks = page();
	expect(placeOf(blocks, "c")).toEqual({ target: "b", position: Pos.INNER_FIRST });
	expect(placeOf(blocks, "d")).toEqual({ target: "c", position: Pos.BOTTOM });
	expect(placeOf(blocks, "a")).toEqual({ target: "b", position: Pos.TOP });
});

test("an op on a block a remote edit already removed is a harmless no-op", () => {
	const blocks = page();
	expect(applyOp(blocks, { kind: "content", id: "gone", content: { text: { text: "x", style: 0 } } })).toEqual([]);
	expect(applyOp(blocks, { kind: "move", id: "gone", to: { target: "a", position: Pos.BOTTOM } })).toEqual([]);
	expect(shape(blocks)).toBe("A B(C,D) E");
});

test("history: undo then redo replays, and a new action clears redo", () => {
	const blocks = page();
	const h = new History();
	h.record(applyAll(blocks, [{ kind: "content", id: "a", content: { text: { text: "A!", style: 0 } } }]));
	h.pushRedo(applyAll(blocks, h.takeUndo()!));
	expect(shape(blocks)).toBe("A B(C,D) E");
	h.pushUndo(applyAll(blocks, h.takeRedo()!));
	expect(shape(blocks)).toBe("A! B(C,D) E");
	h.pushRedo(applyAll(blocks, h.takeUndo()!));
	h.record(applyAll(blocks, [{ kind: "remove", id: "e" }]));
	expect(h.takeRedo()).toBeUndefined();
});
