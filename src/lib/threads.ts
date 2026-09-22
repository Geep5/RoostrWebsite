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

/**
 * Existing agents only; space agents receive mail on the space object.
 * `records` mixes the space's agents with objects whose `agent` field
 * names one of them; an agent never carries `agent` itself, so that field
 * is what tells the two apart. Objects and agents outside `spaceId` are
 * dropped, as is an object pointing at an agent from another space.
 */
export function objectAgentOptions(
	records: Array<{ id: string; fields: Record<string, ValueJSON> }>,
	spaceId: string,
	nameOf: (id: string) => string,
): ObjectAgentOption[] {
	const inSpace = [...records]
		.filter((record) => (record.fields["channel"]?.stringValue ?? "") === spaceId)
		.sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
	const agents = new Map(inSpace.filter((record) => !record.fields["agent"]?.stringValue).map((record) => [record.id, record]));
	const byObject = new Map<string, ObjectAgentOption>();
	const add = (objectId: string, agent: { id: string; fields: Record<string, ValueJSON> }) => {
		if (byObject.has(objectId)) return;
		const agentName = agent.fields["name"]?.stringValue || "Agent";
		byObject.set(objectId, {
			endpoint: { objectId, agentId: agent.id },
			name: nameOf(objectId) || agentName,
			agentName,
			icon: agent.fields["iconEmoji"]?.stringValue ?? "",
		});
	};
	for (const record of inSpace) {
		const pointedAt = record.fields["agent"]?.stringValue;
		if (pointedAt) {
			const agent = agents.get(pointedAt);
			if (agent) add(record.id, agent);
		} else {
			add(record.fields["space_default"]?.stringValue || record.id, record);
		}
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
