/** Pure projections of conversations stored in this object's own DAG. */
import { isLegacyExchange, lastChatMessage, mailboxEntries, uniqueEndpoints } from "$lib/chat";
import { guestAgents, type AgentEndpoint, type MailboxEntry, type ObjectJSON, type ValueJSON } from "$lib/types";

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
 * Who may be @-addressed on `object`: the agents on its guest list (its
 * `agent` property), each as an endpoint on this object. A space object's
 * guest list is its own agents. `records` are the agent objects to name.
 */
export function objectAgentOptions(
	object: { id: string; typeKey: string; fields: Record<string, ValueJSON> },
	records: Array<{ id: string; fields: Record<string, ValueJSON> }>,
	nameOf: (id: string) => string,
): ObjectAgentOption[] {
	const agents = new Map(records.map((record) => [record.id, record]));
	const guests = object.typeKey === "channel"
		? records.filter((r) => (r.fields["channel"]?.stringValue ?? "") === object.id && !r.fields["spawn_parent"]?.stringValue).map((r) => r.id)
		: guestAgents(object.fields);
	const out: ObjectAgentOption[] = [];
	for (const agentId of new Set(guests)) {
		const agent = agents.get(agentId);
		if (!agent) continue;
		const agentName = agent.fields["name"]?.stringValue || "Agent";
		out.push({ endpoint: { objectId: object.id, agentId }, name: nameOf(object.id) || agentName, agentName, icon: agent.fields["iconEmoji"]?.stringValue ?? "" });
	}
	return out.sort((a, b) => a.agentName.localeCompare(b.agentName) || a.endpoint.agentId.localeCompare(b.endpoint.agentId));
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
