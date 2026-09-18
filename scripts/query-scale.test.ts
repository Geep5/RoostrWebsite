/**
 * Query scaling contract.
 *
 * The core keeps queried objects resident, so a query should carry only what
 * changed. Two defects broke that:
 *   - a cold start pushed the whole vault in ONE core call, which fails past
 *     the 16 MiB reservation, so a large space could not be queried at all;
 *   - every call re-serialized the entire corpus to diff signatures, making a
 *     view render O(corpus) even when nothing had moved.
 */
import { beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { initCore } from "../src/lib/engine/core";
import { runQuery } from "../src/lib/engine/query";
import type { ObjectJSON } from "../src/lib/types";

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

function vault(n: number, bodyChars: number, prefix = "obj"): ObjectJSON[] {
	const body = "x".repeat(bodyChars);
	const out: ObjectJSON[] = [];
	for (let i = 0; i < n; i++) {
		out.push({
			id: `${prefix}-${i}`,
			typeKey: i % 2 === 0 ? "task" : "note",
			fields: {
				channel: { stringValue: "space" },
				name: { stringValue: `Object ${i}` },
				description: { stringValue: body },
			},
			blocks: [],
			deleted: false,
			createdAt: 1_700_000_000_000 + i,
			updatedAt: 1_700_000_000_000 + i,
		} as unknown as ObjectJSON);
	}
	return out;
}

const body = {
	filters: [{ key: "typeKey", condition: "equal", value: "task" }],
	sorts: [{ key: "updatedAt", desc: true }],
	limit: 10,
};

test("a cold start larger than the core reservation still queries", () => {
	// 16.6 MB of state: past the 16 MiB request reservation that used to
	// reject the push outright. The core's 128 MB cache arena is a separate,
	// measured ceiling - it gives out around 20 MB of object JSON, because
	// each parsed allocation carries its own header.
	const objects = vault(20_000, 700, "cold");
	const payload = objects.reduce((s, o) => s + JSON.stringify(o).length, 0);
	expect(payload).toBeGreaterThan(16 * 1024 * 1024);

	const result = runQuery(objects.values(), body as never);
	expect(result.total).toBe(10_000);
	expect(result.records.length).toBe(10);
});

test("a steady-state query pushes only what the host says changed", () => {
	const objects = vault(500, 100, "delta");
	const byId = new Map(objects.map((o) => [o.id, o]));
	// Seed the core.
	expect(runQuery(byId.values(), body as never).total).toBe(250);

	// Edit one object in place and declare exactly that.
	const edited = byId.get("delta-0")!;
	edited.fields["name"] = { stringValue: "renamed" };
	const after = runQuery(byId.values(), body as never, { upserted: ["delta-0"], removed: [] });
	expect(after.total).toBe(250);
	expect(after.records[0]?.name ?? after.records[0]?.fields?.["name"]?.stringValue).toBeDefined();

	// A declared removal drops the row without re-pushing the corpus.
	byId.delete("delta-0");
	const removed = runQuery(byId.values(), body as never, { upserted: [], removed: ["delta-0"] });
	expect(removed.total).toBe(249);
});

test("an undeclared in-place edit is invisible until it is declared", () => {
	// The delta is a promise from the host: this is the cost of not rescanning
	// the vault, and why the backend records upserts where it mutates state.
	const objects = vault(50, 50, "trust");
	const byId = new Map(objects.map((o) => [o.id, o]));
	runQuery(byId.values(), body as never);

	byId.get("trust-2")!.fields["typeKey"] = { stringValue: "note" };
	const blind = runQuery(byId.values(), body as never, { upserted: [], removed: [] });
	expect(blind.total).toBe(25);
});
