import assert from "node:assert/strict";
import { coreCall, initCore, resetCore } from "../src/lib/engine/core";
import { runQuery, type QueryRow } from "../src/lib/engine/query";
import type { ObjectJSON } from "../src/lib/types";
import type { QueryBody } from "../src/lib/engine/contracts";

type Result = { total: number; records: QueryRow[] };
type Fixture = {
	nowMs: number;
	objects: ObjectJSON[];
	cases: { name: string; body: QueryBody; ids: string[]; total: number; snippet?: string }[];
};
const fixture: Fixture = await Bun.file(new URL("../../glonOdin/fixtures/query-parity.json", import.meta.url)).json();
await initCore({ wasmBytes: await Bun.file(new URL("../static/engine.wasm", import.meta.url)).arrayBuffer() });
const originalNow = Date.now;
Date.now = () => fixture.nowMs;
try {
	for (const c of fixture.cases) {
		const direct = coreCall<Result>("query", { objects: fixture.objects, body: c.body, nowMs: fixture.nowMs });
		assert.deepEqual(direct.records.map((row) => row.id), c.ids, c.name);
		assert.equal(direct.total, c.total, c.name);
		if (c.snippet) assert.equal(direct.records[0]?.snippet, c.snippet, c.name);
		assert.deepEqual(runQuery(fixture.objects, c.body), direct, `${c.name}: cached browser adapter`);
	}

	const objects = structuredClone(fixture.objects);
	const body: QueryBody = { type: "page", sorts: [{ key: "id", type: "asc" }] };
	const a = objects.find((object) => object.id === "a")!;
	const before = runQuery(objects, body);
	a.fields.name = { stringValue: "Changed in place without touching updatedAt" };
	assert.equal(runQuery(objects, body).records[0].name, "Changed in place without touching updatedAt");
	assert.equal(before.records[0].name, "Alpha", "returned rows are independent snapshots");
	const withoutA = objects.filter((object) => object.id !== "a");
	assert.deepEqual(runQuery(withoutA, body).records.map((row) => row.id), ["b", "c", "d"]);
	assert.equal(runQuery([a], body).records[0].name, "Changed in place without touching updatedAt");
	assert.deepEqual(runQuery([], body), { total: 0, records: [] });
	assert.equal(runQuery([a, { ...a, fields: { name: { stringValue: "Last duplicate wins" } } }], body).records[0].name, "Last duplicate wins");
	resetCore();
	assert.equal(runQuery([a], body).records[0].name, "Changed in place without touching updatedAt", "reset generation rehydrates cache");
	assert.throws(() => coreCall("query", { objects: [], body: {} }));
	assert.throws(() => coreCall("query", { objects: false, body: {}, nowMs: 0 }));
	assert.throws(() => coreCall("query", { upserts: [], removed: [42], body: {}, nowMs: 0 }));
	assert.throws(() => coreCall("query", { objects: [], body: {}, nowMs: 1e100 }));
	assert.throws(() => coreCall("query", { objects: [], body: { limit: 1e100 }, nowMs: 0 }));
	console.log(`${fixture.cases.length} query fixtures passed through WASM and cached browser adapter; cache transition checks passed`);
} finally {
	Date.now = originalNow;
	resetCore();
}
