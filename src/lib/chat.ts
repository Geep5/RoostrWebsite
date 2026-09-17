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

import type { BlockJSON, ObjectJSON } from "$lib/types";

export interface ChatMessage {
	id: string;
	author: string;
	ts: number;
	text: string;
	replyTo: string;
	origin: string;
	reactions: Array<{ emoji: string; authors: string[] }>;
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

/** Chat blocks under `__discussion__`, in block (DAG merge) order. */
export function chatBlocks(object: ObjectJSON): Array<{ id: string; block: BlockJSON }> {
	const byId = new Map(object.blocks.map((b) => [b.id, b]));
	const root = byId.get("__discussion__");
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
export function chatMessages(object: ObjectJSON): ChatMessage[] {
	return chatBlocks(object)
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
export function lastChatMessage(object: ObjectJSON): { text: string; author: string; count: number; last: number } {
	const msgs = chatMessages(object);
	const newest = msgs[msgs.length - 1];
	return {
		text: (newest?.text ?? "").slice(0, 90),
		author: newest?.author ?? "",
		count: msgs.length,
		last: newest?.ts ?? 0,
	};
}
