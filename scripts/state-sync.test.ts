/**
 * State sync on the web host (glonOdin/docs/state-sync.md): bases first,
 * live deltas by `#b`, a bounded window for the rest, orphans rebased and
 * published with their `b`, and object state replayed from the current base.
 * Bases are made the way a computer makes them: core `compact_plan` + `seal_base`.
 */
import { afterEach, beforeAll, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, matchFilter, nip44, type Event, type Filter } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils.js";
import { coreCall, initCore, resetCore } from "../src/lib/engine/core";
import { packCoreValueMaps } from "../src/lib/engine/core-values";
import { RelaySync } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { base64ToBytes, bytesToBase64, changeId, encodeChange } from "../src/lib/engine/proto";
import { classifyObject, pickCurrentBase, replayObject } from "../src/lib/engine/replay";
import { Negentropy, NegentropyStorage } from "../src/lib/engine/negentropy";
import { backend } from "../src/lib/engine/backend";
import type { BaseRow, ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

const sk = new Uint8Array(32).fill(1);
const pk = getPublicKey(sk);
const conversationKey = bytesToHex(nip44.getConversationKey(sk, pk));
const secret = bytesToHex(sk);
/** Base created_at (ms): the bounded window opens a day before it. */
const BASE_MS = 1_760_000_000_000;
const BOUND_S = BASE_MS / 1000 - 86_400;

function change(objectId: string, ops: ChangeJSON["ops"], parentIds: string[], timestamp: number): ChangeJSON {
	const value: ChangeJSON = { id: "", objectId, ops, parentIds, timestamp, author: pk };
	value.id = changeId(value);
	return value;
}
const setName = (value: string): ChangeJSON["ops"][number] => ({ fieldSet: { key: "name", value: { stringValue: value } } });
const b64 = (c: ChangeJSON) => bytesToBase64(encodeChange(c));
const nameOf = (state: ObjectJSON | null) => state?.fields["name"]?.stringValue;

/** What a computer's compaction produces: the next base over `bases` folding `changes`. */
function compact(bases: string[], changes: ChangeJSON[], nowMs = BASE_MS): { base: string; hash: string } {
	const plan = coreCall<{ compact: boolean; base: string; hash: string }>("sync", packCoreValueMaps({ action: "compact_plan", bases, changes, nowMs, force: true }));
	expect(plan.compact).toBe(true);
	return plan;
}
/** Signed relay events of a sealed base: its 1080 parts, then the 31078. */
function baseEvents(base: string, createdAt: number): Event[] {
	const sealed = coreCall<{ event: { kind: number; content: string; tags: string[][] }; parts: Array<{ kind: number; content: string; tags: string[][] }> }>(
		"sync", { action: "seal_base", base, conversationKey, secret });
	return [...sealed.parts, sealed.event].map((p) => finalizeEvent({ kind: p.kind, created_at: createdAt, tags: p.tags, content: p.content }, sk));
}
/** A personal kind-1078 delta as the outbox seals it, `b` when written on a base. */
function deltaEvent(c: ChangeJSON, createdAt: number, base?: string): Event {
	const sealed = coreCall<{ parts: Array<{ content: string; tags: string[][] }> }>("wire", { action: "seal", change: b64(c), conversationKey, secret, objectId: c.objectId, base });
	return finalizeEvent({ kind: 1078, created_at: createdAt, tags: sealed.parts[0].tags, content: sealed.parts[0].content }, sk);
}
function baseRow(base: string): BaseRow {
	const d = coreCall<{ objectId: string; hash: string; epoch: number; prevBase: string; createdAt: number }>("sync", { action: "base_decode", base });
	return { objectId: d.objectId, hash: d.hash, epoch: d.epoch, createdAt: d.createdAt, prevBase: d.prevBase, bytes: base64ToBytes(base) };
}

let c1: ChangeJSON, c2: ChangeJSON, c3: ChangeJSON, c4: ChangeJSON, c5: ChangeJSON, c6: ChangeJSON;
let base1: { base: string; hash: string }, base2: { base: string; hash: string }, base3: { base: string; hash: string };
beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
	c1 = change("doc", [{ objectCreate: { typeKey: "note" } }, setName("v1")], [], 100);
	c2 = change("doc", [setName("v2")], [c1.id], 200);
	// Epoch 1: the computer folds everything it holds.
	base1 = compact([], [c1, c2]);
	// Two deltas on base 1: c4 another device's, c3 this device's, never published.
	c4 = change("doc", [setName("v4")], [c2.id], 400);
	c3 = change("doc", [{ fieldSet: { key: "note", value: { stringValue: "written offline" } } }], [c2.id], 300);
	// The computer saw c4 and a later c5 (whose events it then deleted), never c3.
	c5 = change("doc", [setName("v5")], [c4.id], 500);
	base2 = compact([base1.base], [{ ...c4, b: base1.hash }, { ...c5, b: base1.hash }], BASE_MS + 60_000);
	c6 = change("doc", [setName("v6")], [c5.id], 600);
	base3 = compact([base2.base], [{ ...c6, b: base2.hash }], BASE_MS + 120_000);
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });

