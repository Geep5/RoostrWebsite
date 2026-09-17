/**
 * A chat is a timeline, so it renders by timestamp - not by block order.
 *
 * Block order is DAG merge order. A device that commits while its replica
 * is behind lists stale heads as parents, and its message merges in at a
 * position unrelated to when it was written: a phone's reply appeared
 * above answers that were hours older.
 */
import { expect, test } from "bun:test";
import { chatMessages, lastChatMessage } from "../src/lib/chat";
import type { BlockJSON, ObjectJSON } from "../src/lib/types";

function msg(id: string, author: string, ts: number, text: string, extra: Record<string, string> = {}): BlockJSON {
	return {
		id,
		childrenIds: [],
		content: { custom: { contentType: "chat", meta: { author, ts: String(ts), text, ...extra } } },
	} as unknown as BlockJSON;
}

function objectWith(order: BlockJSON[]): ObjectJSON {
	return {
		id: "task",
		typeKey: "task",
		fields: {},
		blocks: [
			{ id: "__discussion__", childrenIds: order.map((b) => b.id), content: {} } as unknown as BlockJSON,
			...order,
		],
	} as unknown as ObjectJSON;
}

const merged = objectWith([
	msg("m1", "human", 100, "first"),
	msg("m3", "human", 300, "from the phone"),
	msg("m2", "agent", 200, "answer to first"),
]);

test("messages render oldest-first by timestamp, whatever the merge order", () => {
	expect(chatMessages(merged).map((m) => m.text)).toEqual(["first", "answer to first", "from the phone"]);
});

test("the preview is the newest message by time, not the last block", () => {
	expect(lastChatMessage(merged)).toEqual({ text: "from the phone", author: "human", count: 3, last: 300 });
});

test("same-millisecond messages keep a stable block order", () => {
	const tied = objectWith([msg("a", "human", 100, "one"), msg("b", "agent", 100, "two")]);
	expect(chatMessages(tied).map((m) => m.text)).toEqual(["one", "two"]);
});

test("non-chat blocks under the discussion are not messages", () => {
	const withTooling = objectWith([msg("m1", "human", 100, "asked")]);
	withTooling.blocks.push({ id: "t1", childrenIds: [], content: { custom: { contentType: "tool_use", meta: {} } } } as unknown as BlockJSON);
	withTooling.blocks[0].childrenIds.push("t1");
	expect(chatMessages(withTooling).map((m) => m.text)).toEqual(["asked"]);
});

test("a scheduler post reports the recurring object as its origin", () => {
	const scheduled = objectWith([msg("m1", "scheduler", 100, "due", { origin: "schedule", origin_object: "task-7" })]);
	expect(chatMessages(scheduled)[0].origin).toBe("task-7");
});
