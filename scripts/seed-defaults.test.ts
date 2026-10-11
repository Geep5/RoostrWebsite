/**
 * The bundled catalog (bootstrap_space_defaults) converges only once this
 * replica holds what the relays hold. Rehearsal finding: a fresh or
 * half-synced web replica seeded at startup, before its first history pass,
 * and re-wrote and re-published the vault's whole bundled catalog on every
 * load (453 relay events over 3 loads). Offline it still seeds at once.
 */
import { afterEach, beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, matchFilter, nip44, SimplePool, type Event, type Filter } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils.js";
import { coreCall, initCore, resetCore } from "../src/lib/engine/core";
import { backend } from "../src/lib/engine/backend";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { bytesToBase64, changeId, encodeChange } from "../src/lib/engine/proto";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

const sk = new Uint8Array(32).fill(7);
const pk = getPublicKey(sk);
const conversationKey = bytesToHex(nip44.getConversationKey(sk, pk));
const secret = bytesToHex(sk);

/** The space this vault holds; its bundled catalog is what seeding writes. */
const channel: ChangeJSON = { id: "", objectId: "personal", parentIds: [], timestamp: 100, author: "seed-test", ops: [{ objectCreate: { typeKey: "channel" } }, { fieldSet: { key: "name", value: { stringValue: "Personal" } } }] };
beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
	channel.id = changeId(channel);
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => {
	for (const close of cleanup.splice(0).reverse()) await close();
});

