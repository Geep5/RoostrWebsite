import { afterEach, beforeAll, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, matchFilter, nip44, type Event, type Filter } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import { initCore } from "../src/lib/engine/core";
import { blindShared, FETCH_BATCH, RelaySync, type SharedSpaceInfo } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { changeId, encodeChange } from "../src/lib/engine/proto";
import { Negentropy, NegentropyStorage } from "../src/lib/engine/negentropy";
import type { ChangeJSON } from "../src/lib/engine/contracts";

const ownerSk = new Uint8Array(32).fill(1);
const owner = getPublicKey(ownerSk);
const selfKey = nip44.getConversationKey(ownerSk, owner);
const spaceKey = new Uint8Array(32).fill(4);
const space: SharedSpaceInfo = { spaceId: "space", keyId: 1, keyHex: bytesToHex(spaceKey), owner, writers: [owner] };
let spaceTag: string;

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
	spaceTag = blindShared(bytesToHex(spaceKey), "space:space");
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });

async function storeFixture() {
	const name = `negentropy-test-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => { store.close(); await destroyDatabase(name); });
	return store;
}

function note(objectId: string, channel?: string): ChangeJSON {
	const ops: ChangeJSON["ops"] = [{ objectCreate: { typeKey: "note" } }, { fieldSet: { key: "name", value: { stringValue: objectId } } }];
	if (channel) ops.push({ fieldSet: { key: "channel", value: { stringValue: channel } } });
	const value: ChangeJSON = { id: "", objectId, ops, parentIds: [], timestamp: 100, author: owner };
	value.id = changeId(value);
	return value;
}
const base64 = (c: ChangeJSON) => Buffer.from(encodeChange(c)).toString("base64");

/** A personal kind-1078 change, as the outbox seals it. */
function personal(c: ChangeJSON, createdAt: number): Event {
	const h = bytesToHex(sha256(new Uint8Array([...ownerSk, ...new TextEncoder().encode(c.objectId)]))).slice(0, 16);
	return finalizeEvent({ kind: 1078, created_at: createdAt, content: nip44.encrypt(base64(c), selfKey), tags: [["h", h]] }, ownerSk);
}
function spaced(part: string, createdAt: number, tags: string[][] = []): Event {
	return finalizeEvent({ kind: 1078, created_at: createdAt, content: nip44.encrypt(part, spaceKey), tags: [["h", spaceTag], ...tags] }, ownerSk);
}

interface FakeSub {
	id: string;
	closed: boolean;
	oncustom?: (msg: string[]) => void;
	fire(): void;
	receivedEose(): void;
	close(): void;
	oneose?: () => void;
}

/**
 * In-process NIP-77 relay: answers NEG-OPEN/NEG-MSG with a responder over
 * `events`, REQ with the matching events then EOSE. `negErr` / `notice`
 * make it refuse negentropy the two ways real relays do.
 */
function fakeRelay(events: Event[], opts: { negErr?: string; notice?: string; withhold?: Set<string> } = {}) {
	const subs = new Map<string, FakeSub>();
	const sessions = new Map<string, Negentropy>();
	const log = { negOpens: [] as Filter[], idFetches: [] as string[][], walks: [] as Filter[], negCloses: 0 };
	let serial = 0;
	const later = (fn: () => void) => queueMicrotask(fn);
	const relay = {
		onnotice: (_notice: string) => {},
		prepareSubscription(filters: Filter[], params: { onevent?: (e: Event) => void; oneose?: () => void; onclose?: (reason: string) => void }) {
			const id = `sub:${++serial}`;
			const sub: FakeSub = {
				id,
				closed: false,
				oneose: params.oneose,
				fire() {
					const f = filters[0];
					if (f.ids) log.idFetches.push(f.ids);
					else log.walks.push(f);
					later(() => {
						for (const e of events) if (matchFilter(f, e) && !opts.withhold?.has(e.id)) params.onevent?.(e);
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
				if (opts.notice) return later(() => relay.onnotice(opts.notice!));
				if (opts.negErr) return later(() => sub?.oncustom?.(["NEG-ERR", msg[1], opts.negErr!]));
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
			} else if (msg[0] === "NEG-CLOSE") {
				log.negCloses++;
				sessions.delete(msg[1]);
			}
		},
	};
	return { relay, log };
}

interface SyncInternals {
	backfill(since: number): Promise<boolean>;
	historyComplete: boolean;
	pool: unknown;
	backfillChain: Promise<boolean>;
}

function syncFixture(store: ChangeStore, relay: ReturnType<typeof fakeRelay>["relay"], statuses: string[] = []) {
	const sync = new RelaySync(ownerSk, ["wss://fake.test"], store, { onObjects() {}, onStatus: (s) => { if (s.detail) statuses.push(s.detail); } });
	const internals = sync as unknown as SyncInternals;
	internals.pool = { ensureRelay: async () => relay, close() {} };
	sync.setSharedSpaces([space]);
	cleanup.push(async () => { sync.stop(); await internals.backfillChain; });
	return { sync, internals };
}

describe("NIP-77 history sync", () => {
	test("a fresh store fetches exactly the needed ids in batches, completes, and a second run fetches nothing", async () => {
		const store = await storeFixture();
		const events: Event[] = [];
		for (let i = 0; i < 230; i++) events.push(personal(note(`doc${i}`), 1000 + (i >> 2)));
		// Our own space event sits in both the self stream and the space stream.
		const shared = spaced(base64(note("sdoc", "space")), 2000);
		events.push(shared);
		const { relay, log } = fakeRelay(events);
		const { internals } = syncFixture(store, relay);

		expect(await internals.backfill(1)).toBe(true);
		expect(internals.historyComplete).toBe(true);
		// Checkpoints then changes: self + space each.
		expect(log.negOpens.map((f) => `${f.kinds!.join()}${f["#h"] ? "#h" : ""}`)).toEqual(["1079", "1079,5#h", "1078", "1078#h"]);
		expect(log.negOpens.every((f) => f.since === undefined && f.limit === undefined)).toBe(true);
		expect(log.walks).toEqual([]);
		const fetched = log.idFetches.flat();
		expect(fetched.length).toBe(events.length); // each id once, though `shared` is in two streams
		expect(new Set(fetched)).toEqual(new Set(events.map((e) => e.id)));
		expect(log.idFetches.every((ids) => ids.length <= FETCH_BATCH)).toBe(true);
		expect(log.negCloses).toBe(4);
		for (let i = 0; i < 230; i += 57) expect((await store.changesFor(`doc${i}`)).length).toBe(1);
		expect((await store.changesFor("sdoc")).length).toBe(1);
		// Held per stream: the shared event under both scopes.
		expect((await store.relayEvents("", [1078])).length).toBe(231);
		expect((await store.relayEvents(spaceTag, [1078])).map((r) => r.id)).toEqual([shared.id]);

		log.idFetches.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(log.idFetches).toEqual([]);
	});

	test("a needed id the relay does not return keeps history incomplete until it is imported", async () => {
		const store = await storeFixture();
		const events = [0, 1, 2].map((i) => personal(note(`doc${i}`), 1000 + i));
		const withhold = new Set([events[1].id]);
		const statuses: string[] = [];
		const { relay, log } = fakeRelay(events, { withhold });
		const { internals } = syncFixture(store, relay, statuses);
		expect(await internals.backfill(1)).toBe(false);
		expect(internals.historyComplete).toBe(false);
		expect(statuses.some((s) => s.includes("1 reconciled events not returned"))).toBe(true);
		expect((await store.relayEvents("", [1078])).length).toBe(2);
		withhold.clear();
		log.idFetches.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(log.idFetches.flat()).toEqual([events[1].id]);
		expect((await store.changesFor("doc1")).length).toBe(1);
	});

	test("a batch whose import fails holds nothing and is fetched again", async () => {
		const store = await storeFixture();
		const events = [0, 1].map((i) => personal(note(`doc${i}`), 1000 + i));
		const { relay, log } = fakeRelay(events);
		const { internals } = syncFixture(store, relay);
		const add = store.addChanges.bind(store);
		let fail = true;
		store.addChanges = async (rows) => {
			if (fail) { fail = false; throw new Error("disk full"); }
			return add(rows);
		};
		await expect(internals.backfill(1)).rejects.toThrow("disk full");
		expect(internals.historyComplete).toBe(false);
		expect(await store.relayEvents("", [1078])).toEqual([]);
		log.idFetches.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(new Set(log.idFetches.flat())).toEqual(new Set(events.map((e) => e.id)));
	});

	test("chunk parts are held only once their group imports", async () => {
		const store = await storeFixture();
		const full = base64(note("big", "space"));
		const cut = Math.floor(full.length / 2);
		const gid = bytesToHex(sha256(new TextEncoder().encode(full))).slice(0, 16);
		const parts = [full.slice(0, cut), full.slice(cut)].map((part, i) => spaced(part, 3000, [["c", gid, String(i), "2"]]));
		const events = [parts[0]];
		const { relay, log } = fakeRelay(events);
		const { internals } = syncFixture(store, relay);
		expect(await internals.backfill(1)).toBe(false); // an open group is unfinished history
		expect(await store.relayEvents(spaceTag, [1078])).toEqual([]);
		events.push(parts[1]);
		log.idFetches.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(new Set(log.idFetches.flat())).toEqual(new Set(parts.map((e) => e.id)));
		expect(new Set((await store.relayEvents(spaceTag, [1078])).map((r) => r.id))).toEqual(new Set(parts.map((e) => e.id)));
		expect((await store.changesFor("big")).length).toBe(1);
		log.idFetches.length = 0;
		expect(await internals.backfill(1)).toBe(true);
		expect(log.idFetches).toEqual([]);
	});

	for (const [name, opts] of [["NEG-ERR", { negErr: "blocked: not here" }], ["NOTICE", { notice: "unknown message type" }]] as const) {
		test(`${name} on NEG-OPEN falls back to the paged walk and is not asked again`, async () => {
			const store = await storeFixture();
			const events = [0, 1].map((i) => personal(note(`doc${i}`), 1000 + i));
			const { relay, log } = fakeRelay(events, opts);
			const { internals } = syncFixture(store, relay);
			expect(await internals.backfill(1)).toBe(true);
			expect(log.negOpens.length).toBe(1); // remembered after the first refusal
			expect(log.idFetches).toEqual([]);
			expect(log.walks.map((f) => `${f.kinds!.join()}${f["#h"] ? "#h" : ""}`)).toEqual(["1079", "1079,5#h", "1078", "1078#h"]);
			expect(log.walks.every((f) => f.since === 1 && f.limit === 128)).toBe(true);
			expect((await store.changesFor("doc1")).length).toBe(1);
			// Walked events are held too: a later NIP-77 relay would not resend them.
			expect((await store.relayEvents("", [1078])).length).toBe(2);
			expect(await internals.backfill(1)).toBe(true);
			expect(log.negOpens.length).toBe(1);
		});
	}
});
