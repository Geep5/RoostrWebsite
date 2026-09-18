import type { ObjectJSON } from "$lib/types";
import type { QueryBody, QueryResultRowJSON } from "./contracts";
import { coreCall, coreGeneration } from "./core";

export type QueryRow = QueryResultRowJSON & { name: string; snippet?: string };

/**
 * The core keeps queried objects resident (`QUERY_CACHE_MAX_OBJECTS`), so a
 * query only has to carry what CHANGED since the last one. Two things used
 * to spoil that:
 *
 *   1. Every call re-serialized the whole vault to diff signatures, so a
 *      view render was O(corpus) even when nothing had changed - about
 *      26ms at 20k objects, and seconds of pure JSON churn beyond that.
 *   2. A cold start pushed every object in ONE request, which fails hard
 *      past the ABI's 16 MiB reservation: a big space could not be queried
 *      at all on first load.
 *
 * The host knows precisely which objects it replayed, so it now says so
 * (`delta`), and the first push is split into batches that each fit the
 * reservation. Callers with no change tracking can still pass nothing and
 * get the old signature diff.
 */

/** Batches stay well under the core's 16 MiB reservation, headroom included. */
const PUSH_BUDGET_BYTES = 6 * 1024 * 1024;

export interface QueryDelta {
	/** Ids whose state changed since the last query (replayed or committed). */
	upserted: Iterable<string>;
	/** Ids that no longer exist. */
	removed: Iterable<string>;
}

let generation = -1;
let signatures = new Map<string, string>();

function pushAndQuery(
	upserts: ObjectJSON[],
	removed: string[],
	reset: boolean,
	body: QueryBody,
): { total: number; records: QueryRow[] } {
	// One call when it fits - the common case, a handful of changed objects.
	const batches: ObjectJSON[][] = [];
	let batch: ObjectJSON[] = [];
	let size = 0;
	for (const object of upserts) {
		const bytes = JSON.stringify(object).length;
		if (batch.length > 0 && size + bytes > PUSH_BUDGET_BYTES) {
			batches.push(batch);
			batch = [];
			size = 0;
		}
		batch.push(object);
		size += bytes;
	}
	batches.push(batch);
	let result: { total: number; records: QueryRow[] } = { total: 0, records: [] };
	for (const [index, part] of batches.entries()) {
		result = coreCall<{ total: number; records: QueryRow[] }>("query", {
			upserts: part,
			// Removals and the reset belong to the first call only, or a later
			// batch would wipe the objects the earlier ones just loaded.
			removed: index === 0 ? removed : [],
			reset: index === 0 ? reset : false,
			body,
			nowMs: Date.now(),
		});
	}
	return result;
}

export function runQuery(
	states: Iterable<ObjectJSON>,
	body: QueryBody,
	delta?: QueryDelta,
): { total: number; records: QueryRow[] } {
	const currentGeneration = coreGeneration();
	const reset = generation !== currentGeneration;

	if (delta && !reset) {
		// Steady state: touch only what the host says moved.
		const byId = new Map<string, ObjectJSON>();
		for (const state of states) byId.set(state.id, state);
		const upserts: ObjectJSON[] = [];
		for (const id of delta.upserted) {
			const state = byId.get(id);
			if (state) upserts.push(state);
		}
		const removed: string[] = [];
		for (const id of delta.removed) if (!byId.has(id)) removed.push(id);
		const result = pushAndQuery(upserts, removed, false, body);
		generation = currentGeneration;
		return result;
	}

	// Cold start, or a caller without change tracking: full snapshot, and the
	// signature map so later calls can diff.
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
	const result = pushAndQuery([...changed.values()], removed, reset, body);
	// Failed dispatches leave the previous signature snapshot intact.
	signatures = next;
	generation = currentGeneration;
	return result;
}
