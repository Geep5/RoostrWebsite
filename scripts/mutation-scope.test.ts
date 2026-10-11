/**
 * A mutation request carries no states: the planner reads the core's query
 * cache (core mutate.odin mutation_states), which the host flushes first
 * (query.ts syncQueryCache). An object the params name that the cache lacks
 * comes back `missing` and is retried once as overlay (mutate.ts
 * planMutation). Same contract and test as RoostrIOS MutationScopeTests.
 */
import { beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { coreCall, initCore } from "../src/lib/engine/core";
import { packCoreValueMaps, unpackCoreValueMaps } from "../src/lib/engine/core-values";
import { planMutation, type MutationPlan, type MutationRequest } from "../src/lib/engine/mutate";
import { changeId } from "../src/lib/engine/proto";
import { syncQueryCache } from "../src/lib/engine/query";
import { computeObject } from "../src/lib/engine/replay";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

const request = (action: string, params: Record<string, unknown>, seed: string, timestamp = 1_000): MutationRequest =>
	({ action, params, timestamp, author: "scope-test", id_seed: seed, key_id: 0 });

/** The old contract: the caller's complete snapshot, the planner's reference answer. */
const snapshotPlan = (req: MutationRequest, objects: ObjectJSON[]): MutationPlan =>
	unpackCoreValueMaps(coreCall<MutationPlan>("mutation", packCoreValueMaps({ ...req, objects }))) as MutationPlan;

/** Replays planned changes into per-object histories, as a host commits them. */
class Histories {
	private byId = new Map<string, ChangeJSON[]>();
	commit(plan: MutationPlan): string[] {
		for (const change of plan.changes) {
			const history = this.byId.get(change.objectId) ?? [];
			change.parentIds = history.length ? [history[history.length - 1].id] : [];
			change.id = changeId(change);
			history.push(change);
			this.byId.set(change.objectId, history);
		}
		return [...new Set(plan.changes.map((change) => change.objectId))];
	}
	state(id: string): ObjectJSON {
		return computeObject(this.byId.get(id)!)!;
	}
}

/** A note with one text block, replayed from the planner's own changes. */
function note(histories: Histories, name: string, seed: string): ObjectJSON {
	const created = snapshotPlan(request("create", { name }, seed), []);
	const id = created.result.id as string;
	histories.commit(created);
	histories.commit(snapshotPlan(request("block_add", { object_id: id, block: { content: { text: { text: "first" } } }, target_id: "", position: 0 }, `${seed}-b`, 2_000), [histories.state(id)]));
	return histories.state(id);
}

const blockAdd = (target: ObjectJSON) => ({
	object_id: target.id,
	block: { content: { text: { text: "second" } } },
	target_id: target.blocks.find((block) => block.content && "text" in block.content)!.id,
	position: 2,
});

test("a mutation plans over the cache: no states shipped, the snapshot's plan", async () => {
	const histories = new Histories();
	const target = note(histories, "Target", "target");
	const big = note(histories, "Big", "big");
	const template = big.blocks.find((block) => block.content && "text" in block.content)!;
	const filler = "lorem ipsum ".repeat(25);
	// ~10 MB of blocks: the object that used to overrun the per-request arena.
	const huge: ObjectJSON = {
		...big,
		blocks: [...big.blocks, ...Array.from({ length: 25_000 }, (_, i) => ({ ...template, id: `filler-${i}`, content: { text: { text: `${i} ${filler}` } } }))],
	};
	syncQueryCache([target, huge], { upserted: [target.id, huge.id], removed: [] });

	const req = request("block_add", blockAdd(target), "seed");
	expect(JSON.stringify(packCoreValueMaps(req)).length).toBeLessThan(1_000);
	const fetched: string[][] = [];
	const plan = await planMutation(req, async () => {}, async (ids) => (fetched.push(ids), []));
	expect(fetched).toEqual([]);
	expect(plan.changes.length).toBeGreaterThan(0);
	expect(plan).toEqual(snapshotPlan(req, [target, huge]));
});

test("an object the cache lacks is fetched and the plan retried once with it", async () => {
	const histories = new Histories();
	const target = note(histories, "Uncached", "uncached");
	const req = request("block_add", blockAdd(target), "seed-missing");
	const fetched: string[][] = [];
	const plan = await planMutation(req, async () => {}, async (ids) => (fetched.push(ids), ids.includes(target.id) ? [target] : []));
	expect(fetched).toEqual([[target.id]]);
	expect(plan).toEqual(snapshotPlan(req, [target]));

	// An id no replica holds plans as the snapshot without it would.
	const ghost = request("set_field", { object_id: "ghost-object", key: "status", value: { stringValue: "done" } }, "seed-ghost");
	expect(await planMutation(ghost, async () => {}, async () => [])).toEqual(snapshotPlan(ghost, []));
});

test("consecutive mutations see each other: each plan flushes the commits before it", async () => {
	const histories = new Histories();
	// What the backend's dirty set holds between a commit and the next plan.
	let pending: string[] = [];
	const sync = async () => {
		syncQueryCache(pending.map((id) => histories.state(id)), { upserted: pending, removed: [] });
		pending = [];
	};
	const fetched: string[][] = [];
	const statesOf = async (ids: string[]) => (fetched.push(ids), []);
	const space = await planMutation(request("create", { type_key: "channel", name: "Home" }, "space"), sync, statesOf);
	pending = histories.commit(space);
	const created = await planMutation(request("create", { name: "Fresh" }, "fresh", 2_000), sync, statesOf);
	const id = created.result.id as string;
	// The note lands in the space created just before it: a cache scan.
	const fields = created.changes.find((change) => change.objectId === id)!.ops.flatMap((op) => (op.fieldSet ? [op.fieldSet] : []));
	expect(fields.find((field) => field.key === "channel")?.value).toEqual({ stringValue: space.result.id });
	pending = histories.commit(created);
	const added = await planMutation(request("block_add", { object_id: id, block: { content: { text: { text: "hi" } } }, target_id: "", position: 0 }, "fresh-b", 3_000), sync, statesOf);
	expect(fetched).toEqual([]);
	expect(added.changes.map((change) => change.objectId)).toEqual([id]);
});
