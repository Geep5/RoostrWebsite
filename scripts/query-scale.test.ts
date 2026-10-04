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
import { coreCall, coreCallWithBlob, initCore, resetCore } from "../src/lib/engine/core";
import { changeId, encodeChange } from "../src/lib/engine/proto";
import { runQuery } from "../src/lib/engine/query";
import type { ChangeJSON } from "../src/lib/engine/contracts";
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
	// reject the push outright. The core's 128 MB cache is a separate ceiling;
	// a cached object holds only its compact typed state (see below).
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

/** cacheBytes as the core reports it: a one-change corpus push that adds a probe object. */
function cacheBytes(): number {
	const change: ChangeJSON = {
		id: "", objectId: "cache-probe", parentIds: [], timestamp: 1, author: "probe",
		ops: [{ objectCreate: { typeKey: "note" } }],
	};
	change.id = changeId(change);
	const bytes = encodeChange(change);
	const blob = new Uint8Array(4 + bytes.byteLength);
	new DataView(blob.buffer).setUint32(0, bytes.byteLength, true);
	blob.set(bytes, 4);
	return coreCallWithBlob<{ cacheBytes: number }>("corpus", { action: "push", reset: false }, blob).cacheBytes;
}

test("a block-heavy vault caches compact state, and a reset never holds two snapshots", () => {
	// The phone that could not open its vault: 1,548 objects / 8.19 MB of
	// state JSON, mostly small block JSON objects, became 112 MB of cache
	// (each kept its parsed JSON tree) - and a retry reset parsed a second
	// copy before freeing the first. This vault is ~12 MB of such JSON.
	const objects: ObjectJSON[] = [];
	for (let i = 0; i < 1_500; i++) {
		objects.push({
			id: `doc-${i}`, typeKey: "note", fields: { name: { stringValue: `Doc ${i}` } },
			blocks: Array.from({ length: 90 }, (_, b) => ({ id: `doc-${i}-${b}`, content: { text: { text: `line ${b} of doc ${i}` } }, childrenIds: [] })),
			deleted: false, createdAt: 1, updatedAt: 1 + i,
		} as unknown as ObjectJSON);
	}
	const payload = objects.reduce((s, o) => s + JSON.stringify(o).length, 0);
	expect(payload).toBeGreaterThan(10 * 1024 * 1024);
	resetCore();
	const first = runQuery(objects.values(), { filters: [], limit: 1, textQuery: "line 89 of doc 1499" } as never);
	expect(first.total).toBe(1);
	const cached = cacheBytes();
	// The typed model is the floor (~300 bytes per block against ~85 of this
	// JSON); the parsed JSON tree put it near 14x, past the 128 MB ceiling.
	expect(cached).toBeLessThan(5 * payload);
	// A second cold load over the live cache (the "Retry opening vault" path),
	// in host-sized batches (pushAndQuery: 6 MiB of JSON per request).
	let retried = { total: 0 };
	for (let start = 0; start < objects.length; start += 500) {
		retried = coreCall<{ total: number }>("query", { reset: start === 0, upserts: objects.slice(start, start + 500), removed: [], body: { limit: 1 }, nowMs: 1 });
	}
	expect(retried.total).toBe(1_500);
	expect(cacheBytes()).toBeLessThan(cached * 1.1);
	resetCore();
});
