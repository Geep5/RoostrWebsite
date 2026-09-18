/**
 * Per-object serving: which machine's harness does the work for an
 * object. The engine owns the rule (`core/serving.odin`, method
 * `serving`); this module gathers its inputs from the DAG the app already
 * holds (machines, the object's space) and turns the answer into copy.
 * Nothing here decides anything.
 */

import { fetchAllQuery, fetchObject, type QueryResultRow } from "$lib/api";
import { coreCall, initCore } from "$lib/engine/core";
import { objectSpaceId } from "$lib/relations";
import type { ObjectJSON } from "$lib/types";

export interface Serving {
	/** "" when no space default exists and nothing qualifies. */
	machineId: string;
	reason: "pinned" | "pinned-uncapable" | "space" | "space-capable" | "capability" | "unsatisfied";
	requires: string[];
	/** Machine ids whose capabilities cover `requires`, sorted. */
	candidates: string[];
}

export interface MachineRow {
	id: string;
	machineId: string;
	name: string;
	capabilities: string[];
}

// Capability names come from descriptor cards in the vault (`$lib/cards`),
// published by the harness from its catalogs. The list that used to live here
// was a hand-copy of two files in another package, and it drifted.
import { capabilityLabel } from "$lib/cards";
export { capabilityLabel };

/** Type keys whose objects are never served by a machine on their own row. */
export const UNSERVED_TYPES: Record<string, true> = { channel: true, machine: true, agent: true, relation: true, type: true, skill: true };

function strings(fields: ObjectJSON["fields"], key: string): string[] {
	return (fields[key]?.valuesValue?.items ?? []).map((v) => v.stringValue ?? "").filter(Boolean);
}

export function machineRowOf(row: QueryResultRow): MachineRow {
	return {
		id: row.id,
		machineId: row.fields["machine_id"]?.stringValue ?? "",
		name: row.fields["name"]?.stringValue ?? "",
		capabilities: strings(row.fields, "capabilities"),
	};
}

export async function fetchMachines(): Promise<{ rows: QueryResultRow[]; machines: MachineRow[] }> {
	const rows = await fetchAllQuery({ type: "machine" });
	return { rows, machines: rows.map(machineRowOf) };
}

/** Resolve one object against the live machine roster and its space. */
export async function resolveServing(object: ObjectJSON): Promise<{ serving: Serving; machines: MachineRow[] }> {
	const spaceId = objectSpaceId(object);
	const [{ rows, machines }, space] = await Promise.all([fetchMachines(), spaceId ? fetchObject(spaceId) : Promise.resolve(null), initCore()]);
	const serving = coreCall<Serving>("serving", { action: "resolve", object, space, machines: rows });
	return { serving, machines };
}

/**
 * Resolve many objects with one roster fetch. `spaces` are the channel
 * rows (oldest first: the first one is the default space that owns
 * unstamped objects). Returns one entry per input object, in order.
 */
export async function resolveMany(objects: QueryResultRow[], spaces: QueryResultRow[], machineRows: QueryResultRow[]): Promise<Serving[]> {
	await initCore();
	const byId = new Map(spaces.map((s) => [s.id, s]));
	return objects.map((object) => {
		const space = byId.get(object.fields["channel"]?.stringValue || "") ?? spaces[0] ?? null;
		return coreCall<Serving>("serving", { action: "resolve", object, space, machines: machineRows });
	});
}

/** Human name of a machine id: its object's `name`, else the id's first 8 chars. */
export function machineName(machines: MachineRow[], machineId: string): string {
	if (!machineId) return "no machine";
	return machines.find((m) => m.machineId === machineId)?.name || `${machineId.slice(0, 8)}…`;
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
		case "capability":
			return { text: `served by ${name} for ${keys(serving)}`, warning: false };
		case "unsatisfied":
			return { text: `no machine has ${keys(serving)}`, warning: true };
		default:
			return { text: `served by ${name}`, warning: false };
	}
}

function keys(serving: Serving): string {
	return serving.requires.map(capabilityLabel).join(", ");
}
