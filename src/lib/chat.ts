/**
 * The chat timeline of an object: `chat` blocks under `__discussion__`.
 *
 * Ordered by the message's own timestamp, NOT by block order. Block order
 * is DAG merge order, and a device that commits while its replica is
 * behind lists stale heads as parents - its message then merges in at a
 * position that has nothing to do with when it was written. A phone that
 * posted after a long offline stretch showed up above hours-old replies.
 * Ties keep block order, so a same-millisecond pair stays stable.
 */

import type { AgentEndpoint, AgentMessage, BlockJSON, MailboxEntry, ObjectJSON } from "$lib/types";

export interface ChatMessage {
	id: string;
	author: string;
	ts: number;
	text: string;
	replyTo: string;
	origin: string;
	reactions: Array<{ emoji: string; authors: string[] }>;
	mailbox?: MailboxEntry;
}

function parseReactions(raw: string): ChatMessage["reactions"] {
	const out: ChatMessage["reactions"] = [];
	for (const chunk of raw.split(";")) {
		const bar = chunk.indexOf("|");
		if (bar <= 0) continue;
		const authors = chunk.slice(bar + 1).split(",").filter(Boolean);
		if (authors.length) out.push({ emoji: chunk.slice(0, bar), authors });
	}
	return out;
}

/** One address per object; prefer its agent over its human-only inbox. */
export function uniqueEndpoints(endpoints: AgentEndpoint[]): AgentEndpoint[] {
	const byObject = new Map<string, AgentEndpoint>();
	for (const endpoint of endpoints) {
		const prior = byObject.get(endpoint.objectId);
		if (!prior || (!prior.agentId && endpoint.agentId)) byObject.set(endpoint.objectId, endpoint);
	}
	return [...byObject.values()];
}

/** Human replies may address their own object's agent, but never themselves. */
export function replyRecipients(message: AgentMessage, responder: AgentEndpoint): AgentEndpoint[] {
	return uniqueEndpoints([message.sender, ...message.recipients]).filter((endpoint) =>
		endpoint.objectId !== responder.objectId || (!responder.agentId && !!endpoint.agentId),
	);
}

const deliveryPriority: Record<string, number> = { pending: 0, failed: 1, delivered: 2 };
const processingPriority: Record<string, number> = { pending: 0, awaiting_approval: 1, processing: 2, failed: 3, processed: 4 };

/** Duplicate delivery must neither duplicate a bubble nor hide a newer receipt. */
export function mailboxEntries(object: ObjectJSON, rootId?: string): MailboxEntry[] {
	const byId = new Map<string, MailboxEntry>();
	for (const entry of object.mailbox ?? []) {
		if (rootId && entry.threadId !== rootId) continue;
		const prior = byId.get(entry.message.id);
		if (!prior) {
			byId.set(entry.message.id, entry);
			continue;
		}
		const deliveries = new Map(prior.deliveries.map((d) => [d.recipient.objectId, d]));
		for (const delivery of entry.deliveries) {
			const held = deliveries.get(delivery.recipient.objectId);
			if (!held || (held.status !== "delivered" && (delivery.status === "delivered" || delivery.at > held.at
				|| (delivery.at === held.at && (deliveryPriority[delivery.status] > deliveryPriority[held.status]
					|| (delivery.status === held.status && delivery.error > held.error)))))) {
				deliveries.set(delivery.recipient.objectId, delivery);
			}
		}
		const processing = prior.processing.status === "processed" ? prior.processing
			: entry.processing.status === "processed" || entry.processing.at > prior.processing.at
				|| (entry.processing.at === prior.processing.at && (processingPriority[entry.processing.status] > processingPriority[prior.processing.status]
					|| (entry.processing.status === prior.processing.status && entry.processing.error > prior.processing.error)))
				? entry.processing : prior.processing;
		byId.set(entry.message.id, {
			...prior,
			incoming: prior.incoming || entry.incoming,
			outgoing: prior.outgoing || entry.outgoing,
			deliveries: [...deliveries.values()],
			processing,
		});
	}
	const compare = (a: MailboxEntry, b: MailboxEntry) => a.message.sentAt - b.message.sentAt
		|| (a.message.id < b.message.id ? -1 : a.message.id > b.message.id ? 1 : 0);
	const sorted = [...byId.values()].sort(compare);
	const children = new Map<string, MailboxEntry[]>();
	const ready: MailboxEntry[] = [];
	for (const entry of sorted) {
		const parent = entry.message.replyTo;
		if (!byId.has(parent)) ready.push(entry);
		else {
			const siblings = children.get(parent);
			if (siblings) siblings.push(entry);
			else children.set(parent, [entry]);
		}
	}
	ready.reverse();
	const emitted = new Set<string>();
	const ordered: MailboxEntry[] = [];
	// A parent must render before its reply, even with an offline clock.
	// Among available messages, use sentAt/id, never replica block order.
	while (ready.length) {
		const entry = ready.pop()!;
		emitted.add(entry.message.id);
		ordered.push(entry);
		for (const child of children.get(entry.message.id) ?? []) {
			let low = 0;
			let high = ready.length;
			while (low < high) {
				const mid = (low + high) >>> 1;
				if (compare(ready[mid], child) > 0) low = mid + 1;
				else high = mid;
			}
			ready.splice(low, 0, child);
		}
	}
	// Malformed cycles cannot satisfy causality, but must not hide history.
	for (const entry of sorted) if (!emitted.has(entry.message.id)) ordered.push(entry);
	return ordered;
}


