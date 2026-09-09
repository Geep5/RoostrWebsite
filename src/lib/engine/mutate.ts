/** Platform orchestration for the shared Odin mutation planner. */
import type { ChangeJSON } from "./contracts";
import type { ValueJSON, BlockJSON, ObjectJSON } from "$lib/types";
import { spaceKeyGet, spaceKeyRotate, spaceKeyEnsure } from "./spacekeys";
import { coreCall, initCore } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";

export interface MutateCtx {
	/** This device's key-derived author id. */
	author: string;
	changesFor(objectId: string): Promise<ChangeJSON[]>;
	getObject(objectId: string): Promise<{ blocks: BlockJSON[]; fields?: Record<string, ValueJSON>; typeKey?: string } | null>;
	instancesOf(typeKey: string, channel: string): Promise<string[]>;
	objectsWithField(key: string, channel: string): Promise<string[]>;
	/** Complete current state, including tombstones, for cascades and defaults. */
	allObjects(): Promise<ObjectJSON[]>;
	/** Encode, address, durably persist, and publish. */
	commit(change: ChangeJSON): Promise<string>;
}

/** Head change ids: changes no other change lists as a parent. */
export function headsOf(changes: ChangeJSON[]): string[] {
	return coreCall<string[]>("mutation", packCoreValueMaps({ action: "heads", changes }));
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
	const objects = await ctx.allObjects();
	const channelId = typeof params.channel_id === "string" ? params.channel_id : "";
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
		change.parentIds = headsOf(await ctx.changesFor(change.objectId));
		await ctx.commit(change);
	}
	for (const [index, change] of plan.changes.entries()) {
		await commit(change);
		if (action === "channel_create" && index === 0) spaceKeyEnsure(change.objectId);
	}
	// Browsers enforce the synced ledger locally; native hosts additionally purge files.
	for (const change of plan.vanish_changes) await commit(change);
	return plan.result;
}
