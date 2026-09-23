/**
 * Cold start from raw change bytes.
 *
 * The replica stores every change's protobuf beside its JSON, so seeding the
 * core costs no serialisation at all. What that replaces, measured here:
 * the JSON push stringifies each object host-side, the ABI parses it, and the
 * cache re-marshals and re-parses it into its own region - three per object.
 *
 * Two properties are load-bearing, and both failed before they were pinned:
 * the loaded state must answer queries identically to the JSON path, and it
 * must keep answering after the blob is reused - the codec borrows strings
 * from the bytes it reads, so a cached state that points at the request
 * buffer reads garbage on the next call.
 */

import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { coreCall, initCore, resetCore } from "../src/lib/engine/core";
import { loadCorpus, runQuery } from "../src/lib/engine/query";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { backend } from "../src/lib/engine/backend";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

const changeOf = (i: number, name: string): ChangeJSON => ({
	id: "",
	objectId: `obj-${i}`,
	parentIds: [] as string[],
	timestamp: i + 1,
	author: "test",
	ops: [
		{ objectCreate: { typeKey: "note" } },
		{ fieldSet: { key: "name", value: { stringValue: name } } },
		{ fieldSet: { key: "done", value: { boolValue: i % 2 === 0 } } },
	],
});

function bytesOf(change: ChangeJSON): Uint8Array {
	const b64 = coreCall<string>("codec", { action: "encode", change });
	return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

function stateOf(change: ChangeJSON): ObjectJSON {
	const id = coreCall<string>("codec", { action: "hash", change });
	return coreCall<ObjectJSON>("replay", { changes: [{ ...change, id }] });
}

const body = { filters: [], limit: 500 };

test("a vault loaded from bytes answers exactly like one loaded from JSON", () => {
	const changes = Array.from({ length: 40 }, (_, i) => changeOf(i, `Note ${i}`));

	const fromBytes = loadCorpus(changes.map((c) => ({ changes: [bytesOf(c)] })));
	expect(fromBytes.objects).toBe(40);
	expect(fromBytes.changes).toBe(40);
	expect(fromBytes.skipped).toBe(0);
	const corpusRows = runQuery([], body, { upserted: [], removed: [] });
	expect(corpusRows.total).toBe(40);
	const corpusNames = corpusRows.records.map((r) => r.name).sort();

	// Same objects, pushed the old way.
	const states = changes.map(stateOf);
	const jsonRows = runQuery(states, body);
	expect(jsonRows.total).toBe(40);
	expect(jsonRows.records.map((r) => r.name).sort()).toEqual(corpusNames);
});

test("a cached object survives the blob being reused", () => {
	loadCorpus([{ changes: [bytesOf(changeOf(900, "Durable name"))] }]);
	// The next call writes different bytes into the same reservation.
	loadCorpus([{ changes: [bytesOf(changeOf(901, "ZZZZZZZZ"))] }], false);
	const rows = runQuery([], { filters: [], limit: 10 }, { upserted: [], removed: [] });
	expect(rows.records.map((r) => r.name).sort()).toEqual(["Durable name", "ZZZZZZZZ"]);
});

test("a big vault loads in batches rather than failing", () => {
	// 9,000 changes is past the per-push bound, so this exercises the
	// batching AND the merge: a later batch must not wipe an earlier one.
	const changes = Array.from({ length: 9000 }, (_, i) => changeOf(i, `Batch ${i}`));
	const out = loadCorpus(changes.map((c) => ({ changes: [bytesOf(c)] })));
	expect(out.changes).toBe(9000);
	// `cached` is the core's own count after the last batch - the number that
	// says the merge worked, rather than what one push happened to see.
	expect(out.cached).toBe(9000);
	const rows = runQuery([], { filters: [], limit: 1 }, { upserted: [], removed: [] });
	expect(rows.total).toBe(9000);
});

test("damaged changes are counted without discarding healthy objects", () => {
	const out = loadCorpus([{ changes: [bytesOf(changeOf(1, "Good"))] }, { changes: [new Uint8Array([0xff, 0xff, 0xff])] }, { changes: [bytesOf(changeOf(2, "Also good"))] }]);
	expect(out.skipped).toBe(1);
	const rows = runQuery([], body, { upserted: [], removed: [] });
	expect(rows.records.map((r) => r.name).sort()).toEqual(["Also good", "Good"]);
});

test("an empty reset load replaces the previous vault", () => {
	loadCorpus([{ changes: [bytesOf(changeOf(7, "Previous vault"))] }]);
	loadCorpus([]);
	expect(runQuery([], body, { upserted: [], removed: [] }).total).toBe(0);
});

test("hash-ordered storage keeps complete histories across corpus batches", async () => {
	const name = `corpus-histories-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	try {
		const creation = changeOf(9000, "Original");
		creation.id = coreCall<string>("codec", { action: "hash", change: creation });
		const update: ChangeJSON = {
			...creation,
			id: "",
			parentIds: [creation.id],
			timestamp: creation.timestamp + 1,
			ops: [{ fieldSet: { key: "name", value: { stringValue: "Updated" } } }],
		};
		const changes = [creation, ...Array.from({ length: 4001 }, (_, i) => changeOf(i, `Filler ${i}`)), update];
		await store.addChanges(changes.map((change) => {
			change.id = coreCall<string>("codec", { action: "hash", change });
			return { change, bytes: bytesOf(change) };
		}));
		loadCorpus(await store.allChangeHistories());
		const rows = runQuery([], { type: "note", filters: [{ key: "id", condition: "equal", value: creation.objectId }] }, { upserted: [], removed: [] });
		expect(rows.records).toMatchObject([{
			id: creation.objectId, typeKey: "note", name: "Updated", createdAt: creation.timestamp,
			fields: { done: { boolValue: true } },
		}]);
		expect(rows.total).toBe(1);
	} finally {
		store.close();
		await destroyDatabase(name);
	}
});

test("one long object history is never split at a change-count boundary", () => {
	const creation = changeOf(1, "Long history");
	creation.id = coreCall<string>("codec", { action: "hash", change: creation });
	const history = [bytesOf(creation)];
	let parent = creation.id;
	for (let i = 0; i < 4001; i++) {
		const change: ChangeJSON = {
			...creation, id: "", parentIds: [parent], timestamp: creation.timestamp + i + 1,
			ops: [{ fieldSet: { key: "revision", value: { intValue: i } } }],
		};
		parent = coreCall<string>("codec", { action: "hash", change });
		history.push(bytesOf(change));
	}
	loadCorpus([{ changes: history }]);
	const rows = runQuery([], { type: "note" }, { upserted: [], removed: [] });
	expect(rows.records).toMatchObject([{
		id: creation.objectId, name: "Long history", createdAt: creation.timestamp,
		fields: { revision: { intValue: 4000 } },
	}]);
	expect(rows.total).toBe(1);
});

test("a refused later history leaves JSON fallback owing a full reset", () => {
	const histories = Array.from({ length: 4001 }, (_, i) => ({ changes: [bytesOf(changeOf(i, `Partial ${i}`))] }));
	// Framing counts toward the byte limit too. Never split an oversized object.
	histories.push({ changes: [new Uint8Array(24 * 1024 * 1024)] });
	expect(() => loadCorpus(histories)).toThrow("Object history exceeds the corpus batch limit");
	const survivor = stateOf(changeOf(8000, "Authoritative snapshot"));
	const rows = runQuery([survivor], body, { upserted: [], removed: [] });
	expect(rows.records.map((r) => r.name)).toEqual(["Authoritative snapshot"]);
});

test("cold queries and core resets cannot resurrect vanished objects", async () => {
	// The backend exposes a singleton; isolate its store and restore every
	// touched field so this scenario can share a process with other tests.
	interface Internals {
		store: ChangeStore;
		states: Map<string, ObjectJSON>;
		dirty: Set<string>;
		vanished: Set<string>;
		queryUpserted: Set<string>;
		queryRemoved: Set<string>;
		allDirty: boolean;
	}
	const internals = backend as unknown as Internals;
	const saved = {
		store: internals.store, states: internals.states, dirty: internals.dirty, vanished: internals.vanished,
		queryUpserted: internals.queryUpserted, queryRemoved: internals.queryRemoved, allDirty: internals.allDirty,
	};
	const name = `corpus-vanished-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	resetCore();
	try {
		const gone = changeOf(1, "Must stay vanished");
		const live = changeOf(2, "Survivor");
		const ledger: ChangeJSON = {
			id: "", objectId: "__vanished__", parentIds: [], timestamp: 100, author: "test",
			ops: [{ objectCreate: { typeKey: "vanish_log" } }, {
				fieldSet: { key: `vanished:${gone.objectId}`, value: { intValue: 100 } },
			}],
		};
		const changes = [gone, live, ledger];
		await store.addChanges(changes.map((change) => {
			change.id = coreCall<string>("codec", { action: "hash", change });
			return { change, bytes: bytesOf(change) };
		}));
		// Exercise a warm database: there are no replay upserts to mask an
		// incorrectly seeded corpus.
		for (const change of changes) await store.putState(change.objectId, 1, "", stateOf(change));
		Object.assign(internals, {
			store, states: new Map(), dirty: new Set(), vanished: new Set(),
			queryUpserted: new Set(), queryRemoved: new Set(), allDirty: true,
		});
		expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["Survivor"]);
		expect((await backend.fetchQuery(body)).records.map((r: { name: string }) => r.name)).toEqual(["Survivor"]);
		resetCore();
		expect((await backend.fetchQuery(body)).records.map((r: { name: string }) => r.name)).toEqual(["Survivor"]);
	} finally {
		Object.assign(internals, saved);
		resetCore();
		store.close();
		await destroyDatabase(name);
	}
});
