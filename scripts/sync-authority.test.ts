import { afterEach, beforeAll, describe, expect, test, spyOn } from "bun:test";
import { readFileSync } from "node:fs";
import { initCore } from "../src/lib/engine/core";
import { finalizeEvent, getPublicKey, nip19, nip44, type Event } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import { authorizeSharedChange, blindShared, RelaySync, type SharedSpaceInfo } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { changeId, decodeChange, encodeChange } from "../src/lib/engine/proto";
import { computeObject } from "../src/lib/engine/replay";
import type { ChangeJSON, PendingPublish, SharedProvenance } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

const ownerSk = new Uint8Array(32).fill(1), memberSk = new Uint8Array(32).fill(2), strangerSk = new Uint8Array(32).fill(3);
const owner = getPublicKey(ownerSk), member = getPublicKey(memberSk);
const spaceKey = new Uint8Array(32).fill(4);
const space: SharedSpaceInfo = { spaceId: "space", keyId: 1, keyHex: bytesToHex(spaceKey), owner, writers: [owner, member] };
const provenance: SharedProvenance = { spaceId: "space", keyId: 1, signer: member };
function change(objectId: string, ops: ChangeJSON["ops"], snapshot?: unknown): ChangeJSON {
	const value: ChangeJSON = { id: "", objectId, ops, parentIds: [], timestamp: 100, author: owner, ...(snapshot ? { snapshot } : {}) };
	value.id = changeId(value);
	return value;
}
let createSpace: ChangeJSON;
let trustedSpace: ObjectJSON;
let createDoc: ChangeJSON;
let doc: ObjectJSON;
let edit: ChangeJSON;
beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
	createSpace = change("space", [{ objectCreate: { typeKey: "channel" } }, { fieldSet: { key: "members", value: { valuesValue: { items: [{ mapValue: { entries: { npub: { stringValue: nip19.npubEncode(member) }, role: { stringValue: "writer" } } } }] } } } }]);
	trustedSpace = computeObject([createSpace])!;
	createDoc = change("doc", [{ objectCreate: { typeKey: "note" } }, { fieldSet: { key: "channel", value: { stringValue: "space" } } }]);
	doc = computeObject([createDoc])!;
	edit = change("doc", [{ fieldSet: { key: "name", value: { stringValue: "safe" } } }]);
});
const permits = (candidate: ChangeJSON, existing: ObjectJSON | null = doc, p = provenance) => authorizeSharedChange(candidate, p, space, owner, trustedSpace, existing);

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });
async function storeFixture() {
	const name = `authority-test-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => { store.close(); await destroyDatabase(name); });
	return store;
}
interface Received { bytes: Uint8Array; change: ChangeJSON; provenance?: SharedProvenance }
interface SyncInternals {
	eventToChange(event: Event): Promise<Received | null>;
	importBatch(batch: Received[]): Promise<void>;
	handleLiveEvent(event: Event): Promise<void>;
	backfill(since: number): Promise<boolean>;
	watchdog(): Promise<void>;
	cursor: number;
	historyComplete: boolean;
	chunkGroups: Map<string, unknown>;
	queryRelayPage(url: string, filter: Record<string, unknown>): Promise<Event[]>;
	publishOnce(item: { objectId: string; changeId: string; b64: string; attempts: number; notBefore: number; pending: PendingPublish }): Promise<boolean>;
	pool: { publish(relays: string[], event: Event): Promise<string>[]; querySync(): Promise<Event[]>; subscribeMany(): { close(): void }; close(): void };
}
function syncFixture(store: ChangeStore, relays: string[] = []) {
	const sync = new RelaySync(ownerSk, relays, store, { onObjects() {}, onStatus() {} });
	sync.setSharedSpaces([space]);
	cleanup.push(() => sync.stop());
	return { sync, internals: sync as unknown as SyncInternals };
}
const base64 = (c: ChangeJSON) => Buffer.from(encodeChange(c)).toString("base64");
function event(part: string, sk = memberSk, tags: string[][] = [], key = spaceKey): Event {
	return finalizeEvent({ kind: 1078, created_at: 100, content: nip44.encrypt(part, key), tags: [["h", blindShared(bytesToHex(key), "space:space")], ...tags] }, sk);
}
function chunks(c = createDoc) {
	const full = base64(c), cut = Math.floor(full.length / 2);
	const gid = bytesToHex(sha256(new TextEncoder().encode(full))).slice(0, 16);
	return { parts: [full.slice(0, cut), full.slice(cut)], gid };
}

describe("shared authority", () => {
	test("uses transport signer and existing membership, never the inner author", () => {
		expect(permits(edit)).toBe(true);
		expect(permits(edit, doc, { ...provenance, signer: getPublicKey(strangerSk) })).toBe(false);
		expect(permits(edit, doc, { ...provenance, keyId: 2 })).toBe(false);
		const viewer = structuredClone(trustedSpace);
		viewer.fields.members.valuesValue!.items[0].mapValue!.entries.role.stringValue = "viewer";
		expect(authorizeSharedChange(edit, provenance, space, owner, viewer, doc)).toBe(false);
	});
	test("cannot steal existing personal or foreign objects with a new channel stamp", () => {
		for (const scope of ["", "foreign"]) {
			const target = { ...doc, fields: { channel: { stringValue: scope } } };
			expect(permits(createDoc, target)).toBe(false);
			expect(permits(createDoc, target, { ...provenance, signer: owner })).toBe(false);
		}
		expect(permits(createDoc, null)).toBe(true);
		expect(permits(edit, null)).toBe(false);
	});
	test("protects administrative fields, control types and channel deletion", () => {
		for (const key of ["members", "owner", "keyId", "key", "keys", "served_by", "machine", "machine_id", "machineId", "bound_object"]) {
			expect(permits(change("doc", [{ fieldSet: { key, value: { stringValue: "attack" } } }]))).toBe(false);
			expect(permits(change("doc", [{ fieldDelete: { key } }]))).toBe(false);
		}
		for (const typeKey of ["machine", "agent", "program", "typescript", "skill", "peer"]) {
			expect(permits(edit, { ...doc, typeKey })).toBe(false);
			expect(permits(edit, { ...doc, typeKey }, { ...provenance, signer: owner })).toBe(true);
		}
		expect(permits(change("space", [{ objectDelete: {} }]), trustedSpace)).toBe(false);
		expect(permits(change("__vanished__", createDoc.ops), null, { ...provenance, signer: owner })).toBe(false);
	});
	test("snapshots cannot hide privilege changes, omit protected fields, change type or scope", () => {
		const snapshot = { id: "doc", typeKey: "note", fields: { channel: { stringValue: "space" } }, blocks: [], deleted: false };
		expect(permits(change("doc", [], snapshot))).toBe(true);
		for (const mutation of [{ id: "other" }, { typeKey: "agent" }, { fields: {} }, { fields: { ...snapshot.fields, members: { stringValue: "attack" } } }]) {
			expect(permits(change("doc", [], { ...snapshot, ...mutation }))).toBe(false);
		}
		expect(permits(change("doc", [], snapshot), { ...doc, fields: { ...doc.fields, served_by: { stringValue: owner } } })).toBe(false);
		expect(permits(change("doc", [], { ...snapshot, fields: { channel: { stringValue: "foreign" } } }), doc, { ...provenance, signer: owner })).toBe(false);
	});
	test("membership removal takes effect before the next item in the same batch", async () => {
		const store = await storeFixture();
		await store.addChanges([createSpace, createDoc].map((c) => ({ change: c, bytes: encodeChange(c) })));
		const { internals } = syncFixture(store);
		const removal = change("space", [{ fieldSet: { key: "members", value: { valuesValue: { items: [] } } } }]);
		removal.parentIds = [createSpace.id]; removal.id = changeId(removal);
		await internals.importBatch([
			{ change: removal, bytes: encodeChange(removal), provenance: { ...provenance, signer: owner } },
			{ change: edit, bytes: encodeChange(edit), provenance },
		]);
		expect((await store.changesFor("space")).length).toBe(2);
		expect((await store.changesFor("doc")).length).toBe(1);
		expect(await store.isPublished(`space/1/${edit.id}`)).toBe(false);
	});
});

describe("encrypted chunk provenance", () => {
	test("imports and retrieves legacy explicit-default protobuf bytes without changing their content address", async () => {
		const canonical = encodeChange({ ...createDoc, author: "" });
		const legacy = new Uint8Array(canonical.length + 2);
		legacy.set(canonical);
		legacy.set([0x32, 0x00], canonical.length); // Explicit empty author (field 6).
		const digest = sha256.create().update(new Uint8Array([0x0a, 0x00])).update(legacy.subarray(34)).digest();
		legacy.set(digest, 2);
		const decoded = decodeChange(legacy)!;
		expect(decoded.author).toBe("");
		expect(decoded.id).toBe(bytesToHex(digest));
		expect(changeId(decoded)).not.toBe(decoded.id);
		expect(encodeChange(decoded)).not.toEqual(legacy);
		const store = await storeFixture();
		const { internals } = syncFixture(store);
		await internals.handleLiveEvent(event(Buffer.from(legacy).toString("base64"), ownerSk));
		store.close(); await store.open();
		expect(await store.rawChangesFor("doc")).toEqual([{ bytes: legacy, change: decoded }]);
		expect(await store.rawChangesFor("absent")).toEqual([]);
		const tampered = legacy.slice();
		tampered[2] ^= 1;
		expect(await internals.eventToChange(event(Buffer.from(tampered).toString("base64"), ownerSk))).toBeNull();
	});
	test("requires valid outer signatures and own signer for personal encryption", async () => {
		const { internals } = syncFixture(await storeFixture());
		const personalKey = nip44.getConversationKey(ownerSk, owner);
		expect(await internals.eventToChange(event(base64(createDoc), strangerSk, [], personalKey))).toBeNull();
		const valid = event(base64(createDoc));
		expect((await internals.eventToChange(valid))?.provenance).toEqual(provenance);
		expect(await internals.eventToChange(JSON.parse(JSON.stringify({ ...valid, content: valid.content + "x" })) as Event)).toBeNull();
	});
	test("never assembles across signers, space IDs or key versions", async () => {
		const { sync, internals } = syncFixture(await storeFixture());
		const { parts, gid } = chunks();
		const tags = (i: number) => [["c", gid, String(i), "2"]];
		expect(await internals.eventToChange(event(parts[0], memberSk, tags(0)))).toBeNull();
		expect(await internals.eventToChange(event(parts[1], ownerSk, tags(1)))).toBeNull();
		sync.setSharedSpaces([{ ...space, keyId: 2 }]);
		expect(await internals.eventToChange(event(parts[1], memberSk, tags(1)))).toBeNull();
		sync.setSharedSpaces([{ ...space, spaceId: "other" }]);
		expect(await internals.eventToChange(event(parts[1], memberSk, tags(1)))).toBeNull();
		sync.setSharedSpaces([space]);
		expect((await internals.eventToChange(event(parts[1], memberSk, tags(1))))?.change.id).toBe(createDoc.id);
	});
	test("rejects malformed indices, inconsistent totals and expired partial groups", async () => {
		const { internals } = syncFixture(await storeFixture());
		const { parts, gid } = chunks();
		for (const index of ["-1", "2", "1x", "0.5", ""]) expect(await internals.eventToChange(event(parts[0], memberSk, [["c", gid, index, "2"]]))).toBeNull();
		expect(await internals.eventToChange(event(parts[0], memberSk, [["c", gid, "0", "2"]]))).toBeNull();
		expect(await internals.eventToChange(event(parts[1], memberSk, [["c", gid, "1", "3"]]))).toBeNull();
		expect(await internals.eventToChange(event(parts[1], memberSk, [["c", gid, "1", "2"]]))).toBeNull();
		const clock = spyOn(Date, "now").mockReturnValue(Date.now() + 300_001);
		try {
			expect(await internals.eventToChange(event(parts[0], memberSk, [["c", gid, "0", "2"]]))).toBeNull();
			expect((await internals.eventToChange(event(parts[1], memberSk, [["c", gid, "1", "2"]])))?.change.id).toBe(createDoc.id);
		} finally { clock.mockRestore(); }
	});
	test("caps active groups and preserves the cursor floor after expiration", async () => {
		const store = await storeFixture();
		const { internals } = syncFixture(store);
		await store.setCursor(200);
		for (let i = 0; i < 129; i++) await internals.handleLiveEvent(event("YQ", memberSk, [["c", i.toString(16).padStart(16, "0"), "0", "2"]]));
		expect(internals.chunkGroups.size).toBe(128);
		expect(await store.getCursor()).toBe(99);
		expect((await store.getReplayGroups()).length).toBe(129);
		const now = Date.now();
		const clock = spyOn(Date, "now").mockReturnValue(now + 300_001);
		try {
			await internals.handleLiveEvent(event("YQ", memberSk, [["c", "ffffffffffffffff", "0", "2"]]));
			expect(internals.chunkGroups.size).toBe(1);
			store.close(); await store.open();
			expect(await store.getCursor()).toBe(99);
			expect((await store.getReplayGroups()).length).toBe(130);
		} finally { clock.mockRestore(); }
	});
});

describe("history completion", () => {
	test("an empty real EOSE succeeds but a closed unavailable relay does not", async () => {
		const { internals } = syncFixture(await storeFixture());
		let closed = false;
		Object.assign(internals.pool, {
			ensureRelay: async () => ({
				prepareSubscription: (_filters: unknown, callbacks: { oneose(): void; onclose(reason: string): void }) => ({
					oneose: callbacks.oneose,
					fire() { if (closed) callbacks.onclose("unavailable"); else callbacks.oneose(); },
					receivedEose() {},
					close() {},
				}),
			}),
		});
		expect(await internals.queryRelayPage("wss://invalid.test", { kinds: [1078] })).toEqual([]);
		closed = true;
		await expect(internals.queryRelayPage("wss://invalid.test", { kinds: [1078] })).rejects.toThrow("unavailable");
	});
	test("complete chunk repair releases the durable floor and watchdog returns to head checks", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		const history = parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]]));
		const pages = spyOn(internals, "queryRelayPage").mockImplementation(async (_relay, filter) => filter["#h"] ? history.slice(0, 1) : []);
		expect(await internals.backfill(100)).toBe(false);
		store.close(); await store.open();
		expect(await store.getCursor()).toBe(99);
		pages.mockImplementation(async (_relay, filter) => filter["#h"] ? history : []);
		expect(await internals.backfill(201)).toBe(true);
		expect(await store.getCursor()).toBe(200);
		expect((await store.changesFor("doc")).length).toBe(1);
		for (const [, filter] of pages.mock.calls) expect(filter.limit).toBe(128);
		const pageCalls = pages.mock.calls.length;
		const heads = spyOn(internals.pool, "querySync").mockResolvedValue([]);
		await internals.watchdog();
		expect(pages.mock.calls.length).toBe(pageCalls);
		expect(heads).toHaveBeenCalledTimes(2);
	});
	test("ordinary live chunk success restores the cursor without requiring another history scan", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store);
		internals.cursor = 200;
		internals.historyComplete = true;
		const { parts, gid } = chunks();
		await internals.handleLiveEvent(event(parts[0], ownerSk, [["c", gid, "0", "2"]]));
		expect(await store.getCursor()).toBe(99);
		await internals.handleLiveEvent(event(parts[1], ownerSk, [["c", gid, "1", "2"]]));
		expect(await store.getCursor()).toBe(200);
		expect(internals.historyComplete).toBe(true);
		expect((await store.changesFor("doc")).length).toBe(1);
	});
	test("failed relay scans retain discarded groups across reload until a covering repair", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		await internals.handleLiveEvent(event(parts[0], ownerSk, [["c", gid, "0", "2"]]));
		const pages = spyOn(internals, "queryRelayPage").mockRejectedValue(new Error("unavailable"));
		expect(await internals.backfill(201)).toBe(false);
		expect(internals.chunkGroups.size).toBe(1);
		store.close(); await store.open();
		expect(await store.getCursor()).toBe(0);
		const restarted = syncFixture(store, ["wss://invalid.test"]).internals;
		restarted.cursor = await store.getCursor();
		const repair = spyOn(restarted, "queryRelayPage").mockResolvedValue([]);
		expect(await restarted.backfill(restarted.cursor + 1)).toBe(false);
		expect(await store.getCursor()).toBe(0);
		expect((await store.getReplayGroups()).length).toBe(1);
		repair.mockImplementation(async (_relay, filter) => filter["#h"] ? parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]])) : []);
		expect(await restarted.backfill(restarted.cursor + 1)).toBe(true);
		expect(repair.mock.calls.every(([, filter]) => filter.since === 0)).toBe(true);
		expect(await store.getCursor()).toBe(100);
		expect(await store.getReplayGroups()).toEqual([]);
		expect(pages).toHaveBeenCalledTimes(2);
	});
	test("import failure preserves recovery across reload and a later successful scan retires it", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		spyOn(internals, "queryRelayPage").mockImplementation(async (_relay, filter) => filter["#h"] ? parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]])) : []);
		const writes = spyOn(store, "addChanges").mockRejectedValueOnce(new Error("disk full"));
		await expect(internals.backfill(100)).rejects.toThrow("disk full");
		store.close(); await store.open();
		expect(await store.getCursor()).toBe(0);
		expect(internals.historyComplete).toBe(false);
		expect((await store.getReplayGroups()).length).toBe(1);
		writes.mockRestore();
		expect(await internals.backfill(201)).toBe(true);
		expect(await store.getCursor()).toBe(200);
		expect(await store.getReplayGroups()).toEqual([]);
		expect(internals.chunkGroups.size).toBe(0);
	});
	test("a fresh conflict during an otherwise complete scan requires another clean covering pass", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		const pages = spyOn(internals, "queryRelayPage").mockImplementation(async (_relay, filter) => {
			if (filter["#h"]) {
				await internals.handleLiveEvent(event(parts[0], ownerSk, [["c", gid, "0", "2"]]));
				await internals.handleLiveEvent(event(parts[1], ownerSk, [["c", gid, "1", "3"]]));
			}
			return [];
		});
		expect(await internals.backfill(201)).toBe(false);
		expect(internals.chunkGroups.size).toBe(0);
		expect(await store.getCursor()).toBe(99);
		pages.mockResolvedValue([]);
		expect(await internals.backfill(201)).toBe(false);
		pages.mockImplementation(async (_relay, filter) => filter["#h"] ? parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]])) : []);
		expect(await internals.backfill(201)).toBe(true);
		expect(pages.mock.calls.slice(-2).every(([, filter]) => filter.since === 0)).toBe(true);
		expect(await store.getCursor()).toBe(200);
	});
	test("a concurrent live import failure cannot be cleared by the scan awaiting it", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const entered = Promise.withResolvers<void>();
		const release = Promise.withResolvers<void>();
		const queried = Promise.withResolvers<void>();
		const writes = spyOn(store, "addChanges").mockImplementation(async () => {
			entered.resolve();
			await release.promise;
			throw new Error("disk full");
		});
		const live = internals.handleLiveEvent(event(base64(createDoc), ownerSk));
		await entered.promise;
		const pages = spyOn(internals, "queryRelayPage").mockImplementation(async () => { queried.resolve(); return []; });
		const scan = internals.backfill(100);
		await queried.promise;
		release.resolve();
		await expect(live).rejects.toThrow("disk full");
		expect(await scan).toBe(false);
		expect(await store.getCursor()).toBe(0);
		expect(internals.historyComplete).toBe(false);
		writes.mockRestore();
		pages.mockImplementation(async (_relay, filter) => filter["#h"] ? [event(base64(createDoc), ownerSk)] : []);
		expect(await internals.backfill(201)).toBe(true);
		expect(await store.getCursor()).toBe(200);
		expect((await store.changesFor("doc")).length).toBe(1);
	});
	test("a suffix-first repair and later duplicate suffixes do not recreate partial groups", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		const history = parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]]));
		await internals.handleLiveEvent(history[1]);
		const pages = spyOn(internals, "queryRelayPage").mockImplementation(async (_relay, filter) => filter["#h"] ? history : []);
		expect(await internals.backfill(201)).toBe(true);
		expect(await store.getCursor()).toBe(200);
		expect(internals.chunkGroups.size).toBe(0);
		expect(await store.getReplayGroups()).toEqual([]);
		await internals.handleLiveEvent(history[1]);
		expect(internals.chunkGroups.size).toBe(0);
		expect(pages.mock.calls.every(([, filter]) => filter.since === 0)).toBe(true);
		expect(await store.getCursor()).toBe(200);
	});
	test("a chunk arriving during the repair cursor commit keeps recovery pending", async () => {
		const store = await storeFixture();
		await store.setCursor(200);
		const { internals } = syncFixture(store, ["wss://invalid.test"]);
		internals.cursor = 200;
		const { parts, gid } = chunks();
		const history = parts.map((part, i) => event(part, ownerSk, [["c", gid, String(i), "2"]]));
		const pages = spyOn(internals, "queryRelayPage").mockResolvedValue([]);
		const setCursor = store.setCursor.bind(store);
		const commits = spyOn(store, "setCursor").mockImplementation(async (value, groups) => {
			if (value === 200) await internals.eventToChange(history[0]);
			await setCursor(value, groups);
		});
		expect(await internals.backfill(100)).toBe(false);
		expect(await store.getCursor()).toBe(99);
		expect(internals.chunkGroups.size).toBe(1);
		expect((await store.getReplayGroups()).length).toBe(1);
		commits.mockRestore();
		pages.mockImplementation(async (_relay, filter) => filter["#h"] ? history : []);
		expect(await internals.backfill(201)).toBe(true);
		expect(await store.getCursor()).toBe(200);
	});
});

describe("durable personal outbox", () => {
	test("offline save and exact signed ciphertext survive close and reopen until acknowledgement", async () => {
		const store = await storeFixture();
		await store.addLocalChange(encodeChange(createDoc), createDoc);
		const { internals } = syncFixture(store);
		const attempts: Event[] = [];
		let offline = true;
		internals.pool = { publish: (_, e) => { attempts.push(e); return [offline ? Promise.reject(new Error("offline")) : Promise.resolve("ok")]; }, querySync: async () => [], subscribeMany: () => ({ close() {} }), close() {} };
		const pending = (await store.getPending(createDoc.id))!;
		const item = { objectId: "doc", changeId: createDoc.id, b64: base64(createDoc), attempts: 0, notBefore: 0, pending };
		expect(await internals.publishOnce(item)).toBe(false);
		store.close(); await store.open();
		expect((await store.changesFor("doc"))[0].id).toBe(createDoc.id);
		const restored = (await store.getPending(createDoc.id))!;
		expect(JSON.stringify(restored.events?.[0])).toBe(JSON.stringify(attempts[0]));
		offline = false;
		expect(await internals.publishOnce({ ...item, pending: restored })).toBe(true);
		expect(JSON.stringify(attempts[1])).toBe(JSON.stringify(attempts[0]));
		expect(attempts[0].tags.some((tag) => tag[0] === "expiration")).toBe(false);
		await store.markPublished(createDoc.id);
		expect(await store.pendingPublishes()).toEqual([]);
	});
	test("boot restores unpublished personal changes without depending on any shared space", async () => {
		const store = await storeFixture();
		await store.addLocalChange(encodeChange(createDoc), createDoc);
		store.close(); await store.open();
		const { sync, internals } = syncFixture(store);
		const sent = Promise.withResolvers<Event>();
		internals.pool = { publish: (_, e) => { sent.resolve(e); return [Promise.resolve("ok")]; }, querySync: async () => [], subscribeMany: () => ({ close() {} }), close() {} };
		await sync.start();
		expect((await sent.promise).pubkey).toBe(owner);
		sync.stop();
	});
});
