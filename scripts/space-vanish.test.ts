/**
 * Deleting a space removes everything in it. The owner's delete vanishes the
 * space and every object naming it - objects this replica only receives
 * after the space went included - and the owner's kind-5 h-deletion is the
 * signal members' devices vanish it on. A member can only leave: the space
 * goes for that identity alone, and only its owner deletes it for everyone.
 */
import { afterEach, beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, type Event } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils.js";
import { initCore, resetCore } from "../src/lib/engine/core";
import { backend } from "../src/lib/engine/backend";
import { blindShared, RelaySync } from "../src/lib/engine/sync";
import { spaceKeyGet } from "../src/lib/engine/spacekeys";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { changeId, encodeChange } from "../src/lib/engine/proto";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON, ValueJSON } from "../src/lib/types";

const ownerSk = new Uint8Array(32).fill(1), memberSk = new Uint8Array(32).fill(2);
const owner = getPublicKey(ownerSk), member = getPublicKey(memberSk);
const keyHex = bytesToHex(new Uint8Array(32).fill(4));

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => {
	for (const close of cleanup.splice(0).reverse()) await close();
});

async function storeFixture(): Promise<ChangeStore> {
	const name = `space-vanish-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => {
		store.close();
		await destroyDatabase(name);
	});
	return store;
}

// ── The owner's h-deletion ───────────────────────────────────────────

function deletion(sk: Uint8Array): Event {
	return finalizeEvent({ kind: 5, created_at: 100, content: "space deleted", tags: [["h", blindShared(keyHex, "space:space")], ["k", "1078"], ["k", "1079"]] }, sk);
}

/** Feeds `event` through the private ingest seam of a device signed in as `localSk`; returns the spaces it reported vanished. */
async function signalled(localSk: Uint8Array, spaceOwner: string | undefined, event: Event): Promise<string[]> {
	const store = await storeFixture();
	const vanished: string[] = [];
	const sync = new RelaySync(localSk, [], store, { onObjects() {}, onStatus() {} }, { onSpaceVanished: (id) => vanished.push(id) });
	cleanup.push(() => sync.stop());
	sync.setSharedSpaces([{ spaceId: "space", keyHex, keyId: 1, owner: spaceOwner, writers: [owner, member] }]);
	const internals = sync as unknown as { ingestEvent(event: Event): Promise<unknown> };
	expect(await internals.ingestEvent(event)).toBeNull();
	return vanished;
}

test("only the space owner's h-deletion vanishes a shared space", async () => {
	expect(await signalled(memberSk, owner, deletion(memberSk))).toEqual([]);
	expect(await signalled(memberSk, owner, deletion(ownerSk))).toEqual(["space"]);
	// No imported owner: the space is this identity's own.
	expect(await signalled(ownerSk, undefined, deletion(memberSk))).toEqual([]);
	expect(await signalled(ownerSk, undefined, deletion(ownerSk))).toEqual(["space"]);
});

// ── The browser replica ──────────────────────────────────────────────

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

/** Points the backend singleton, signed in as `member`, at a fresh store holding `changes`; restored afterwards. */
async function replicaFixture(changes: ChangeJSON[]): Promise<ChangeStore> {
	const store = await storeFixture();
	await store.addChanges(changes.map((change) => ({ change, bytes: encodeChange(change) })));
	const saved: Internals = { ...internals };
	resetCore();
	Object.assign(internals, {
		store, states: new Map(), dirty: new Set(), vanished: new Set(), queryUpserted: new Set(), queryRemoved: new Set(),
		allDirty: true, author: "space-vanish-test", pk: member,
	});
	cleanup.push(() => {
		Object.assign(internals, saved);
		resetCore();
	});
	return store;
}

let clock = 100;
function created(objectId: string, typeKey: string, fields: Record<string, ValueJSON>): ChangeJSON {
	const change: ChangeJSON = {
		id: "", objectId, parentIds: [], timestamp: clock++, author: "space-vanish-test",
		ops: [{ objectCreate: { typeKey } }, ...Object.entries(fields).map(([key, value]) => ({ fieldSet: { key, value } }))],
	};
	change.id = changeId(change);
	return change;
}
const spaceObject = (id: string) => created(id, "channel", { name: { stringValue: id } });
const note = (id: string, name: string, space: string) => created(id, "note", { name: { stringValue: name }, channel: { stringValue: space } });

class MemoryStorage implements Storage {
	private values = new Map<string, string>();
	get length(): number { return this.values.size; }
	clear(): void { this.values.clear(); }
	getItem(key: string): string | null { return this.values.get(key) ?? null; }
	key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
	removeItem(key: string): void { this.values.delete(key); }
	setItem(key: string, value: string): void { this.values.set(key, String(value)); }
}

/** A browser keyring (spacekeys.ts) holding a key per space; `owner` marks an imported entry. */
function keyringFixture(owners: Record<string, string | undefined>): void {
	const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
	const storage = new MemoryStorage();
	const channels = Object.fromEntries(Object.entries(owners).map(([id, entryOwner]) => [id, { key: keyHex, keyId: 1, createdAt: 1, ...(entryOwner ? { owner: entryOwner } : {}) }]));
	storage.setItem("roostr-space-keys", JSON.stringify({ version: 1, channels }));
	Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
	cleanup.push(() => {
		if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
		else Reflect.deleteProperty(globalThis, "localStorage");
	});
}

test("a vanished space takes every object in it, including ones that arrive after it went", async () => {
	const store = await replicaFixture([
		spaceObject("gone"), note("before", "Before", "gone"),
		spaceObject("kept"), note("survivor", "Survivor", "kept"),
		created("__vanished__", "vanish_log", { "vanished:gone": { intValue: 100 } }),
	]);
	expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["Survivor"]);
	expect((await backend.fetchChannels()).map((c) => c.id)).toEqual(["kept"]);
	// A relay copy written into the space by a device that never heard it
	// went: it lands in a steady-state replay batch, not a full sweep.
	const late = note("late", "Late", "gone");
	await store.addChanges([{ change: late, bytes: encodeChange(late) }]);
	internals.dirty.add("late");
	expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["Survivor"]);
	const queried = (await backend.fetchQuery({ filters: [], limit: 500 })).records.map((r: { id: string }) => r.id);
	expect(queried).toContain("survivor");
	expect(queried).not.toContain("before");
	expect(queried).not.toContain("late");
	// Held but gone: the digest covers kept, survivor and the ledger only.
	expect((await backend.syncDigest()).objects).toBe(3);
});

test("spaces report their owner; a member can leave, only the owner deletes for everyone", async () => {
	keyringFixture({ mine: undefined, echo: member, joined: owner });
	await replicaFixture([spaceObject("mine"), spaceObject("echo"), spaceObject("joined"), note("shared", "Shared note", "joined"), spaceObject("local")]);
	// An imported entry naming this identity, or no keyring entry at all, is this identity's own space.
	const owners = Object.fromEntries((await backend.fetchChannels()).map((c) => [c.id, c.owner]));
	expect(owners).toEqual({ mine: "", echo: "", joined: owner, local: "" });

	await expect(backend.mutate("vanish", { object_ids: ["joined", "shared"] })).rejects.toThrow("Only the space's owner can delete it for everyone");
	for (const channel_id of ["mine", "echo", "local"]) {
		await expect(backend.mutate("space_leave", { channel_id })).rejects.toThrow("You own this space");
	}
	expect((await backend.fetchObjects()).map((o) => o.name)).toEqual(["Shared note"]);

	expect(await backend.mutate("space_leave", { channel_id: "joined" })).toEqual({ ok: true, left: "joined" });
	expect(spaceKeyGet("joined")).toBeNull();
	expect(spaceKeyGet("mine")).not.toBeNull();
	expect((await backend.fetchChannels()).map((c) => c.id)).toEqual(["mine", "echo", "local"]);
	expect(await backend.fetchObjects()).toEqual([]);
});

test("the owner's signal is the one path that vanishes a space someone else owns", async () => {
	keyringFixture({ joined: owner });
	await replicaFixture([spaceObject("joined"), note("shared", "Shared note", "joined")]);
	await backend.mutate("vanish", { object_ids: ["joined"] }, true);
	expect(await backend.fetchChannels()).toEqual([]);
	expect(await backend.fetchObjects()).toEqual([]);
	// Unlike leaving, the key stays: it addresses this identity's own h-deletion.
	expect(spaceKeyGet("joined")).not.toBeNull();
});
