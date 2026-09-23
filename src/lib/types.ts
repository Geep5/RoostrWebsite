/**
 * Wire types shared between the app server (which reads the glon DAG)
 * and the client. Mirrors of glon's proto shapes as plain JSON.
 */

export interface ValueJSON {
	stringValue?: string;
	intValue?: number;
	floatValue?: number;
	boolValue?: boolean;
	listValue?: { values: string[] };
	mapValue?: { entries: Record<string, ValueJSON> };
	valuesValue?: { items: ValueJSON[] };
	linkValue?: { targetId: string; relationKey: string };
}

export interface MarkJSON {
	from: number;
	to: number;
	type: number;
	param?: string;
}

export interface BlockJSON {
	id: string;
	childrenIds: string[];
	content: {
		text?: { text: string; style: number; marks?: MarkJSON[]; checked?: boolean; color?: string };
		custom?: { contentType: string; meta?: Record<string, string>; data?: string };
		layout?: { style: number };
		table?: Record<string, never>;
		tableColumn?: Record<string, never>;
		tableRow?: { isHeader?: boolean };
	};
	fields?: { entries: Record<string, ValueJSON> };
	align?: number;
	backgroundColor?: string;
}

/**
 * One conversation in an object, decoded by the core from the protobuf on its
 * root block (`core/conversation.odin`). No client decodes it itself.
 */
export interface ConversationJSON {
	id: string;
	/** "human" | "a2a" | "agent_private"; "" from a newer writer. */
	kind: string;
	title: string;
	participants: string[];
	createdAt: number;
	openedBy: string;
	aboutMessageId: string;
	closed: boolean;
	messageCount: number;
}

export interface AgentEndpoint {
	objectId: string;
	agentId: string;
}

export interface AgentMessage {
	id: string;
	exchangeId: string;
	sender: AgentEndpoint;
	recipients: AgentEndpoint[];
	text: string;
	replyTo: string;
	sentAt: number;
	title: string;
	requestReply: boolean;
	historical: boolean;
	operation: string;
	author: string;
	unknown?: string;
}

export interface MessageDelivery {
	recipient: AgentEndpoint;
	status: "pending" | "delivered" | "failed";
	error: string;
	at: number;
}

export interface MessageProcessing {
	status: "pending" | "awaiting_approval" | "processing" | "processed" | "failed";
	owner: string;
	error: string;
	at: number;
}

export interface MailboxEntry {
	message: AgentMessage;
	threadId: string;
	incoming: boolean;
	outgoing: boolean;
	deliveries: MessageDelivery[];
	processing: MessageProcessing;
}

export interface ObjectJSON {
	id: string;
	typeKey: string;
	fields: Record<string, ValueJSON>;
	blocks: BlockJSON[];
	/** Absent when the object has never held a conversation. */
	conversations?: ConversationJSON[];
	/** Envelopes and local receipts decoded by the core, never by the UI. */
	mailbox?: MailboxEntry[];
	deleted: boolean;
	createdAt: number;
	updatedAt: number;
}

export interface ObjectSummary {
	id: string;
	typeKey: string;
	name: string;
	updatedAt: number;
	/** Scoping space object id ("" = personal/unassigned). */
	channelId: string;
	/** Object's emoji icon ("" = none, render the type glyph). */
	icon: string;
	/** The bundled done relation - drives the task-layout checkbox in lists. */
	done?: boolean;
}

export interface SpaceJSON {
	id: string;
	name: string;
	icon: string;
	pinnedIds: string[];
	members: Array<{ npub: string; role: string }>;
	keyId: number;
	createdAt: number;
	/** Drag-reorder position for the rail. Absent = fall back to createdAt. */
	order?: number;
}

export interface RelationDefJSON {
	id: string;
	key: string;
	format: string;
	name: string;
	iconEmoji?: string;
	/** Owning space id; "" = bundled/system def, present in every space. */
	space?: string;
	hidden: boolean;
	readOnly: boolean;
	maxCount: number;
	/** Object-format restriction (Anytype relationFormatObjectTypes): type ids
	 * the picker limits candidates to. Empty = unrestricted. */
	objectTypes?: string[];
	/** Roostr extension (no Anytype equivalent): a query or collection id whose
	 * members are the only pickable objects. Takes precedence over objectTypes. */
	objectSource?: string;
	options: Array<{ id: string; text: string; color: string; orderId: string }>;
}

/**
 * The substrate's own object kinds - source files, programs, agent
 * internals, infrastructure. Anytype never surfaces its system objects in
 * sets; unsourced queries and pickers exclude these (a query can still
 * target one explicitly via Source).
 */
