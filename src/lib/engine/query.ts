import type { ObjectJSON } from "$lib/types";
import type { QueryBody, QueryResultRowJSON } from "./contracts";
import { coreCall, coreCallWithBlob, coreGeneration } from "./core";

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

/**
 * Load a whole vault into the core from raw change bytes.
 *
 * The host stores every change with its protobuf beside its JSON, so a cold
 * start hands the bytes over untouched: the core decodes, toposorts and
 * replays straight into its cache. What that replaces, measured on this
 * machine: 8.3 MB of JSON stringified host-side for 10k objects, parsed by
 * the ABI, then re-marshalled and re-parsed per object into the cache region -
 * three serialisations each, and a hard failure past 16 MiB.
 *
 * Each input history must contain all changes for one distinct object.
 * Frames are `[u32 little-endian length][change bytes]`, repeated. Self
 * describing, so the JSON request carries no length array.
 */
export interface CorpusLoad {
	/** Objects this load replayed, summed across batches. */
	objects: number;
	changes: number;
	bytes: number;
	/** Undecodable changes, skipped and counted rather than hidden. */
	skipped: number;
	/** Objects the core holds afterwards - the authoritative total. */
	cached: number;
}

export function loadCorpus(histories: Iterable<readonly Uint8Array[]>, reset = true): CorpusLoad {
	const out: CorpusLoad = { objects: 0, changes: 0, bytes: 0, skipped: 0, cached: 0 };
	let batch: Uint8Array[] = [];
	let size = 0;
	let objects = 0;
	let first = reset;
	const flush = () => {
		if (batch.length === 0) return;
		const part = pushCorpus(batch, first);
		first = false;
		out.objects += part.objects;
		out.changes += part.changes;
		out.bytes += part.bytes;
		out.skipped += part.skipped;
		out.cached = part.cached;
		batch = [];
		size = 0;
		objects = 0;
	};
	try {
		for (const history of histories) {
			if (history.length === 0) continue;
			const bytes = byteLengthOf(history);
			// Never replay a prefix as a complete object. The backend can fall
			// back to its computed JSON snapshot for an oversized history.
			if (bytes > CORPUS_BATCH_BYTES) throw new Error("Object history exceeds the corpus batch limit");
			if (objects >= CORPUS_BATCH_OBJECTS || size + bytes > CORPUS_BATCH_BYTES) flush();
			for (const change of history) batch.push(change);
			size += bytes;
			objects++;
		}
		flush();
		if (first) {
			// Empty reset loads must clear the previous corpus too.
			coreCall("query", { reset: true, upserts: [], removed: [], body: { limit: 1 }, nowMs: Date.now() });
		}
	} catch (error) {
		// Earlier batches may already have committed. A subsequent JSON
		// query must replace that partial corpus, not apply only a delta.
		generation = -1;
		throw error;
	}
	signatures.clear();
	generation = coreGeneration();
	return out;
}

/** Object and framed-byte bounds; histories are indivisible. */
const CORPUS_BATCH_OBJECTS = 4000;
const CORPUS_BATCH_BYTES = 24 * 1024 * 1024;

const byteLengthOf = (parts: readonly Uint8Array[]): number => parts.reduce((sum, p) => sum + 4 + p.byteLength, 0);

/** True when the core has no corpus for this generation yet. */
export const needsColdLoad = (): boolean => generation !== coreGeneration();

function pushCorpus(parts: Uint8Array[], reset: boolean): CorpusLoad {
	let total = 0;
	for (const part of parts) total += 4 + part.byteLength;
	if (total === 0) return { objects: 0, changes: 0, bytes: 0, skipped: 0, cached: 0 };
	const blob = new Uint8Array(total);
	const header = new DataView(blob.buffer);
	let offset = 0;
	for (const part of parts) {
		header.setUint32(offset, part.byteLength, true);
		offset += 4;
		blob.set(part, offset);
		offset += part.byteLength;
	}
	// The corpus IS the snapshot the signature diff would have built, so the
	// caller drops the stale signatures once the last batch lands.
	return coreCallWithBlob<CorpusLoad>("corpus", { action: "push", reset }, blob);
}
