/**
 * Per-object serving: which machine's harness does the work for an
 * object. The engine owns the rule (`core/serving.odin`, method
 * `serving`); this module gathers its inputs from the DAG the app already
 * holds (machines, the agents an object names) and turns the answer into
 * copy. Nothing here decides anything.
 */

import { fetchAllQuery, type QueryResultRow } from "$lib/api";
import { coreCall, initCore } from "$lib/engine/core";
import { guestAgents, type ObjectJSON, type ValueJSON } from "$lib/types";

export interface Serving {
	/** "" when nothing pins the object and no machine qualifies. */
	machineId: string;
	reason: "self" | "pinned" | "pinned-uncapable" | "agent" | "agent-capable" | "capability" | "unsatisfied" | "unserved";
	/** Catalog keys of the machine skills the object's Skills need. */
	skills: string[];
	/** Machine ids that have every one of `skills` working, sorted. */
	candidates: string[];
}

export interface MachineRow {
	id: string;
	machineId: string;
	name: string;
}

/** Type keys whose objects are never served by a machine on their own row. */
export const UNSERVED_TYPES: Record<string, true> = { channel: true, machine: true, agent: true, relation: true, type: true, skill: true };

/** Catalog key -> Skill object name, refreshed by every resolve (see `skillStates`). */
let skillNames = new Map<string, string>();

/** A catalog key's display name: its Skill object's name, else the key itself. */
export function skillLabel(key: string): string {
	return skillNames.get(key) || key;
}

export function machineRowOf(row: QueryResultRow): MachineRow {
	return {
		id: row.id,
		machineId: row.fields["machine_id"]?.stringValue ?? "",
		name: row.fields["name"]?.stringValue ?? "",
	};
}

export async function fetchMachines(): Promise<{ rows: QueryResultRow[]; machines: MachineRow[] }> {
	const rows = await fetchAllQuery({ type: "machine" });
	return { rows, machines: rows.map(machineRowOf) };
}

/**
 * The agent objects any of `objects` names in its `agent` guest list: the
 * engine takes the first of them with a `served_by` as the object's pin.
 */
async function guestAgentRows(objects: Array<Pick<ObjectJSON, "fields">>): Promise<QueryResultRow[]> {
	const ids = [...new Set(objects.flatMap((o) => guestAgents(o.fields)))];
	return ids.length > 0 ? fetchAllQuery({ filters: [{ key: "id", condition: "in", value: ids }] }) : [];
}

/**
 * What the resolver needs beyond machines and agents: the skill objects
 * (their `key` says which are machine software), and the capability objects
 * (a machine has a skill once its capability for that key is active).
 * Without these every object with Skills resolves wrongly.
 */
async function skillStates(): Promise<{ skills: QueryResultRow[]; capabilities: QueryResultRow[] }> {
	const [skills, capabilities] = await Promise.all([fetchAllQuery({ type: "skill" }), fetchAllQuery({ type: "capability" })]);
	skillNames = new Map(skills.flatMap((s) => {
		const key = s.fields["key"]?.stringValue ?? "";
		const name = s.fields["name"]?.stringValue ?? "";
		return key && name ? [[key, name] as const] : [];
	}));
	return { skills, capabilities };
}

/** Resolve one object against the live machine roster and its agents. */
export async function resolveServing(object: ObjectJSON): Promise<{ serving: Serving; machines: MachineRow[] }> {
	const [{ rows, machines }, agents, states] = await Promise.all([fetchMachines(), guestAgentRows([object]), skillStates(), initCore()]);
	const serving = coreCall<Serving>("serving", { action: "resolve", object, agents, machines: rows, ...states });
	return { serving, machines };
}

/**
 * Resolve many objects with one roster fetch and one fetch of the agents
 * they name. Returns one entry per input object, in order.
 */
export async function resolveMany(objects: QueryResultRow[], machineRows: QueryResultRow[]): Promise<Serving[]> {
	const [agentRows, states] = await Promise.all([guestAgentRows(objects), skillStates(), initCore()]);
	const byId = new Map(agentRows.map((a) => [a.id, a]));
	return objects.map((object) => {
		const agents = guestAgents(object.fields).flatMap((id) => byId.get(id) ?? []);
		return coreCall<Serving>("serving", { action: "resolve", object, agents, machines: machineRows, ...states });
	});
}

/** Human name of a machine id: its object's `name`, else the id's first 8 chars. */
export function machineName(machines: MachineRow[], machineId: string): string {
	if (!machineId) return "no machine";
	return machines.find((m) => m.machineId === machineId)?.name || `${machineId.slice(0, 8)}…`;
}

/**
 * The machine id a `served_by` field names, however it was written: the
 * machine_id string pins use, or a link / one-item list pointing at the
 * machine's object (resolved to its machine_id through `machines`).
 */
export function servedByMachineId(fields: Record<string, ValueJSON>, machines: MachineRow[]): string {
	const v = fields["served_by"];
	const first = v?.valuesValue?.items?.[0];
	const raw = v?.stringValue || v?.linkValue?.targetId || first?.stringValue || first?.linkValue?.targetId || "";
	return machines.find((m) => m.id === raw)?.machineId ?? raw;
}

/** The sentence a chip or cell shows; `warning` when the resolution cannot be honoured. */
export function servingCopy(serving: Serving, machines: MachineRow[]): { text: string; warning: boolean } {
	if (!serving.machineId) return { text: serving.reason === "unsatisfied" ? `no machine has ${keys(serving)}` : "no machine yet", warning: serving.reason === "unsatisfied" };
	const name = machineName(machines, serving.machineId);
	switch (serving.reason) {
		case "pinned":
			return { text: `served by ${name} (pinned)`, warning: false };
		case "pinned-uncapable":
			return { text: `pinned to ${name}, which lacks ${keys(serving)}`, warning: true };
		case "agent-capable":
		case "capability":
			return { text: `served by ${name} for ${keys(serving)}`, warning: false };
		case "unsatisfied":
			return { text: `no machine has ${keys(serving)}`, warning: true };
		default:
			return { text: `served by ${name}`, warning: false };
	}
}

function keys(serving: Serving): string {
	return serving.skills.map(skillLabel).join(", ");
}
