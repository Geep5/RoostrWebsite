/**
 * A mutation request carries no states: the planner reads the core's query
 * cache (core mutate.odin mutation_states), which the host flushes first
 * (query.ts syncQueryCache). An object the params name that the cache lacks
 * comes back `missing` and is retried once as overlay (mutate.ts
 * planMutation). A local commit is applied to that cache by the core
 * (query.ts applyCommitted), so its state is never re-sent. Same contract and
 * test as RoostrIOS MutationScopeTests.
 */
import { beforeAll, expect, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { coreCall, initCore, resetCore } from "../src/lib/engine/core";
import { backend } from "../src/lib/engine/backend";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
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

test("a local commit is applied by the core: no state upsert follows, and the next plan sees it", async () => {
	// The backend is a singleton: isolate its store and restore every touched field.
	interface Internals {
		store: ChangeStore;
		states: Map<string, ObjectJSON>;
		dirty: Set<string>;
		vanished: Set<string>;
		queryUpserted: Set<string>;
		queryRemoved: Set<string>;
		allDirty: boolean;
		author: string;
		pk: string;
	}
	const internals = backend as unknown as Internals;
	const saved = {
		store: internals.store, states: internals.states, dirty: internals.dirty, vanished: internals.vanished, queryUpserted: internals.queryUpserted,
		queryRemoved: internals.queryRemoved, allDirty: internals.allDirty, author: internals.author, pk: internals.pk,
	};
	const name = `apply-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	resetCore();
	// Every state the host sends the core's cache, by object id.
	const upserted: string[] = [];
	const encode = TextEncoder.prototype.encode;
	const spy = spyOn(TextEncoder.prototype, "encode").mockImplementation(function (this: TextEncoder, input?: string) {
		if (input?.startsWith('{"method":"query"')) {
			for (const state of (JSON.parse(input).payload.upserts ?? []) as { id: string }[]) upserted.push(state.id);
		}
		return encode.call(this, input);
	});
	try {
		Object.assign(internals, {
			store, states: new Map(), dirty: new Set(), vanished: new Set(),
			queryUpserted: new Set(), queryRemoved: new Set(), allDirty: true,
			author: "apply-test", pk: "ab".repeat(32),
		});
		const space = (await backend.mutate("create", { type_key: "channel", name: "Home" })).id as string;
		const note = (await backend.mutate("create", { name: "Page" })).id as string;
		await backend.mutate("block_add", { object_id: note, block: { content: { text: { text: "typed" } } }, target_id: "", position: 0 });
		await backend.mutate("set_field", { object_id: note, key: "status", value: { stringValue: "draft" } });
		// table_row_add plans from the table the cache holds: it must see the create.
		const table = (await backend.mutate("table_create", { object_id: note, rows: 1, cols: 2 })).id as string;
		await backend.mutate("table_row_add", { object_id: note, table_id: table });
		await backend.mutate("delete", { object_id: note });
		await backend.mutate("restore", { object_id: note });
		expect(upserted).toEqual([]);
		expect(internals.queryUpserted.size).toBe(0);
		// A relay echoing this device's own changes re-replays the object to
		// the same heads: still nothing owed.
		internals.dirty.add(note);
		internals.dirty.add(space);
		await backend.fetchObjects();
		expect(internals.queryUpserted.size).toBe(0);

		// The cache the planner reads is the replica's replay, field for field.
		const rows = (await backend.fetchQuery({ filters: [{ key: "id", condition: "equal", value: note }] })).records as { fields: unknown; name: string }[];
		expect(rows.length).toBe(1);
		const replica = internals.states.get(note)!;
		expect(rows[0].fields).toEqual(replica.fields);
		expect(replica.blocks.filter((block) => block.content && "tableRow" in block.content).length).toBe(2);
		expect(upserted).toEqual([]);
		expect(internals.states.get(space)!.heads?.length).toBe(1);

		// A cache whose state carries no heads (an older replay) refuses the
		// apply: the replayed state is upserted as before, and with its heads
		// the next commit applies again.
		const { heads: _, ...headless } = replica;
		syncQueryCache([headless], { upserted: [note], removed: [] });
		upserted.length = 0;
		const warn = spyOn(console, "warn").mockImplementation(() => {});
		await backend.mutate("set_field", { object_id: note, key: "status", value: { stringValue: "refused" } });
		warn.mockRestore();
		expect(internals.queryUpserted.has(note)).toBe(true);
		await backend.mutate("set_field", { object_id: note, key: "status", value: { stringValue: "applied" } });
		expect(upserted).toEqual([note]);
		expect(internals.queryUpserted.size).toBe(0);
		const after = (await backend.fetchQuery({ filters: [{ key: "status", condition: "equal", value: "applied" }] })).records as { id: string }[];
		expect(after.map((row) => row.id)).toEqual([note]);
		expect(upserted).toEqual([note]);
	} finally {
		spy.mockRestore();
		Object.assign(internals, saved);
		resetCore();
		store.close();
		await destroyDatabase(name);
	}
});
