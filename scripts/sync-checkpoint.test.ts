import { afterEach, beforeAll, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, nip19, nip44, type Event } from "nostr-tools";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { initCore, resetCore } from "../src/lib/engine/core";
import { blindShared, RelaySync, type SharedSpaceInfo } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { bytesToBase64, changeId, encodeChange } from "../src/lib/engine/proto";
import { computeObject } from "../src/lib/engine/replay";
import { backend } from "../src/lib/engine/backend";
import type { ChangeJSON, CheckpointRow } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

// The browser never builds checkpoints; a publisher's kind-1079 payload is
// hand-assembled here from the wire layout in core/proto.odin
// (Checkpoint: 1 objectId, 2 headIds, 3 coveredIds, 4 Snapshot, 5 createdAt;
// Snapshot: 1 id, 2 typeKey, 3 {1 key, 2 Value}, 7 createdAt, 8 updatedAt; Value.stringValue = 1).

const ownerSk = new Uint8Array(32).fill(1), memberSk = new Uint8Array(32).fill(2);
const owner = getPublicKey(ownerSk), member = getPublicKey(memberSk);
const spaceKey = new Uint8Array(32).fill(4);
const space: SharedSpaceInfo = { spaceId: "space", keyId: 1, keyHex: bytesToHex(spaceKey), owner, writers: [owner, member] };
const personalKey = nip44.getConversationKey(ownerSk, owner);

function varint(n: number): number[] {
	const out: number[] = [];
	while (n >= 0x80) { out.push((n & 0x7f) | 0x80); n >>>= 7; }
	out.push(n);
	return out;
}
function field(no: number, bytes: Uint8Array | number[]): number[] {
	return [...varint((no << 3) | 2), ...varint(bytes.length), ...bytes];
}
const utf8 = (s: string) => new TextEncoder().encode(s);
function checkpointBytes(objectId: string, typeKey: string, fields: Record<string, string>, heads: string[], covered: string[], createdAt: number): Uint8Array {
	const snapshot = [
		...field(1, utf8(objectId)),
		...field(2, utf8(typeKey)),
		...Object.entries(fields).flatMap(([key, value]) => field(3, [...field(1, utf8(key)), ...field(2, field(1, utf8(value)))])),
		...varint((7 << 3) | 0), ...varint(100),
		...varint((8 << 3) | 0), ...varint(100),
	];
	return new Uint8Array([
		...field(1, utf8(objectId)),
		...heads.flatMap((id) => field(2, hexToBytes(id))),
		...covered.flatMap((id) => field(3, hexToBytes(id))),
		...field(4, snapshot),
		...varint((5 << 3) | 0), ...varint(createdAt),
	]);
}
function change(objectId: string, ops: ChangeJSON["ops"], parentIds: string[] = []): ChangeJSON {
	const value: ChangeJSON = { id: "", objectId, ops, parentIds, timestamp: 100, author: owner };
	value.id = changeId(value);
	return value;
}
const name = (value: string): ChangeJSON["ops"][number] => ({ fieldSet: { key: "name", value: { stringValue: value } } });

let createSpace: ChangeJSON, createDoc: ChangeJSON, edit: ChangeJSON;
let c1: Uint8Array, c2: Uint8Array;
beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
	createSpace = change("space", [{ objectCreate: { typeKey: "channel" } }, { fieldSet: { key: "members", value: { valuesValue: { items: [{ mapValue: { entries: { npub: { stringValue: nip19.npubEncode(member) }, role: { stringValue: "writer" } } } }] } } } }]);
	createDoc = change("doc", [{ objectCreate: { typeKey: "note" } }, { fieldSet: { key: "channel", value: { stringValue: "space" } } }, name("v1")]);
	edit = change("doc", [name("v2")], [createDoc.id]);
	c1 = checkpointBytes("doc", "note", { channel: "space", name: "v1" }, [createDoc.id], [createDoc.id], 1);
	c2 = checkpointBytes("doc", "note", { channel: "space", name: "v2" }, [edit.id], [createDoc.id, edit.id], 2);
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });
async function storeFixture() {
	const dbName = `checkpoint-test-${crypto.randomUUID()}`;
	const store = new ChangeStore(dbName);
	await store.open();
	cleanup.push(async () => { store.close(); await destroyDatabase(dbName); });
	return store;
}
interface SyncInternals {
	ingestEvent(event: Event): Promise<{ objectId: string; checkpoint?: { hash: string } } | null>;
	importBatch(batch: unknown[]): Promise<void>;
	backfillChain: Promise<boolean>;
}
function syncFixture(store: ChangeStore) {
	const sync = new RelaySync(ownerSk, [], store, { onObjects() {}, onStatus() {} });
	const internals = sync as unknown as SyncInternals;
	sync.setSharedSpaces([space]);
	cleanup.push(async () => { sync.stop(); await internals.backfillChain; });
	return { sync, internals };
}
/** Kind-1079 event: personal (sealed under our conversation key) or shared (space key, owner's or a member's signature). */
function checkpointEvent(bytes: Uint8Array, shared: boolean, sk = ownerSk): Event {
	const key = shared ? spaceKey : personalKey;
	const tags = shared ? [["h", blindShared(bytesToHex(spaceKey), "space:space")]] : [];
	return finalizeEvent({ kind: 1079, created_at: 100, content: nip44.encrypt(bytesToBase64(bytes), key), tags }, sk);
}
async function importEvent(internals: SyncInternals, event: Event): Promise<boolean> {
	const item = await internals.ingestEvent(event);
	if (!item) return false;
	await internals.importBatch([item]);
	return true;
}