/** Legacy shared roots are history only until the mailbox migration completes. */
export function isLegacyExchange(object: ObjectJSON, rootId = "__discussion__"): boolean {
	const isExchange = object.conversations?.some((c) => c.id === rootId && c.kind === "a2a")
		|| (rootId === "__discussion__" && !!object.fields["a2a_pair"]?.stringValue);
	return !!isExchange && !object.mailbox?.some((entry) => entry.threadId === rootId);
}

/**
 * Chat blocks under one conversation root, in block (DAG merge) order.
 *
 * An object holds many conversations - the human thread plus the agents' -
 * so the root is a parameter. `__discussion__` is the human thread, which is
 * what every caller that does not care about threads wants.
 */
export function chatBlocks(object: ObjectJSON, rootId = "__discussion__"): Array<{ id: string; block: BlockJSON }> {
	const byId = new Map(object.blocks.map((b) => [b.id, b]));
	const root = byId.get(rootId);
	if (!root) return [];
	const out: Array<{ id: string; block: BlockJSON }> = [];
	for (const cid of root.childrenIds) {
		const block = byId.get(cid);
		// The harness also stores tool_use/tool_result/compaction blocks
		// under __discussion__ - only chat messages belong in a timeline.
		if (block?.content.custom?.contentType === "chat") out.push({ id: cid, block });
	}
	return out;
}

/** The object's messages, oldest first by timestamp. */
export function chatMessages(object: ObjectJSON, rootId = "__discussion__"): ChatMessage[] {
	const entries = mailboxEntries(object, rootId);
	if (entries.length) {
		const byId = new Map(object.blocks.map((block) => [block.id, block]));
		return entries.map((entry) => ({
			id: entry.message.id,
			author: entry.message.author || entry.message.sender.agentId,
			ts: entry.message.sentAt,
			text: entry.message.text,
			replyTo: entry.message.replyTo,
			origin: entry.message.sender.objectId === object.id ? "" : entry.message.sender.objectId,
			reactions: parseReactions(byId.get(entry.message.id)?.content.custom?.meta?.["reactions"] ?? ""),
			mailbox: entry,
		}));
	}
	return chatBlocks(object, rootId)
		.map(({ id, block }, index) => {
			const meta = block.content.custom?.meta ?? {};
			return {
				index,
				message: {
					id,
					author: meta["author"] ?? "",
					ts: Number(meta["ts"] ?? 0),
					text: meta["text"] ?? "",
					replyTo: meta["replyTo"] ?? "",
					// The harness scheduler posts with origin "schedule" and
					// the recurring object under origin_object.
					origin: (meta["origin"] === "schedule" ? meta["origin_object"] : meta["origin"]) ?? "",
					reactions: parseReactions(meta["reactions"] ?? ""),
				},
			};
		})
		.sort((a, b) => a.message.ts - b.message.ts || a.index - b.index)
		.map((row) => row.message);
}

/** Newest message plus counts - the drawer/inbox preview. */
export function lastChatMessage(
	object: ObjectJSON,
	rootId = "__discussion__",
): { text: string; author: string; count: number; last: number } {
	const msgs = chatMessages(object, rootId);
	const newest = msgs[msgs.length - 1];
	return {
		text: (newest?.text ?? "").slice(0, 90),
		author: newest?.author ?? "",
		count: msgs.length,
		last: newest?.ts ?? 0,
	};
}
