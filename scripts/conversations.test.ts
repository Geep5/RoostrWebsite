/**
 * Many conversations in one object, as the drawer sees them.
 *
 * The core decodes the thread metadata (`conversations` on the object), so
 * these are projection rules: which rows appear, in what order, and who is
 * credited for the last message.
 */

import { expect, test } from "bun:test";
import { authorLabel, objectThreads } from "../src/lib/threads";
import { chatMessages } from "../src/lib/chat";
import type { ObjectJSON } from "../src/lib/types";

/** No store in a plain test: the name lookup is a parameter. */
const label = (author: string) => authorLabel(author, () => "");

const HUMAN = "816854963a03323d"; // 16 hex: the key-derived human id
const AGENT = "9b08be05-ed4b-4417-a976-1efead0cb561"; // 36-char object uuid

const message = (id: string, author: string, text: string, ts: number) => ({
	id,
	childrenIds: [],
	content: { custom: { contentType: "chat", meta: { author, text, ts: String(ts) } } },
});

const object: ObjectJSON = {
	id: "obj",
	typeKey: "note",
	fields: {},
	blocks: [
		{ id: "__discussion__", childrenIds: ["m1"], content: { custom: { contentType: "discussion", meta: {} } } },
		message("m1", HUMAN, "what should we charge?", 1000),
		{ id: "__thread__a", childrenIds: ["m2", "m3"], content: { custom: { contentType: "discussion", meta: {} } } },
		message("m2", AGENT, "pulled the sheet", 2000),
		message("m3", AGENT, "median is $29", 3000),
		{ id: "__thread__b", childrenIds: [], content: { custom: { contentType: "discussion", meta: {} } } },
	] as unknown as ObjectJSON["blocks"],
	conversations: [
		{ id: "__discussion__", kind: "human", title: "", participants: [], createdAt: 0, openedBy: "", aboutMessageId: "", closed: false, messageCount: 1 },
		{ id: "__thread__a", kind: "a2a", title: "Pricing research", participants: [AGENT], createdAt: 1500, openedBy: HUMAN, aboutMessageId: "m1", closed: false, messageCount: 2 },
		{ id: "__thread__b", kind: "agent_private", title: "", participants: [AGENT], createdAt: 500, openedBy: AGENT, aboutMessageId: "", closed: true, messageCount: 0 },
	],
	deleted: false,
	createdAt: 0,
	updatedAt: 3000,
};

test("each thread's timeline holds only its own messages", () => {
	expect(chatMessages(object).map((m) => m.text)).toEqual(["what should we charge?"]);
	expect(chatMessages(object, "__thread__a").map((m) => m.text)).toEqual(["pulled the sheet", "median is $29"]);
	expect(chatMessages(object, "__thread__b")).toEqual([]);
});

test("the drawer lists the object's own threads, newest first, without the human row", () => {
	const rows = objectThreads(object);
	// The human thread has its own pinned row in the drawer, so it is not here.
	expect(rows.map((r) => r.id)).toEqual(["__thread__a", "__thread__b"]);
	expect(rows[0].title).toBe("Pricing research");
	expect(rows[0].count).toBe(2);
	expect(rows[0].snippet).toBe("median is $29");
	expect(rows[0].inObject).toBe(true);
	// An untitled thread is named by what it is, not left blank.
	expect(rows[1].title).toBe("Agent notes");
	expect(rows[1].closed).toBe(true);
	// An empty thread sorts by when it was opened, not epoch.
	expect(rows[1].last).toBe(500);
});

test("an object with no conversations yields no rows", () => {
	expect(objectThreads({ ...object, conversations: undefined, blocks: [] })).toEqual([]);
});

test("a foreign author is never credited as you", () => {
	expect(label(HUMAN)).toBe("you");
	expect(label("")).toBe("you");
	expect(label(AGENT)).toBe("agent"); // no summary loaded in a plain test
	// The bug this pins: a non-uuid, non-human id used to render as "you".
	expect(label("agent-scout")).toBe("agent-scout");
	expect(label("some-very-long-foreign-author-id")).toBe("some-very-lo…");
});