describe("checkpoint replay", () => {
	test("a checkpoint stands in for the history it covers", () => {
		const full = computeObject([createDoc, edit])!;
		expect(computeObject([], c2)).toEqual(full);
		expect(computeObject([edit], c1)).toEqual(full);
		// A checkpoint that does not fit the held changes is ignored, never trusted over them.
		const stranger = checkpointBytes("doc", "note", { name: "forged" }, [edit.id], ["ab".repeat(32)], 3);
		expect(computeObject([createDoc, edit], stranger)).toEqual(full);
	});
});

describe("checkpoint import", () => {
	test("personal checkpoints land in the store and only a covering one supersedes", async () => {
		const store = await storeFixture();
		const { sync, internals } = syncFixture(store);
		expect(await importEvent(internals, checkpointEvent(c2, false))).toBe(true);
		const held = (await store.getCheckpoint("doc"))!;
		expect(held).toMatchObject({ objectId: "doc", heads: [edit.id] });
		expect(sync.stats.checkpoints).toBe(1);
		// The narrower, older-generation checkpoint arrives late: a clean no-op.
		expect(await importEvent(internals, checkpointEvent(c1, false))).toBe(true);
		expect((await store.getCheckpoint("doc"))!.hash).toBe(held.hash);
		expect(sync.stats.checkpoints).toBe(1);
		// Another device checkpointed a different branch of equal width: neither
		// covers the other, so the held one stays no matter the hash.
		const fork = change("doc", [name("fork")], [createDoc.id]);
		const forkBytes = checkpointBytes("doc", "note", { channel: "space", name: "fork" }, [fork.id], [createDoc.id, fork.id], 2);
		const forkRow: CheckpointRow = { objectId: "doc", bytes: forkBytes, hash: "ff".repeat(32), heads: [fork.id] };
		expect(await store.putCheckpoint(forkRow)).toBe(false);
		// Same covered set, different bytes: the higher (real) hash wins on every replica.
		const rivalBytes = checkpointBytes("doc", "note", { channel: "space", name: "v2" }, [edit.id], [createDoc.id, edit.id], 9);
		const rival: CheckpointRow = { objectId: "doc", bytes: rivalBytes, hash: bytesToHex(sha256(rivalBytes)), heads: held.heads };
		expect(held.hash).toBe(bytesToHex(sha256(c2)));
		expect(await store.putCheckpoint(rival)).toBe(rival.hash > held.hash);
	});
	test("shared checkpoints are accepted from the space owner only", async () => {
		const store = await storeFixture();
		await store.addChanges([{ change: createSpace, bytes: encodeChange(createSpace) }]);
		const { internals } = syncFixture(store);
		expect(await importEvent(internals, checkpointEvent(c2, true, memberSk))).toBe(true);
		expect(await store.getCheckpoint("doc")).toBeUndefined();
		expect(await importEvent(internals, checkpointEvent(c2, true, ownerSk))).toBe(true);
		expect((await store.getCheckpoint("doc"))?.heads).toEqual([edit.id]);
		// An owner's checkpoint cannot pull an object from another scope into this space.
		const foreign = checkpointBytes("elsewhere", "note", { channel: "other", name: "x" }, [edit.id], [edit.id], 1);
		expect(await importEvent(internals, checkpointEvent(foreign, true, ownerSk))).toBe(true);
		expect(await store.getCheckpoint("elsewhere")).toBeUndefined();
	});
	test("an object held only as a checkpoint is visible after a cold boot", async () => {
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
		const store = await storeFixture();
		resetCore();
		try {
			expect(await importEvent(syncFixture(store).internals, checkpointEvent(c2, false))).toBe(true);
			expect(await store.changesFor("doc")).toEqual([]);
			Object.assign(internals, {
				store, states: new Map(), dirty: new Set(), vanished: new Set(),
				queryUpserted: new Set(), queryRemoved: new Set(), allDirty: true,
			});
			expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["v2"]);
			expect((await backend.fetchQuery({ filters: [], limit: 10 })).records.map((r: { name: string }) => r.name)).toEqual(["v2"]);
			// Warm boot: the memo is keyed on the checkpoint hash as well as the change count.
			Object.assign(internals, { states: new Map(), allDirty: true });
			expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["v2"]);
		} finally {
			Object.assign(internals, saved);
			resetCore();
		}
	});
});