export const SYSTEM_TYPE_KEYS = [
	"typescript",
	"program",
	"json",
	"proto",
	"relation",
	"type",
	"template",
	"agent",
	"channel",
	"peer",
	"machine",
	"pinned_fact",
	"milestone",
	"skill",
	"set",
	"",
] as const;

/** glon.Position values. */
export const Pos = {
	NONE: 0,
	TOP: 1,
	BOTTOM: 2,
	LEFT: 3,
	RIGHT: 4,
	INNER: 5,
	REPLACE: 6,
	INNER_FIRST: 7,
} as const;

/** glon.TextStyle values. */
export const Style = {
	PARAGRAPH: 0,
	HEADER1: 1,
	HEADER2: 2,
	HEADER3: 3,
	QUOTE: 4,
	CODE: 5,
	BULLET: 6,
	NUMBERED: 7,
	CHECKBOX: 8,
	TITLE: 9,
	TOGGLE: 10,
	CALLOUT: 11,
	DESCRIPTION: 12,
} as const;

/** glon.LayoutStyle values. */
export const Layout = { ROW: 0, COLUMN: 1, DIV: 2, HEADER: 3, TABLE_ROWS: 4, TABLE_COLUMNS: 5 } as const;

/** glon.MarkType values. */
export const MarkT = {
	BOLD: 0,
	ITALIC: 1,
	STRIKETHROUGH: 2,
	UNDERLINE: 3,
	INLINE_CODE: 4,
	LINK: 5,
	TEXT_COLOR: 6,
	BACKGROUND_COLOR: 7,
	MENTION: 8,
	OBJECT_MARK: 9,
} as const;

export function fieldStr(fields: Record<string, ValueJSON>, key: string): string {
	const v = fields[key];
	return typeof v?.stringValue === "string" ? v.stringValue : "";
}

export type RepeatFreq = "day" | "week" | "month" | "year";

/** `repeat_set` rule params. `anchor_ms` (epoch ms on the day the cadence counts from) defaults to today. */
export interface RepeatRuleJSON {
	freq: RepeatFreq;
	interval: number;
	/** 0 = Sun … 6 = Sat. Weekly only. */
	weekdays: number[];
	/** Monthly only: same calendar day, or same "2nd Tuesday". */
	monthly: "date" | "weekday";
	/** Minutes after local midnight. */
	time: number;
	/** IANA zone the rule was written in (informational). */
	tz: string;
	anchor_ms?: number;
}

/** The engine's `repeat` field, decoded from its mapValue. */
export interface RepeatJSON extends Omit<RepeatRuleJSON, "anchor_ms"> {
	/** Local-day index (days since 1970-01-01 in the wall clock) the cadence counts from. */
	anchor: number;
	/** Epoch ms of the current occurrence. */
	next: number;
	fired_for?: number;
	fired_at?: number;
	fired_by?: string;
	last_done?: number;
	count?: number;
	last_run?: { at: number; machine: string; conversation: string; error?: string };
}

const REPEAT_FREQS: readonly string[] = ["day", "week", "month", "year"];

export function repeatOf(fields: Record<string, ValueJSON>): RepeatJSON | null {
	const e = fields["repeat"]?.mapValue?.entries;
	if (!e) return null;
	const int = (v: ValueJSON | undefined) => (typeof v?.intValue === "number" ? v.intValue : undefined);
	const str = (v: ValueJSON | undefined) => (typeof v?.stringValue === "string" ? v.stringValue : undefined);
	const freq = str(e["freq"]);
	const time = int(e["time"]);
	const anchor = int(e["anchor"]);
	const next = int(e["next"]);
	if (!freq || !REPEAT_FREQS.includes(freq) || time === undefined || anchor === undefined || next === undefined) return null;
	const run = e["last_run"]?.mapValue?.entries;
	const runAt = run && int(run["at"]);
	return {
		freq: freq as RepeatFreq,
		interval: int(e["interval"]) ?? 1,
		weekdays: (e["weekdays"]?.valuesValue?.items ?? []).map(int).filter((d): d is number => d !== undefined),
		monthly: str(e["monthly"]) === "weekday" ? "weekday" : "date",
		time,
		tz: str(e["tz"]) ?? "",
		anchor,
		next,
		fired_for: int(e["fired_for"]),
		fired_at: int(e["fired_at"]),
		fired_by: str(e["fired_by"]),
		last_done: int(e["last_done"]),
		count: int(e["count"]),
		last_run: run && runAt !== undefined ? { at: runAt, machine: str(run["machine"]) ?? "", conversation: str(run["conversation"]) ?? "", error: str(run["error"]) } : undefined,
	};
}
