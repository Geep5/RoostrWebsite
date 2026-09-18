/**
 * An object's own conversations, as drawer rows.
 *
 * Its own module, with no store import, so it can be tested: `$lib/data.svelte`
 * declares `$state`, which does not exist outside the Svelte compiler. The
 * name lookup is a parameter for the same reason.
 *
 * The core decodes the thread metadata from the protobuf on each root block
 * and serves it as `conversations` on the object, so nothing here parses
 * anything - it projects.
 */

import { lastChatMessage } from "$lib/chat";
import type { AgentThread } from "$lib/conversations";
import type { ObjectJSON } from "$lib/types";

/**
 * Who wrote a message, for a one-line snippet.
 *
 * Three author shapes exist, and guessing between them is how a stranger's
 * message gets labelled "you": a human author is the 16 hex chars the daemon
 * derives from the key (`sha256(privkey)[:16]`, shared by every device that
 * holds it), an agent is a 36-char object uuid, and anything else is a foreign
 * writer we can only quote.
 */
export function authorLabel(author: string, nameOf: (id: string) => string): string {
	if (!author) return "you";
	if (/^[0-9a-f]{16}$/.test(author)) return "you";
	if (author.length === 36) return nameOf(author) || "agent";
	return author.length > 12 ? `${author.slice(0, 12)}…` : author;
}

/** Title for a thread that was opened without one. */
function fallbackTitle(kind: string): string {
	return kind === "agent_private" ? "Agent notes" : "Agents";
}

/**
 * The object's conversations past the human thread: the agent-to-agent and
 * private threads living in this object's block tree. The human thread has
 * its own pinned row in the drawer, so it is excluded here.
 */
export function objectThreads(object: ObjectJSON, nameOf: (id: string) => string = () => "agent"): AgentThread[] {
	const out: AgentThread[] = [];
	for (const c of object.conversations ?? []) {
		if (c.id === "__discussion__") continue;
		const last = lastChatMessage(object, c.id);
		out.push({
			id: c.id,
			title: c.title || fallbackTitle(c.kind),
			count: c.messageCount,
			// An empty thread sorts by when it was opened, not by epoch.
			last: last.last || c.createdAt,
			snippet: last.text,
			snippetWho: last.author ? authorLabel(last.author, nameOf) : "",
			inObject: true,
			kind: c.kind,
			participants: c.participants,
			closed: c.closed,
		});
	}
	return out.sort((a, b) => b.last - a.last);
}
