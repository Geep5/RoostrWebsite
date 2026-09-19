/**
 * Many conversations in one object, as the drawer sees them.
 *
 * The core decodes the thread metadata (`conversations` on the object), so
 * these are projection rules: which rows appear, in what order, and who is
 * credited for the last message.
 */

import { expect, test } from "bun:test";
import { authorLabel, objectAgentOptions, objectThreads } from "../src/lib/threads";
import { chatMessages, isLegacyExchange, mailboxEntries, replyRecipients } from "../src/lib/chat";
import type { AgentEndpoint, AgentMessage, MailboxEntry, ObjectJSON } from "../src/lib/types";

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
	expect(label(AGENT)).not.toBe("you");
	// The bug this pins: a non-uuid, non-human id used to render as "you".
	expect(label("agent-scout")).not.toBe("you");
	expect(label("some-very-long-foreign-author-id")).not.toBe("you");
});

const home: AgentEndpoint = { objectId: "obj", agentId: "" };
const homeAgent: AgentEndpoint = { objectId: "obj", agentId: AGENT };
const other: AgentEndpoint = { objectId: "other", agentId: "other-agent" };
const third: AgentEndpoint = { objectId: "third", agentId: "third-agent" };

function envelope(id: string, sentAt: number, extra: Partial<AgentMessage> = {}): MailboxEntry {
	return {
		message: {
			id, exchangeId: "mail", sender: home, recipients: [homeAgent, other], text: id,
			replyTo: "", sentAt, title: "Research group", requestReply: true, historical: false,
			operation: "", author: HUMAN, ...extra,
		},
		threadId: `__thread__${extra.exchangeId ?? "mail"}`,
		incoming: true, outgoing: true,
		deliveries: [
			{ recipient: homeAgent, status: "pending", error: "", at: 0 },
			{ recipient: other, status: "pending", error: "", at: 0 },
		],
		processing: { status: "pending", owner: "", error: "", at: 0 },
	};
}

test("mailbox copies project one exchange and one bubble per immutable message, without leaking into private chat", () => {
	const first = envelope("first", 100);
	const reply = envelope("reply", 200, { sender: other, recipients: [homeAgent], replyTo: "first", author: other.agentId });
	const mailed = { ...object, mailbox: [reply, first, reply] };
	expect(chatMessages(mailed, "__thread__mail").map((m) => m.id)).toEqual(["first", "reply"]);
	expect(chatMessages(mailed).map((m) => m.id)).toEqual(["m1"]);
	expect(chatMessages(mailed, "__thread__a").map((m) => m.id)).toEqual(["m2", "m3"]);
	const row = objectThreads(mailed).find((thread) => thread.id === "__thread__mail")!;
	expect({ count: row.count, snippet: row.snippet, author: row.snippetWho, members: row.endpoints }).toEqual({
		count: 2, snippet: "reply", author: other.agentId, members: [homeAgent, other],
	});
	expect(row.legacy).toBe(false);
});

test("offline replies follow their parents while independent messages and timestamp ties converge across replicas", () => {
	const parent = envelope("parent", 300);
	const reply = envelope("reply", 10, { replyTo: "parent" });
	const independent = envelope("independent", 200);
	const tieB = envelope("tie-b", 400);
	const tieA = envelope("tie-a", 400);
	const detachedPrivate = envelope("private", 500, { exchangeId: "private", replyTo: "unavailable-parent" });
	for (const entries of [
		[reply, tieB, independent, detachedPrivate, tieA, parent],
		[parent, tieA, independent, reply, tieB, detachedPrivate, reply],
	]) {
		const mailed = { ...object, mailbox: entries };
		expect(chatMessages(mailed, "__thread__mail").map((m) => m.id)).toEqual(["independent", "parent", "reply", "tie-a", "tie-b"]);
		expect(chatMessages(mailed, "__thread__private").map((m) => m.id)).toEqual(["private"]);
	}
});

