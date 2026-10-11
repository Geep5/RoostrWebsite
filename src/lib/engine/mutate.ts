/** Platform orchestration for the shared Odin mutation planner. */
import type { ChangeJSON } from "./contracts";
import type { ObjectJSON } from "$lib/types";
import { spaceKeyAll, spaceKeyGet, spaceKeyRotate, spaceKeyEnsure, spaceKeyRemove, spaceOwner } from "./spacekeys";
import { coreCall, initCore } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";
import { needsColdLoad } from "./query";
import { objectHeads, type StateInputs } from "./replay";

export interface MutateCtx {
	/** This device's key-derived author id. */
	author: string;
	/** This identity's hex pubkey: space ownership is decided against it. */
	pk: string;
	/** Applying the space owner's kind-5 deletion signal: vanishing a space
	 * someone else owns is then this identity's part of the owner's delete,
	 * not a member deleting it for everyone. */
	ownerSignal: boolean;
	/** Everything the DAG holds for one object: stored changes, its bases, its legacy checkpoint, and this device's unpublished ids. */
	dagFor(objectId: string): Promise<StateInputs>;
	/** Bring the core's query cache up to date with every committed and
	 * imported change: the planner reads states from it, not the request. */
	syncCache(): Promise<void>;
	/** This replica's current states of `ids`; ids it does not hold are left out. */
	statesOf(ids: string[]): Promise<ObjectJSON[]>;
	/** Encode, address, durably persist, and publish. */
	commit(change: ChangeJSON): Promise<string>;
}

export interface MutationRequest {
	action: string;
	params: Record<string, unknown>;
	timestamp: number;
	author: string;
	id_seed: string;
	key_id: number;
}

export interface MutationPlan {
	changes: ChangeJSON[];
	result: Record<string, unknown>;
	vanish_ids: string[];
	vanish_changes: ChangeJSON[];
}

/**
 * Plan over the core's query cache. The request carries no states: an object
 * its params name that the cache lacks comes back `missing`, and the retry
 * carries this replica's states of those ids as overlay and names the rest
 * `absent`, so nothing is ever planned as if a held object did not exist.
 *
 * `syncCache` runs right before each planner call, with no await between:
 * a core trap in another caller's request in the gap empties the cache
 * (core.ts generation), and a plan over an empty cache would scan nothing.
 */
export async function planMutation(
	request: MutationRequest,
	syncCache: () => Promise<void>,
	statesOf: (ids: string[]) => Promise<ObjectJSON[]>,
): Promise<MutationPlan> {
	const call = (payload: Record<string, unknown>) =>
		unpackCoreValueMaps(coreCall<MutationPlan | { missing: string[] }>("mutation", packCoreValueMaps(payload))) as MutationPlan | { missing: string[] };
	do await syncCache(); while (needsColdLoad());
	const first = call({ ...request });
	if (!("missing" in first)) return first;
	const overlay = await statesOf(first.missing);
	const held = new Set(overlay.map((state) => state.id));
	do await syncCache(); while (needsColdLoad());
	const retried = call({ ...request, overlay, absent: first.missing.filter((id) => !held.has(id)) });
	if ("missing" in retried) throw new Error(`Mutation needs objects this replica cannot supply: ${retried.missing.join(", ")}`);
	return retried;
}

export async function runMutation(
	ctx: MutateCtx,
	action: string,
	params: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	await initCore();
	const channelId = typeof params.channel_id === "string" ? params.channel_id : "";
	// Mirror of the UI routing: an owner deletes a space for everyone, a
	// member only leaves it. Checked before planning, so nothing commits.
	if (action === "space_leave" && channelId && spaceOwner(spaceKeyGet(channelId), ctx.pk) === "") {
		throw new Error("You own this space: delete it for everyone instead of leaving it.");
	}
	if (action === "vanish" && !ctx.ownerSignal) {
		const keys = spaceKeyAll();
		const ids: unknown[] = [params.object_id, ...(Array.isArray(params.object_ids) ? params.object_ids : [])];
		if (ids.some((id) => typeof id === "string" && spaceOwner(keys[id], ctx.pk) !== "")) {
			throw new Error("Only the space's owner can delete it for everyone: leave it instead.");
		}
	}
	const rotating = action === "channel_member_remove" || action === "channel_key_rotate";
	const keyId = rotating ? (spaceKeyGet(channelId)?.keyId ?? 0) + 1 : 0;
	const plan = await planMutation({
		action,
		params,
		timestamp: Date.now(),
		author: ctx.author,
		id_seed: crypto.randomUUID(),
		key_id: keyId,
	}, ctx.syncCache, ctx.statesOf);

	// Key material stays in the host; invalid domain requests never rotate keys.
	if (rotating) spaceKeyRotate(channelId);
	async function commit(change: ChangeJSON): Promise<void> {
		change.parentIds = objectHeads(await ctx.dagFor(change.objectId));
		await ctx.commit(change);
	}
	for (const [index, change] of plan.changes.entries()) {
		await commit(change);
		if (action === "channel_create" && index === 0) spaceKeyEnsure(change.objectId);
	}
	// Leaving keeps nothing to sync the space with: the ledger hides it, and
	// the key goes from this device's keyring.
	if (action === "space_leave") spaceKeyRemove(channelId);
	// Browsers enforce the synced ledger locally; native hosts additionally purge files.
	for (const change of plan.vanish_changes) await commit(change);
	return plan.result;
}
