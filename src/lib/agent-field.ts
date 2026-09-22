/**
 * An object's `agent` field names the agent that answers it; empty means
 * the space's default agent. These type keys never have an agent of their
 * own: on some (skill, chat-less control types) the same key means
 * ownership, on the rest it makes no sense. No chip, no "assigned" row.
 */
export const AGENTLESS_TYPES: Record<string, true> = {
	agent: true,
	channel: true,
	relation: true,
	type: true,
	template: true,
	skill: true,
	descriptor: true,
	install: true,
	program: true,
	typescript: true,
	json: true,
	proto: true,
	pinned_fact: true,
	milestone: true,
	machine: true,
};
