/**
 * Agent prompts - the shared half of putting an agent on a computer.
 *
 * `/setup` (the stepper) and the agent-creation paths read the same DAG
 * (system_prompt objects, descriptor cards, the paired harness) and write
 * the same field contract, so the contract lives once, here: an `agent`
 * object carries a `prompt` link to a system_prompt object in its space
 * plus `served_by`. The prompt's own configuration (system, model,
 * requires, skills) stays on the prompt object - editing it edits every
 * agent linked to it - and per-agent fields (`system`, `model`,
 * `responsible_types`, non-secret setup values) override the prompt,
 * never the other way round. Secret values never touch the agent.
 */

import { fetchAllQuery, fetchChannels, fetchObject, note, type QueryResultRow } from "$lib/api";
import { isLocalBackend } from "$lib/client-backend";
import { harnessFetch, pairedSession } from "$lib/local-transport";
import { cardOf, type Card, type CardField, type DescribedObject } from "$lib/card-shape";
import type { ValueJSON } from "$lib/types";

/** A system_prompt object as the setup flows need it. */
export interface PromptView {
	/** Object id - the target of an agent's `prompt` link. */
	id: string;
	name: string;
	description: string;
	model: string;
	/** Capability object ids the prompt's `requires` links point at (a legacy string item reads as a capability key). */
	requires: string[];
	/** Skill object ids the prompt's `skills` links point at (a legacy string item reads as a skill name). */
	skills: string[];
	/** Setup FieldSpec, matched from the descriptor card of the same name; empty for ad-hoc prompts. */
	fields: CardField[];
	/** Space (channel id) the prompt belongs to; "" when unstamped. */
	channel: string;
}

/**
 * The assistant seed, mirrored from glonOdin/harness/src/prompts.ts
 * (DEFAULT_PROMPT: DEFAULT_SYSTEM/DEFAULT_MODEL). The website only ever
 * CREATES the assistant prompt - every other prompt object is the
 * harness's to seed - and only when the space has none to reuse.
 */
export const ASSISTANT_PROMPT = {
	name: "Assistant",
	description: "A general Roostr agent: answers its chat and object discussions, reads and organizes the space.",
	system: `You are a helpful agent living inside Roostr, a local-first notes app where
everything is an object in a content-addressed DAG. You converse with your
principal through your chat and through any object's discussion — messages
from other objects arrive framed with their origin and the object's contents.
ALWAYS answer in plain text: your final reply is posted to the surface the
question came from automatically (never use chat_reply_on for that; it is
only for unprompted messages on OTHER objects). Use tools to read, search,
create, and organize objects; use memory_* tools to pin durable facts and
milestones. Be concise and concrete. When a listed skill matches the task,
read it with skill_read before starting.`,
	model: "claude-sonnet-4-5",
};

/** Link targets of a field, in shape order; a legacy string item passes through. */
function linkTokens(v: ValueJSON | undefined): string[] {
	const items = v?.valuesValue?.items ?? (v?.linkValue || v?.stringValue ? [v] : []);
	return items.map((i) => i.linkValue?.targetId ?? i.stringValue ?? "").filter(Boolean);
}

const promptNameOf = (row: QueryResultRow): string => row.fields["name"]?.stringValue || row.name || "";

function promptViewOf(row: QueryResultRow, cards: Card[]): PromptView {
	const name = promptNameOf(row);
	// The FieldSpec is not on the prompt object (it is harness-side seed
	// data); the descriptor card of the same name carries a copy the setup
	// form renders from.
	const card = cards.find((c) => c.kind === "agent" && (c.key.toLowerCase() === name.toLowerCase() || c.name === name));
	return {
		id: row.id,
		name,
		description: row.fields["description"]?.stringValue ?? "",
		model: row.fields["model"]?.stringValue ?? "",
		requires: linkTokens(row.fields["requires"]),
		skills: linkTokens(row.fields["skills"]),
		fields: card?.fields ?? [],
		channel: row.fields["channel"]?.stringValue ?? "",
	};
}

/** Every published card (for capability labels and FieldSpec) and every system_prompt object, both by name. */
export async function loadPrompts(): Promise<{ cards: Card[]; prompts: PromptView[] }> {
	const [descriptors, rows] = await Promise.all([fetchAllQuery({ type: "descriptor" }), fetchAllQuery({ type: "system_prompt" })]);
	const objects = await Promise.all(descriptors.map((r) => fetchObject(r.id) as Promise<DescribedObject>));
	const cards: Card[] = [];
	for (const o of objects) {
		const card = cardOf(o);
		if (card) cards.push(card);
	}
	cards.sort((a, b) => a.name.localeCompare(b.name));
	const prompts = rows.map((r) => promptViewOf(r, cards)).sort((a, b) => a.name.localeCompare(b.name));
	return { cards, prompts };
}

/**
 * The system_prompt object named `promptName` in `channelId`, created from
 * the assistant seed when absent - the website half of the harness's
 * ensureSystemPrompt (identity: prompt name × space; a channel-less caller
 * seeds into the vault's oldest channel, the engine's own creation
 * fallback, resolved here so reuse matches). Only the assistant seed
 * travels with the website, so a miss on any other name returns "".
 */
export async function ensurePrompt(promptName: string, channelId: string): Promise<string> {
	let channel = channelId;
	if (!channel) {
		const channels = await fetchChannels();
		channel = channels.sort((a, b) => a.createdAt - b.createdAt)[0]?.id ?? "";
	}
	const existing = (await fetchAllQuery({ type: "system_prompt" })).find(
		(r) => promptNameOf(r) === promptName && (r.fields["channel"]?.stringValue ?? "") === channel,
	);
	if (existing) return existing.id;
	if (promptName !== ASSISTANT_PROMPT.name) return "";
	const fields: Record<string, ValueJSON> = {
		system: sv(ASSISTANT_PROMPT.system),
		model: sv(ASSISTANT_PROMPT.model),
		description: sv(ASSISTANT_PROMPT.description),
	};
	if (channel) fields.channel = sv(channel);
	const { id } = await note.create(promptName, "system_prompt", fields);
	return id;
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

/**
 * The fields that make an `agent` object run `prompt` on `machineId`: the
 * `prompt` link and the server pin. The prompt's own configuration stays
 * on the prompt object - never `kind`, never a copy of the prompt's fields
 * - and non-secret FieldSpec values for this agent are the caller's to
 * write on top.
 */
export function agentCreateFields(promptName: string, machineId: string, prompt: Pick<PromptView, "id">): Record<string, ValueJSON> {
	return {
		prompt: { linkValue: { targetId: prompt.id, relationKey: "prompt" } },
		served_by: sv(machineId),
	};
}
