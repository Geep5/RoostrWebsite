/** Names and recipient discovery for the local object mailbox. */
import { fetchAllQuery } from "$lib/api";
import { lastChatMessage } from "$lib/chat";
import { store } from "$lib/data.svelte";
import type { AgentEndpoint, ObjectJSON } from "$lib/types";
import { authorLabel, objectAgentOptions, objectThreads as pureObjectThreads, type AgentThread, type ObjectAgentOption } from "$lib/threads";

function nameOf(id: string): string {
	return store.summaries.find((s) => s.id === id)?.name
		|| store.channels.find((s) => s.id === id)?.name
		|| store.agents.find((s) => s.id === id)?.name || "";
}

export function whoName(author: string): string {
	return authorLabel(author, nameOf);
}

export function endpointName(endpoint: AgentEndpoint): string {
	const objectName = nameOf(endpoint.objectId) || endpoint.objectId.slice(0, 8);
	const agentName = endpoint.agentId ? nameOf(endpoint.agentId) : "";
	return agentName && agentName !== objectName ? `${objectName} · ${agentName}` : objectName;
}

export const lastMessage = lastChatMessage;
export const objectThreads = (object: ObjectJSON): AgentThread[] => pureObjectThreads(object, nameOf);

export async function loadObjectAgents(object: ObjectJSON): Promise<ObjectAgentOption[]> {
	const spaceId = object.typeKey === "channel" ? object.id : object.fields["channel"]?.stringValue ?? "";
	const agents = await fetchAllQuery({ type: "agent", filters: [{ key: "channel", condition: "equal", value: spaceId }] });
	return objectAgentOptions(agents, spaceId, (id) => id === object.id ? object.fields["name"]?.stringValue ?? "" : nameOf(id));
}

export function agoShort(ts: number): string {
	if (!ts) return "";
	const m = Math.round((Date.now() - ts) / 60000);
	if (m < 1) return "now";
	if (m < 60) return `${m}m`;
	if (m < 48 * 60) return `${Math.round(m / 60)}h`;
	return `${Math.round(m / 1440)}d`;
}
