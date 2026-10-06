import type { ValueJSON } from "$lib/types";

/**
 * An object's `agent` field is its guest list: the agents that may be
 * addressed here with @. Nothing answers without being on it — a space's
 * default roster is the space object's own guest list. These type keys
 * never have an agent of their own: on some (skill, chat-less control
 * types) the same key means ownership, on the rest it makes no sense.
 * No chip, no "assigned" row.
 */
export const AGENTLESS_TYPES: Record<string, true> = {
	agent: true,
	channel: true,
	relation: true,
	type: true,
	skill: true,
	tool: true,
	credential: true,
	program: true,
	typescript: true,
	json: true,
	proto: true,
	pinned_fact: true,
	milestone: true,
	machine: true,
	// A Judge asks Jev on its Repeat; no agent runs it.
	judge: true,
};


/** The one wire shape new `agent` writes use: a valuesValue list of linkValue items. */
export function agentLinksValue(ids: string[]): ValueJSON {
	return { valuesValue: { items: ids.map((id) => ({ linkValue: { targetId: id, relationKey: "agent" } })) } };
}
