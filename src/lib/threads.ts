/** Pure projections of conversations stored in this object's own DAG. */
import { isLegacyExchange, lastChatMessage, mailboxEntries, uniqueEndpoints } from "$lib/chat";
import type { AgentEndpoint, MailboxEntry, ObjectJSON, ValueJSON } from "$lib/types";

export interface AgentThread {
	id: string;
	title: string;
	count: number;
	last: number;
	snippet: string;
	snippetWho: string;
	kind: string;
	participants: string[];
	endpoints: AgentEndpoint[];
	closed: boolean;
	legacy: boolean;
	problems: number;
}

export interface ObjectAgentOption {
	endpoint: AgentEndpoint;
	name: string;
	agentName: string;
	icon: string;
}

export function authorLabel(author: string, nameOf: (id: string) => string): string {
	if (!author) return "you";
	if (/^[0-9a-f]{16}$/.test(author)) return "you";
	if (author.length === 36) return nameOf(author) || "agent";
	return author.length > 12 ? `${author.slice(0, 12)}…` : author;
}

/** Existing agents only; space agents receive mail on the space object. */
export function objectAgentOptions(
	records: Array<{ id: string; fields: Record<string, ValueJSON> }>,
	spaceId: string,
	nameOf: (id: string) => string,
): ObjectAgentOption[] {
	const byObject = new Map<string, ObjectAgentOption>();
	for (const record of [...records].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0)) {
		if ((record.fields["channel"]?.stringValue ?? "") !== spaceId) continue;
		const objectId = record.fields["bound_object"]?.stringValue || record.fields["space_default"]?.stringValue || record.id;
		if (byObject.has(objectId)) continue;
		const agentName = record.fields["name"]?.stringValue || "Agent";
		byObject.set(objectId, {
			endpoint: { objectId, agentId: record.id },
			name: nameOf(objectId) || agentName,
			agentName,
			icon: record.fields["iconEmoji"]?.stringValue ?? "",
		});
	}
	return [...byObject.values()].sort((a, b) => a.name.localeCompare(b.name) || a.endpoint.objectId.localeCompare(b.endpoint.objectId));
}

/** Mailbox messages, not receipt blocks or legacy counts, define exchange rows. */
export function objectThreads(object: ObjectJSON, nameOf: (id: string) => string = () => ""): AgentThread[] {
	const groups = new Map<string, MailboxEntry[]>();
	for (const entry of mailboxEntries(object)) {
		const entries = groups.get(entry.threadId);
		if (entries) entries.push(entry);
		else groups.set(entry.threadId, [entry]);
	}
	const conversations = new Map((object.conversations ?? []).map((c) => [c.id, c]));
	const ids = new Set([...conversations.keys(), ...groups.keys()]);
	const out: AgentThread[] = [];
	for (const id of ids) {
		if (id === "__discussion__") continue;
		const conversation = conversations.get(id);
		const entries = groups.get(id) ?? [];
		const newest = entries[entries.length - 1]?.message;
		const last = newest ? { text: newest.text.slice(0, 90), author: newest.author || newest.sender.agentId, last: newest.sentAt, count: entries.length }
			: lastChatMessage(object, id);
		const kind = entries.length ? "a2a" : conversation?.kind ?? "";
		out.push({
			id,
			title: conversation?.title || entries[0]?.message.title || (kind === "agent_private" ? "Agent notes" : "Agents"),
			count: entries.length || conversation?.messageCount || last.count,
			last: last.last || conversation?.createdAt || 0,
			snippet: last.text,
			snippetWho: last.author ? authorLabel(last.author, nameOf) : "",
			kind,
			participants: conversation?.participants ?? [],
			endpoints: uniqueEndpoints(entries.flatMap((entry) => [entry.message.sender, ...entry.message.recipients])),
			closed: conversation?.closed ?? false,
			legacy: isLegacyExchange(object, id),
			problems: entries.reduce((count, entry) => count + (entry.outgoing ? entry.deliveries.filter((d) => d.status === "failed").length : 0)
				+ (entry.incoming && ["failed", "awaiting_approval"].includes(entry.processing.status) ? 1 : 0), 0),
		});
	}
	return out.sort((a, b) => b.last - a.last || a.id.localeCompare(b.id));
}
