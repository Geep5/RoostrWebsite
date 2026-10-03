/** Platform orchestration for the shared Odin mutation planner. */
import type { ChangeJSON } from "./contracts";
import type { ValueJSON, BlockJSON, ObjectJSON } from "$lib/types";
import { spaceKeyAll, spaceKeyGet, spaceKeyRotate, spaceKeyEnsure, spaceKeyRemove, spaceOwner } from "./spacekeys";
import { coreCall, initCore } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";
import { bytesToBase64 } from "./proto";

/** Everything the DAG holds for one object: stored changes plus its checkpoint, if any. */
export interface ObjectDag {
	changes: ChangeJSON[];
	checkpoint?: Uint8Array;
}

export interface MutateCtx {
	/** This device's key-derived author id. */
	author: string;
	/** This identity's hex pubkey: space ownership is decided against it. */
	pk: string;
	/** Applying the space owner's kind-5 deletion signal: vanishing a space
	 * someone else owns is then this identity's part of the owner's delete,
	 * not a member deleting it for everyone. */
	ownerSignal: boolean;
	dagFor(objectId: string): Promise<ObjectDag>;
	getObject(objectId: string): Promise<{ blocks: BlockJSON[]; fields?: Record<string, ValueJSON>; typeKey?: string } | null>;
	instancesOf(typeKey: string, channel: string): Promise<string[]>;
	objectsWithField(key: string, channel: string): Promise<string[]>;
	/** Complete current state, including tombstones, for cascades and defaults. */
	allObjects(): Promise<ObjectJSON[]>;
	/** Encode, address, durably persist, and publish. */
	commit(change: ChangeJSON): Promise<string>;
}

/** Head change ids: changes no other change lists as a parent, plus checkpoint heads nothing built on. */
export function headsOf(dag: ObjectDag): string[] {
	const payload: Record<string, unknown> = { action: "heads", changes: dag.changes };
	if (dag.checkpoint) payload.checkpoint = bytesToBase64(dag.checkpoint);
	return coreCall<string[]>("mutation", packCoreValueMaps(payload));
}

interface MutationPlan {
	changes: ChangeJSON[];
	result: Record<string, unknown>;
	vanish_ids: string[];
	vanish_changes: ChangeJSON[];
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
	const objects = await ctx.allObjects();
	const rotating = action === "channel_member_remove" || action === "channel_key_rotate";
	const keyId = rotating ? (spaceKeyGet(channelId)?.keyId ?? 0) + 1 : 0;
	const plan = unpackCoreValueMaps(coreCall<MutationPlan>("mutation", packCoreValueMaps({
		action,
		params,
		objects,
		timestamp: Date.now(),
		author: ctx.author,
		id_seed: crypto.randomUUID(),
		key_id: keyId,
	}))) as MutationPlan;

	// Key material stays in the host; invalid domain requests never rotate keys.
	if (rotating) spaceKeyRotate(channelId);
	async function commit(change: ChangeJSON): Promise<void> {
		change.parentIds = headsOf(await ctx.dagFor(change.objectId));
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
