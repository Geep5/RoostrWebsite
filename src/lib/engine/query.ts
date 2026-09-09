import type { ObjectJSON } from "$lib/types";
import type { QueryBody, QueryResultRowJSON } from "./contracts";
import { coreCall, coreGeneration } from "./core";

export type QueryRow = QueryResultRowJSON & { name: string; snippet?: string };

let generation = -1;
let signatures = new Map<string, string>();

/** The iterable API permits in-place object edits, so identity/updatedAt alone
 * cannot invalidate the cache. Signatures detect those edits without sending
 * unchanged objects across the WASM boundary or reparsing the vault in Odin. */
export function runQuery(
	states: Iterable<ObjectJSON>,
	body: QueryBody,
): { total: number; records: QueryRow[] } {
	const currentGeneration = coreGeneration();
	const reset = generation !== currentGeneration;
	const next = new Map<string, string>();
	const changed = new Map<string, ObjectJSON>();
	for (const state of states) {
		const signature = JSON.stringify(state);
		next.set(state.id, signature);
		if (reset || signatures.get(state.id) !== signature) changed.set(state.id, state);
		else changed.delete(state.id); // duplicate ids retain the last state
	}
	const removed: string[] = [];
	if (!reset) for (const id of signatures.keys()) if (!next.has(id)) removed.push(id);
	const result = coreCall<{ total: number; records: QueryRow[] }>("query", {
		upserts: [...changed.values()],
		removed,
		reset,
		body,
		nowMs: Date.now(),
	});
	// Failed dispatches leave the previous signature snapshot intact.
	signatures = next;
	generation = currentGeneration;
	return result;
}
