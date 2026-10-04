/**
 * A mutation hands the planner full states only for the objects its params
 * name; every other object goes without blocks (mutate.ts mutationObjects).
 * A vault holding a huge unrelated object - whose blocks alone overrun the
 * core's per-request arena - still mutates, with the plan full states of the
 * named objects give. Same rule and test as RoostrIOS MutationScopeTests.
 */
import { beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { coreCall, initCore } from "../src/lib/engine/core";
import { packCoreValueMaps, unpackCoreValueMaps } from "../src/lib/engine/core-values";
import { mutationObjects } from "../src/lib/engine/mutate";
import { changeId } from "../src/lib/engine/proto";
import { computeObject } from "../src/lib/engine/replay";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

interface Plan {
	changes: ChangeJSON[];
	result: Record<string, unknown>;
}

function plan(action: string, params: Record<string, unknown>, objects: ObjectJSON[], seed: string, timestamp = 1_000): Plan {
	return unpackCoreValueMaps(coreCall<Plan>("mutation", packCoreValueMaps({
		action, params, objects, timestamp, author: "scope-test", id_seed: seed, key_id: 0,
	}))) as Plan;
}

/** A note with one text block, replayed from the planner's own changes. */
function note(name: string, seed: string): ObjectJSON {
	const created = plan("create", { name }, [], seed);
	const id = created.result.id as string;
	const history: ChangeJSON[] = [];
	const addressed = (change: ChangeJSON): ChangeJSON => {
		change.parentIds = history.length ? [history[history.length - 1].id] : [];
		change.id = changeId(change);
		history.push(change);
		return change;
	};
	created.changes.forEach(addressed);
	const state = computeObject(history)!;
	plan("block_add", { object_id: id, block: { content: { text: { text: "first" } } }, target_id: "", position: 0 }, [state], `${seed}-b`, 2_000)
		.changes.forEach(addressed);
	return computeObject(history)!;
}

test("a huge unrelated object neither changes nor breaks a mutation's plan", () => {
	const target = note("Target", "target");
	const big = note("Big", "big");
	const template = big.blocks.find((block) => block.content && "text" in block.content)!;
	const filler = "lorem ipsum ".repeat(25);
	// ~10 MB of blocks: what made the real vault's restart panic the core.
	const huge: ObjectJSON = {
		...big,
		blocks: [...big.blocks, ...Array.from({ length: 25_000 }, (_, i) => ({ ...template, id: `filler-${i}`, content: { text: { text: `${i} ${filler}` } } }))],
	};
	expect(JSON.stringify(packCoreValueMaps(huge)).length).toBeGreaterThan(8_000_000);

	const textBlock = target.blocks.find((block) => block.content && "text" in block.content)!;
	const params = { object_id: target.id, block: { content: { text: { text: "second" } } }, target_id: textBlock.id, position: 2 };
	const scoped = mutationObjects([target, huge], params);
	expect(JSON.stringify(scoped).length).toBeLessThan(100_000);
	expect(scoped[0]).toBe(target);
	expect(scoped[1].blocks).toEqual([]);

	const withScope = plan("block_add", params, scoped, "seed");
	const reference = plan("block_add", params, [target], "seed");
	expect(withScope.changes.length).toBeGreaterThan(0);
	expect(withScope).toEqual(reference);
});
