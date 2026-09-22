/**
 * Agent kinds - the shared half of putting an agent on a computer.
 *
 * `/setup` (the stepper) and the agent object's setup panel read the same
 * DAG (descriptor cards, the paired harness) and write the same field
 * contract, so the contract lives once, here: an `agent` object carries
 * `kind`, `served_by`, `requires`, `responsible_types` and, when the card
 * names one, `model`. Secret values never touch the agent.
 */

import { fetchAllQuery, fetchObject } from "$lib/api";
import { isLocalBackend } from "$lib/client-backend";
import { harnessFetch, pairedSession } from "$lib/local-transport";
import { cardOf, type Card, type DescribedObject } from "$lib/card-shape";
import type { ValueJSON } from "$lib/types";

/** Contract 1: the decoded card's `agent` block, present only on kind === "agent" cards. */
export interface AgentSpec {
	system: string;
	model: string;
	requires: string[];
	skills: string[];
	responsibleTypes: string[];
}

export interface KindCard {
	card: Card;
	agent: AgentSpec;
}

/** A card older than Contract 1 has no `agent`; it is still a kind, with empty defaults. */
export function agentSpecOf(object: DescribedObject): AgentSpec {
	const raw = object.descriptor?.agent;
	return {
		system: raw?.system ?? "",
		model: raw?.model ?? "",
		requires: raw?.requires ?? [],
		skills: raw?.skills ?? [],
		responsibleTypes: raw?.responsibleTypes ?? [],
	};
}

/** Every published card (for capability labels) and the agent kinds among them, both by name. */
export async function loadKinds(): Promise<{ cards: Card[]; kinds: KindCard[] }> {
	const descriptors = await fetchAllQuery({ type: "descriptor" });
	const objects = await Promise.all(descriptors.map((r) => fetchObject(r.id) as Promise<DescribedObject>));
	const cards: Card[] = [];
	const kinds: KindCard[] = [];
	for (const o of objects) {
		const card = cardOf(o);
		if (!card) continue;
		cards.push(card);
		if (card.kind === "agent") kinds.push({ card, agent: agentSpecOf(o) });
	}
	cards.sort((a, b) => a.name.localeCompare(b.name));
	kinds.sort((a, b) => a.card.name.localeCompare(b.card.name));
	return { cards, kinds };
}

/** machine_id of the harness this tab is paired with; "" when hosted, unpaired, or unreachable. */
export async function localMachineId(): Promise<string> {
	if (!isLocalBackend || !pairedSession()) return "";
	try {
		const res = await harnessFetch("/machine");
		return res.ok ? ((await res.json()) as { id: string }).id : "";
	} catch {
		return "";
	}
}

/**
 * Claim an agent on the paired harness's roster now, the way SpaceAgents
 * does, so it answers before the next converge. `served_by` already stands
 * on the object, so a failure here is not an error.
 */
export async function adoptLocally(id: string): Promise<boolean> {
	try {
		const res = await harnessFetch("/agents/toggle", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id, enabled: true }) });
		return res.ok;
	} catch {
		return false;
	}
}

export const sv = (s: string): ValueJSON => ({ stringValue: s });
export const lv = (items: string[]): ValueJSON => ({ valuesValue: { items: items.map((s) => ({ stringValue: s })) } });

/** Contract 2: the fields that make an `agent` object one of `kind`, on `machineId`. */
export function agentCreateFields(kindKey: string, machineId: string, kind: KindCard): Record<string, ValueJSON> {
	const fields: Record<string, ValueJSON> = {
		kind: sv(kindKey),
		served_by: sv(machineId),
		requires: lv(kind.agent.requires),
		responsible_types: lv(kind.agent.responsibleTypes),
	};
	// A card without a model (older codec) leaves the harness default in force.
	if (kind.agent.model) fields.model = sv(kind.agent.model);
	return fields;
}
