import type { ObjectJSON } from "$lib/types";
import type { BaseRow, ChangeJSON, ObjectInputs, ReplayApi } from "./contracts";
import { coreCall } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";
import { bytesToBase64 } from "./proto";

/** Replay through the same Odin implementation used by the native engine. */
export function computeObject(changes: ChangeJSON[], checkpoint?: Uint8Array): ObjectJSON | null {
	return coreCall<ObjectJSON | null>("replay", checkpoint ? { changes, checkpoint: bytesToBase64(checkpoint) } : { changes });
}

export const replay: ReplayApi = { computeObject };

// ── State sync (docs/state-sync.md "Core API") ─────────────────────

/** What one object's state-sync request is built from; `pending` = ids this device never published. */
export type StateInputs = Pick<ObjectInputs, "changes" | "bases" | "checkpoint"> & { pending?: Iterable<string> };

export interface Classification {
	base: { hash: string; epoch: number } | null;
	live: string[];
	covered: string[];
	premigrationCovered: string[];
	orphan: string[];
	stale: string[];
	liveHeads: string[];
}

export interface Rebased {
	/** Hash of the current base every rebased change carries as `b`. */
	base: string;
	/** In publish order. */
	changes: Array<{ orphan: string; id: string; change: ChangeJSON; bytes: string }>;
}

/**
 * The shared request: bases as base64, changes in the ordered map encoding
 * (rebase hashes them, so every device must hand over the same op order),
 * `pending` narrowed to this object's changes.
 */
function stateRequest(action: string, input: StateInputs): unknown {
	const held = new Set(input.changes.map((change) => change.id));
	const pending = [...(input.pending ?? [])].filter((id) => held.has(id));
	return packCoreValueMaps({
		action,
		bases: input.bases.map(bytesToBase64),
		changes: input.changes,
		pending,
		checkpoint: input.checkpoint && input.bases.length === 0 ? bytesToBase64(input.checkpoint) : null,
	});
}

/** Current base state + live deltas (core `replay_from_base`); with no base, identical to `replay`. */
export function replayObject(input: StateInputs): ObjectJSON | null {
	if (input.changes.length === 0 && input.bases.length === 0 && !input.checkpoint) return null;
	return coreCall<ObjectJSON | null>("sync", stateRequest("replay_from_base", input));
}

export function classifyObject(input: StateInputs): Classification {
	return coreCall<Classification>("sync", stateRequest("classify", input));
}

/** Every orphan of one object rebased onto its current base (core `rebase`); needs a base. */
export function rebaseObject(input: StateInputs): Rebased {
	const out = coreCall<Rebased>("sync", stateRequest("rebase", input));
	return { base: out.base, changes: out.changes.map((item) => ({ ...item, change: unpackCoreValueMaps<ChangeJSON>(item.change) })) };
}

/** Index of the current base (core `base_current`: epoch, then created_at, then hash). */
export function currentBaseIndex(bases: Uint8Array[]): number {
	if (bases.length === 0) return -1;
	return coreCall<{ index: number } | null>("sync", { action: "base_current", bases: bases.map(bytesToBase64) })?.index ?? -1;
}

/** Hash of the current base among held rows of one object; the store's `putBase` picker. */
export function pickCurrentBase(rows: BaseRow[]): string {
	return rows[currentBaseIndex(rows.map((row) => row.bytes))].hash;
}

/**
 * DAG heads a new local change builds on. With a base: the live heads plus
 * the base heads nothing live built on (the core's heads of the live deltas
 * over the current base as a checkpoint); without one, the plain heads of
 * every held change and the legacy checkpoint.
 */
export function objectHeads(input: StateInputs): string[] {
	if (input.bases.length === 0) {
		const payload: Record<string, unknown> = { action: "heads", changes: input.changes };
		if (input.checkpoint) payload.checkpoint = bytesToBase64(input.checkpoint);
		return coreCall<string[]>("mutation", packCoreValueMaps(payload));
	}
	const live = new Set(classifyObject(input).live);
	return coreCall<string[]>("mutation", packCoreValueMaps({
		action: "heads",
		changes: input.changes.filter((change) => live.has(change.id)),
		checkpoint: bytesToBase64(input.bases[currentBaseIndex(input.bases)]),
	}));
}
