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
import { coreCall, initCore } from "../src/lib/engine/core";
import { loadCorpus, needsColdLoad, runQuery } from "../src/lib/engine/query";
import type { ObjectJSON } from "../src/lib/types";

await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

const changeOf = (i: number, name: string) => ({
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

function bytesOf(change: ReturnType<typeof changeOf>): Uint8Array {
	const b64 = coreCall<string>("codec", { action: "encode", change });
	return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

function stateOf(change: ReturnType<typeof changeOf>): ObjectJSON {
	const id = coreCall<string>("codec", { action: "hash", change });
	return coreCall<ObjectJSON>("replay", { changes: [{ ...change, id }] });
}

const body = { filters: [], limit: 500 };

test("a vault loaded from bytes answers exactly like one loaded from JSON", () => {
	const changes = Array.from({ length: 40 }, (_, i) => changeOf(i, `Note ${i}`));

	const fromBytes = loadCorpus(changes.map(bytesOf), true);
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
	loadCorpus([bytesOf(changeOf(900, "Durable name"))], true);
	// The next call writes different bytes into the same reservation.
	loadCorpus([bytesOf(changeOf(901, "ZZZZZZZZ"))], false);
	const rows = runQuery([], { filters: [], limit: 10 }, { upserted: [], removed: [] });
	expect(rows.records.map((r) => r.name).sort()).toEqual(["Durable name", "ZZZZZZZZ"]);
});

test("a big vault loads in batches rather than failing", () => {
	// 9,000 changes is past the per-push bound, so this exercises the
	// batching AND the merge: a later batch must not wipe an earlier one.
	const changes = Array.from({ length: 9000 }, (_, i) => changeOf(i, `Batch ${i}`));
	const out = loadCorpus(changes.map(bytesOf), true);
	expect(out.changes).toBe(9000);
	// `cached` is the core's own count after the last batch - the number that
	// says the merge worked, rather than what one push happened to see.
	expect(out.cached).toBe(9000);
	const rows = runQuery([], { filters: [], limit: 1 }, { upserted: [], removed: [] });
	expect(rows.total).toBe(9000);
});

test("damaged bytes are counted, and an empty load is refused", () => {
	const out = loadCorpus([bytesOf(changeOf(1, "Good")), new Uint8Array([0xff, 0xff, 0xff]), bytesOf(changeOf(2, "Also good"))], true);
	expect(out.objects).toBe(2);
	expect(out.skipped).toBe(1);
	// No bytes is not an empty vault: the caller must not read it as one.
	expect(loadCorpus([], true)).toEqual({ objects: 0, changes: 0, bytes: 0, skipped: 0, cached: 0 });
});

test("a completed load clears the cold-start debt", () => {
	loadCorpus([bytesOf(changeOf(7, "Loaded"))], true);
	expect(needsColdLoad()).toBe(false);
});