test("duplicate delivery cannot regress completed receipts or replay processed work", () => {
	const pending = envelope("message", 100);
	const completed: MailboxEntry = {
		...pending,
		deliveries: [{ recipient: other, status: "delivered", error: "", at: 200 }],
		processing: { status: "processed", owner: "machine", error: "", at: 200 },
	};
	const lateFailure: MailboxEntry = {
		...pending,
		deliveries: [{ recipient: other, status: "failed", error: "offline", at: 300 }],
		processing: { status: "failed", owner: "old-machine", error: "interrupted", at: 300 },
	};
	for (const entries of [[pending, completed, lateFailure], [lateFailure, pending, completed]]) {
		const mailed = { ...object, mailbox: entries };
		const projected = mailboxEntries(mailed)[0];
		expect(projected.deliveries.find((receipt) => receipt.recipient.objectId === "other")?.status).toBe("delivered");
		expect(projected.processing.status).toBe("processed");
		expect(objectThreads(mailed).find((thread) => thread.id === "__thread__mail")?.problems).toBe(0);
	}
	expect(pending.deliveries[1].status).toBe("pending");
	expect(lateFailure.processing.status).toBe("failed");
});

test("failed delivery and processing remain visible until a newer explicit retry resets them", () => {
	const failed: MailboxEntry = {
		...envelope("message", 100),
		deliveries: [{ recipient: other, status: "failed", error: "recipient unavailable", at: 200 }],
		processing: { status: "failed", owner: "machine", error: "interrupted", at: 200 },
	};
	const reset: MailboxEntry = {
		...failed,
		deliveries: [{ recipient: other, status: "pending", error: "", at: 300 }],
		processing: { status: "pending", owner: "", error: "", at: 300 },
	};
	expect(objectThreads({ ...object, mailbox: [failed] }).find((thread) => thread.id === "__thread__mail")?.problems).toBe(2);
	for (const entries of [[failed, reset], [reset, failed]]) {
		const projected = mailboxEntries({ ...object, mailbox: entries })[0];
		expect(projected.deliveries[0].status).toBe("pending");
		expect(projected.processing.status).toBe("pending");
		expect(projected.processing.error).toBe("");
	}
});

test("reply-all keeps the human sender's own agent but excludes the responding agent's object", () => {
	const message = envelope("message", 100, { recipients: [homeAgent, other, third] }).message;
	expect(replyRecipients(message, home)).toEqual([homeAgent, other, third]);
	expect(replyRecipients(message, other)).toEqual([homeAgent, third]);
	const humanOnly = { ...message, recipients: [other, third] };
	expect(replyRecipients(humanOnly, other)).toEqual([home, third]);
	expect(replyRecipients(humanOnly, home)).toEqual([other, third]);
	expect(message.recipients).toEqual([homeAgent, other, third]);
});

test("existing recipient agents resolve to bound objects, space objects, or their own object without crossing spaces", () => {
	const record = (id: string, values: Record<string, string>) => ({
		id, fields: Object.fromEntries(Object.entries(values).map(([key, value]) => [key, { stringValue: value }])),
	});
	const records = [
		record("agent-b", { channel: "space", bound_object: "obj", name: "Duplicate" }),
		record("agent-a", { channel: "space", bound_object: "obj", name: "Bound" }),
		record("space-agent", { channel: "space", space_default: "space", name: "Space" }),
		record("unbound", { channel: "space", name: "Unbound" }),
		record("foreign", { channel: "elsewhere", bound_object: "foreign-object" }),
	];
	const options = objectAgentOptions(records, "space", (id) => id);
	expect(options.map((option) => option.endpoint)).toEqual([
		{ objectId: "obj", agentId: "agent-a" },
		{ objectId: "space", agentId: "space-agent" },
		{ objectId: "unbound", agentId: "unbound" },
	]);
	expect(objectAgentOptions(records.toReversed(), "space", (id) => id)).toEqual(options);
});

test("shared A2A history is read-only until its own mailbox copy exists; human and private threads stay writable", () => {
	expect(isLegacyExchange(object, "__thread__a")).toBe(true);
	expect(isLegacyExchange(object, "__discussion__")).toBe(false);
	expect(isLegacyExchange(object, "__thread__b")).toBe(false);
	expect(isLegacyExchange({ ...object, mailbox: [envelope("imported", 100, { exchangeId: "a", historical: true })] }, "__thread__a")).toBe(false);
	expect(isLegacyExchange({ ...object, fields: { a2a_pair: { stringValue: "old-pair" } }, conversations: [] })).toBe(true);
});