async function storeFixture() {
	const name = `state-sync-test-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => { store.close(); await destroyDatabase(name); });
	return store;
}

interface SyncInternals {
	ingestEvent(event: Event): Promise<unknown>;
	importBatch(batch: unknown[]): Promise<void>;
	backfill(since: number): Promise<boolean>;
	subscribeLive(): void;
	pool: unknown;
	backfillChain: Promise<boolean>;
	historyComplete: boolean;
}
function syncFixture(store: ChangeStore, pool: unknown) {
	const sync = new RelaySync(sk, ["wss://fake.test"], store, { onObjects() {}, onStatus() {} });
	const internals = sync as unknown as SyncInternals;
	internals.pool = pool;
	cleanup.push(async () => { sync.stop(); await internals.backfillChain; });
	return { sync, internals };
}
async function importEvents(internals: SyncInternals, events: Event[]): Promise<void> {
	for (const event of events) {
		const item = await internals.ingestEvent(event);
		if (item) await internals.importBatch([item]);
	}
}

describe("classification glue", () => {
	test("a pending id makes this device's own delta on an unknown base an orphan; anyone else's stays stale", () => {
		// Only the newest base held: base 2, between c3's base 1 and base 3, never reached this device -
		// it may have folded c3, so a delta of unknown fate is neither replayed nor rebased.
		const input = { bases: [base64ToBytes(base3.base)], changes: [{ ...c3, b: base1.hash }] };
		expect(classifyObject(input)).toMatchObject({ base: { hash: base3.hash, epoch: 3 }, orphan: [], stale: [c3.id] });
		// This device never published it: no compactor can have seen it.
		expect(classifyObject({ ...input, pending: [c3.id] })).toMatchObject({ orphan: [c3.id], stale: [] });
		expect(nameOf(replayObject(input))).toBe("v6");
		expect(replayObject(input)?.fields["note"]).toBeUndefined();
	});

	test("a new base rebases this device's orphan, marks it, and publishes the rebased change with its b", async () => {
		const store = await storeFixture();
		// The outbox's first send is the rebased change: resolved by the fake pool itself.
		const sent = Promise.withResolvers<Event>();
		const { sync, internals } = syncFixture(store, { publish: (_relays: string[], event: Event) => { sent.resolve(event); return [Promise.resolve("ok")]; }, close() {} });
		await importEvents(internals, baseEvents(base1.base, BASE_MS / 1000));
		expect((await store.currentBase("doc"))?.hash).toBe(base1.hash);
		await importEvents(internals, [deltaEvent(c4, 1_750_000_000, base1.hash)]);
		// This device wrote c3 on base 1 (as the backend commits: b + its obligation).
		await store.addLocalChange(encodeChange(c3), { ...c3, b: base1.hash }, base1.hash);
		let state = replayObject({ ...(await store.objectInputs("doc")), pending: await store.pendingChangeIds() });
		expect(nameOf(state)).toBe("v4");
		expect(state?.fields["note"]?.stringValue).toBe("written offline");

		await importEvents(internals, baseEvents(base2.base, BASE_MS / 1000 + 60));
		expect((await store.currentBase("doc"))?.hash).toBe(base2.hash);
		expect(sync.stats.rebased).toBe(1);
		const inputs = await store.objectInputs("doc");
		expect(inputs.changes.map((c) => c.id)).not.toContain(c3.id); // the orphan is never replayed again
		const rebased = inputs.changes.find((c) => c.id !== c4.id)!;
		expect(rebased).toMatchObject({ objectId: "doc", author: c3.author, timestamp: c3.timestamp, parentIds: [c5.id], b: base2.hash });
		expect(rebased.ops).toEqual(c3.ops);
		expect(inputs.count).toBe(3); // c4, c3 (marked) and its rebase: the replay-cache counter still moves
		expect((await store.getPending(rebased.id))?.base).toBe(base2.hash);
		state = replayObject({ ...inputs, pending: await store.pendingChangeIds() });
		expect(nameOf(state)).toBe("v5");
		expect(state?.fields["note"]?.stringValue).toBe("written offline");
		const published = await sent.promise;
		expect(published.kind).toBe(1078);
		expect(published.tags).toContainEqual(["b", base2.hash]);

		// The same base again (another relay): nothing left to rebase.
		await importEvents(internals, baseEvents(base2.base, BASE_MS / 1000 + 60));
		expect(sync.stats.rebased).toBe(1);
		expect(sync.stats.bases).toBe(2);
	});
});

describe("backend replay from base", () => {
	interface Internals {
		store: ChangeStore;
		sync: unknown;
		states: Map<string, ObjectJSON>;
		dirty: Set<string>;
		vanished: Set<string>;
		queryUpserted: Set<string>;
		queryRemoved: Set<string>;
		allDirty: boolean;
		author: string;
	}
	test("the replay cache is invalidated when the base changes, and local writes build on the current base", async () => {
		const internals = backend as unknown as Internals;
		const saved = { ...internals };
		const store = await storeFixture();
		resetCore();
		try {
			await store.putBase(baseRow(base1.base), pickCurrentBase);
			await store.addChanges([{ bytes: encodeChange(c4), change: { ...c4, b: base1.hash } }]);
			Object.assign(internals, { store, sync: null, states: new Map(), dirty: new Set(), vanished: new Set(), queryUpserted: new Set(), queryRemoved: new Set(), allDirty: true, author: "tester" });
			expect((await backend.fetchObject("doc")).fields["name"]?.stringValue).toBe("v4");
			// Held from a base: queried from its replayed JSON, not the corpus.
			expect((await backend.fetchQuery({ filters: [], limit: 10 })).records.map((r: { name: string }) => r.name)).toEqual(["v4"]);
			// Written behind the replay; IndexedDB runs this read after that write.
			expect((await store.getStates()).get("doc")?.base).toBe(base1.hash);

			// Same change count, same (absent) checkpoint: only the base moved.
			await store.putBase(baseRow(base2.base), pickCurrentBase);
			Object.assign(internals, { states: new Map(), allDirty: true });
			expect((await backend.fetchObject("doc")).fields["name"]?.stringValue).toBe("v5");

			await backend.mutate("set_field", { object_id: "doc", key: "name", value: { stringValue: "v6" } });
			const local = (await store.objectInputs("doc")).changes.find((c) => c.id !== c4.id)!;
			expect(local).toMatchObject({ parentIds: [c5.id], b: base2.hash });
			expect((await store.getPending(local.id))?.base).toBe(base2.hash);
			expect((await backend.fetchObject("doc")).fields["name"]?.stringValue).toBe("v6");
		} finally {
			Object.assign(internals, saved);
			resetCore();
		}
	});
});

interface FakeSub { id: string; closed: boolean; oncustom?: (msg: string[]) => void; oneose?: () => void; fire(): void; receivedEose(): void; close(): void }

/** In-process NIP-77 relay (as in sync-negentropy.test.ts): NEG-OPEN/NEG-MSG over `events`, REQ with the matching ones. */
function fakeRelay(events: Event[]) {
	const subs = new Map<string, FakeSub>();
	const sessions = new Map<string, Negentropy>();
	const log = { negOpens: [] as Filter[], idFetches: [] as string[][], walks: [] as Filter[] };
	let serial = 0;
	const later = (fn: () => void) => queueMicrotask(fn);
	const relay = {
		onnotice: (_notice: string) => {},
		prepareSubscription(filters: Filter[], params: { onevent?: (e: Event) => void; oneose?: () => void }) {
			const id = `sub:${++serial}`;
			const sub: FakeSub = {
				id, closed: false, oneose: params.oneose,
				fire() {
					const f = filters[0];
					if (f.ids) log.idFetches.push(f.ids);
					else log.walks.push(f);
					later(() => {
						for (const e of events) if (matchFilter(f, e)) params.onevent?.(e);
						sub.oneose?.();
					});
				},
				receivedEose() {},
				close() { subs.delete(id); },
			};
			subs.set(id, sub);
			return sub;
		},
		async send(text: string) {
			const msg = JSON.parse(text) as [string, string, ...unknown[]];
			const sub = subs.get(msg[1]);
			if (msg[0] === "NEG-OPEN") {
				log.negOpens.push(msg[2] as Filter);
				const storage = new NegentropyStorage();
				for (const e of events) if (matchFilter(msg[2] as Filter, e)) storage.insert(e.created_at, e.id);
				storage.seal();
				const neg = new Negentropy(storage, 250_000);
				sessions.set(msg[1], neg);
				const reply = neg.reconcile(msg[3] as string).next!;
				later(() => sub?.oncustom?.(["NEG-MSG", msg[1], reply]));
			} else if (msg[0] === "NEG-MSG") {
				const reply = sessions.get(msg[1])!.reconcile(msg[2] as string).next!;
				later(() => sub?.oncustom?.(["NEG-MSG", msg[1], reply]));
			} else if (msg[0] === "NEG-CLOSE") sessions.delete(msg[1]);
		},
	};
	return { relay, log };
}

describe("bases-first history", () => {
	test("bases (and a split base's parts) first, then live deltas by #b and a window bounded by the newest base", async () => {
		const store = await storeFixture();
		// A base too big for one event: parts 1..n-1 ride in kind 1080, fetched by #c.
		const bigCreate = change("big", [{ objectCreate: { typeKey: "note" } }, setName("big"), { fieldSet: { key: "body", value: { stringValue: "x".repeat(90_000) } } }], [], 100);
		const big = compact([], [bigCreate]);
		const bigEvents = baseEvents(big.base, BASE_MS / 1000);
		const partEvents = bigEvents.slice(0, -1);
		expect(partEvents.length).toBeGreaterThan(1);
		expect(bigEvents.map((e) => e.kind)).toEqual([...partEvents.map(() => 1080), 31078]);
		const fresh = change("fresh", [{ objectCreate: { typeKey: "note" } }, setName("new since the base")], [], 600);
		const old = [deltaEvent(c1, 1_700_000_000), deltaEvent(c2, 1_700_000_100)];
		const kept = [...baseEvents(base1.base, BASE_MS / 1000), ...bigEvents, deltaEvent(c4, 1_750_000_000, base1.hash), deltaEvent(fresh, BASE_MS / 1000 + 100)];
		const { relay, log } = fakeRelay([...old, ...kept]);
		const { sync, internals } = syncFixture(store, { ensureRelay: async () => relay, close() {} });

		expect(await internals.backfill(1)).toBe(true);
		expect(internals.historyComplete).toBe(true);
		const shape = (f: Filter) => `${f.kinds!.join()}${f["#b"] ? `#b:${f["#b"].join()}` : ""}${f["#h"] ? `#h:${f["#h"].join()}` : ""}${f.since ? `>${f.since}` : ""}`;
		const ledgerTag = coreCall<string>("wire", { action: "blind", secret, id: "__vanished__" });
		// Bases, then checkpoints, then the #b deltas with the ledger stream (one cover, either order), then the window.
		const opens = log.negOpens.map(shape);
		expect([...opens.slice(0, 2), ...opens.slice(2, 4).sort(), ...opens.slice(4)]).toEqual(["31078", "1079", ...[`1078#b:${[base1.hash, big.hash].sort().join()}`, `1078#h:${ledgerTag}`].sort(), `1078>${BOUND_S}`]);
		expect(log.walks.map((f) => `${f.kinds!.join()}#c:${f["#c"]!.join()}`)).toEqual([`1080#c:${bigEvents.at(-1)!.tags.find((t) => t[0] === "c")![1]}`]);
		// Pre-migration history older than the window is never fetched once bases exist; parts come by #c, not by id.
		const parts = new Set(partEvents.map((e) => e.id));
		expect(new Set(log.idFetches.flat())).toEqual(new Set(kept.filter((e) => !parts.has(e.id)).map((e) => e.id)));
		expect(sync.stats.bases).toBe(2);
		expect([...(await store.currentBases()).keys()].sort()).toEqual(["big", "doc"]);
		expect((await store.changesFor("doc")).map((c) => [c.id, c.b])).toEqual([[c4.id, base1.hash]]);
		expect((await store.changesFor("fresh")).length).toBe(1);
		expect(nameOf(replayObject(await store.objectInputs("doc")))).toBe("v4");
		expect(replayObject(await store.objectInputs("big"))?.fields["body"]?.stringValue?.length).toBe(90_000);
		// The `#b` set is held: a second pass fetches nothing and asks for no parts.
		expect((await store.relayEvents("", [1078], { b: [base1.hash] })).length).toBe(1);
		log.idFetches.length = 0;
		log.walks.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(log.idFetches).toEqual([]);
		expect(log.walks).toEqual([]);
	});

	test("the vanish ledger never gets a base: its whole stream is read even when it is older than the window", async () => {
		// Rehearsal finding: a fresh replica of a compacted vault never loaded __vanished__ (its history predates
		// the newest base by more than a day), so nothing vanished was enforced and the ledger was unknown here.
		const store = await storeFixture();
		const ledger = change("__vanished__", [{ objectCreate: { typeKey: "vanish_log" } }, setName("Vanished objects")], [], 50);
		const ledgerEvent = deltaEvent(ledger, 1_600_000_000);
		const { relay } = fakeRelay([...baseEvents(base1.base, BASE_MS / 1000), ledgerEvent, deltaEvent(c1, 1_700_000_000)]);
		const { internals } = syncFixture(store, { ensureRelay: async () => relay, close() {} });
		expect(await internals.backfill(1)).toBe(true);
		expect((await store.changesFor("__vanished__")).map((c) => c.id)).toEqual([ledger.id]);
		// Pre-migration history of a based object stays unfetched.
		expect((await store.changesFor("doc")).length).toBe(0);
	});

	test("live subscriptions carry bases and their parts", async () => {
		const store = await storeFixture();
		const live: Filter[] = [];
		const { internals } = syncFixture(store, {
			subscribeMany: (_relays: string[], filter: Filter) => { live.push(filter); return { close() {} }; },
			close() {},
		});
		internals.subscribeLive();
		expect(live.find((f) => f.authors)?.kinds).toEqual([1078, 1079, 31078, 1080]);
	});
});