async function storeFixture(): Promise<ChangeStore> {
	const name = `seed-defaults-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => {
		store.close();
		await destroyDatabase(name);
	});
	return store;
}

interface Internals {
	store: ChangeStore;
	sync: { stop(): void; backfillChain?: Promise<boolean> } | null;
	states: Map<string, ObjectJSON>;
	dirty: Set<string>;
	vanished: Set<string>;
	queryUpserted: Set<string>;
	queryRemoved: Set<string>;
	allDirty: boolean;
	author: string;
	pk: string;
	started: boolean;
	seeded: boolean;
	mutationTail: Promise<void>;
}
const internals = backend as unknown as Internals;

/**
 * Points the backend singleton at `store` with nothing loaded; restored (and
 * stopped) afterwards. Resolves once its bootstrap_space_defaults has run.
 */
function useReplica(store: ChangeStore): Promise<unknown> {
	const saved: Internals = { ...internals };
	resetCore();
	Object.assign(internals, {
		store, sync: null, states: new Map(), dirty: new Set(), vanished: new Set(), queryUpserted: new Set(), queryRemoved: new Set(),
		allDirty: true, author: "seed-test", pk, started: false, seeded: false,
	});
	const seeding = Promise.withResolvers<unknown>();
	const mutate = backend.mutate.bind(backend);
	backend.mutate = (action, params, ownerSignal) => {
		const run = mutate(action, params, ownerSignal);
		if (action === "bootstrap_space_defaults") seeding.resolve(run);
		return run;
	};
	cleanup.push(async () => {
		Reflect.deleteProperty(backend, "mutate");
		const sync = internals.sync;
		backend.stop();
		await sync?.backfillChain;
		await internals.mutationTail;
		Object.assign(internals, saved);
		resetCore();
	});
	return seeding.promise;
}

class MemoryStorage implements Storage {
	private values = new Map<string, string>();
	get length(): number { return this.values.size; }
	clear(): void { this.values.clear(); }
	getItem(key: string): string | null { return this.values.get(key) ?? null; }
	key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
	removeItem(key: string): void { this.values.delete(key); }
	setItem(key: string, value: string): void { this.values.set(key, String(value)); }
}

/** This identity signed in with `relays` configured. */
function signedIn(relays: string[]): void {
	const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
	const storage = new MemoryStorage();
	storage.setItem("roostr-key", secret);
	storage.setItem("roostr-relays", JSON.stringify(relays));
	Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
	cleanup.push(() => {
		if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
		else Reflect.deleteProperty(globalThis, "localStorage");
	});
}

/** Every SimplePool talks to `relay` (or fails to connect); returns what was published. */
function network(relay: unknown): Event[] {
	const published: Event[] = [];
	const proto = SimplePool.prototype as unknown as Record<string, unknown>;
	const saved = Object.fromEntries(["ensureRelay", "querySync", "subscribeMany", "publish", "close"].map((k) => [k, proto[k]]));
	Object.assign(proto, {
		ensureRelay: async () => {
			if (!relay) throw new Error("connection refused");
			return relay;
		},
		querySync: async () => [],
		subscribeMany: () => ({ close() {} }),
		publish: (_relays: string[], event: Event) => {
			published.push(event);
			return [Promise.resolve("ok")];
		},
		close: () => {},
	});
	cleanup.push(() => void Object.assign(proto, saved));
	return published;
}

/** In-process relay answering REQ with the matching events (NIP-77 refused: the paged walk). */
function fakeRelay(events: Event[]) {
	return {
		onnotice: (_notice: string) => {},
		prepareSubscription(filters: Filter[], params: { onevent?: (e: Event) => void; oneose?: () => void; onclose?: (reason: string) => void }) {
			return {
				fire() {
					queueMicrotask(() => {
						for (const e of events) if (matchFilter(filters[0], e)) params.onevent?.(e);
						params.oneose?.();
					});
				},
				receivedEose() {},
				close() {},
			};
		},
		async send(text: string) {
			const msg = JSON.parse(text) as [string, string, ...unknown[]];
			if (msg[0] === "NEG-OPEN") queueMicrotask(() => this.onnotice("ERROR: negentropy disabled"));
		},
	};
}

/** The vault on the relays: a space another device seeded, as personal 1078s. */
async function seededVault(): Promise<Event[]> {
	const store = await storeFixture();
	await store.addChanges([{ change: channel, bytes: encodeChange(channel) }]);
	const saved: Internals = { ...internals };
	resetCore();
	Object.assign(internals, { store, sync: null, states: new Map(), dirty: new Set(), vanished: new Set(), queryUpserted: new Set(), queryRemoved: new Set(), allDirty: true, author: "seed-test", pk });
	try {
		await backend.mutate("bootstrap_space_defaults", {});
	} finally {
		Object.assign(internals, saved);
		resetCore();
	}
	const events: Event[] = [];
	let at = 1_700_000_000;
	for (const id of await store.objectIds()) {
		for (const { bytes, change } of await store.rawChangesFor(id)) {
			const sealed = coreCall<{ parts: Array<{ content: string; tags: string[][] }> }>("wire", { action: "seal", change: bytesToBase64(bytes), conversationKey, secret, objectId: change.objectId });
			for (const part of sealed.parts) events.push(finalizeEvent({ kind: 1078, created_at: at++, tags: part.tags, content: part.content }, sk));
		}
	}
	return events;
}

/** A half-synced replica: the space arrived, its bundled catalog not yet (an interrupted first load). */
async function halfSynced(): Promise<ChangeStore> {
	const store = await storeFixture();
	await store.addChanges([{ change: channel, bytes: encodeChange(channel) }]);
	return store;
}

test("a fresh replica that syncs existing defaults publishes zero seeding changes", async () => {
	const vault = await seededVault();
	expect(vault.length).toBeGreaterThan(20);
	const store = await halfSynced();
	signedIn(["wss://fake.test"]);
	const published = network(fakeRelay(vault));
	const seeding = useReplica(store);
	await backend.start();
	await seeding;
	expect(await store.pendingPublishes()).toEqual([]);
	expect(published.filter((e) => e.kind === 1078)).toEqual([]);
	// The catalog is there - synced, not re-written.
	expect((await backend.fetchRelations()).length).toBeGreaterThan(0);
});

test("offline (no relay reachable) the first run still seeds", async () => {
	const store = await halfSynced();
	signedIn(["wss://fake.test"]);
	network(null);
	const seeding = useReplica(store);
	await backend.start();
	await seeding;
	expect((await store.pendingPublishes()).length).toBeGreaterThan(20);
	expect((await backend.fetchRelations()).length).toBeGreaterThan(0);
});

test("with no relay configured the first run still seeds", async () => {
	const store = await halfSynced();
	signedIn([]);
	network(null);
	const seeding = useReplica(store);
	await backend.start();
	await seeding;
	expect((await store.pendingPublishes()).length).toBeGreaterThan(20);
});
