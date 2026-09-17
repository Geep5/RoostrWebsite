/**
 * The history walk must persist each page as it lands.
 *
 * It used to buffer every event from every relay and import once at the
 * end. A phone loses its tab to a lock screen or a memory reclaim long
 * before a full vault finishes downloading, so that buffer was discarded
 * over and over: each device ended up with whatever arbitrary slice it
 * managed to finish, and two browsers on the same key showed different
 * sets of spaces.
 */
import { afterEach, beforeAll, expect, test, spyOn } from "bun:test";
import { readFileSync } from "node:fs";
import { finalizeEvent, getPublicKey, nip44, type Event } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils.js";
import { initCore } from "../src/lib/engine/core";
import { blindShared, PAGE_LIMIT, RelaySync, type SharedSpaceInfo } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { changeId, encodeChange } from "../src/lib/engine/proto";
import type { ChangeJSON } from "../src/lib/engine/contracts";

const ownerSk = new Uint8Array(32).fill(1);
const owner = getPublicKey(ownerSk);
const spaceKey = new Uint8Array(32).fill(4);
const space: SharedSpaceInfo = { spaceId: "space", keyId: 1, keyHex: bytesToHex(spaceKey), owner, writers: [owner] };

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => {
	for (const close of cleanup.splice(0).reverse()) await close();
});

/** Test seam, as in sync-authority.test.ts: drive the private walk. */
interface SyncInternals {
	backfill(since: number): Promise<boolean>;
	queryRelayPage(url: string, filter: Record<string, unknown>): Promise<Event[]>;
	backfillChain: Promise<boolean>;
}

async function storeFixture(): Promise<ChangeStore> {
	const name = `incremental-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => {
		store.close();
		await destroyDatabase(name);
	});
	return store;
}

function spaceChange(objectId: string, name: string): ChangeJSON {
	const value: ChangeJSON = {
		id: "",
		objectId,
		ops: [{ objectCreate: { typeKey: "channel" } }, { fieldSet: { key: "name", value: { stringValue: name } } }],
		parentIds: [],
		timestamp: 100,
		author: owner,
	};
	value.id = changeId(value);
	return value;
}

/** One change, wrapped as the shared-space relay event the walk ingests. */
function event(change: ChangeJSON): Event {
	const part = Buffer.from(encodeChange(change)).toString("base64");
	return finalizeEvent(
		{ kind: 1078, created_at: 100, content: nip44.encrypt(part, spaceKey), tags: [["h", blindShared(bytesToHex(spaceKey), "space:space")]] },
		ownerSk,
	);
}

test("a page is in the store before the walk finishes", async () => {
	const store = await storeFixture();
	const sync = new RelaySync(ownerSk, ["wss://one.test"], store, { onObjects() {}, onStatus() {} });
	sync.setSharedSpaces([space]);
	// Unchecked by necessity: the walk and its paging hook are private.
	const internals = sync as unknown as SyncInternals;
	cleanup.push(async () => {
		sync.stop();
		await internals.backfillChain;
	});

	const first = spaceChange("space", "Grant Private");
	// A full page keeps the walk going, so a second call happens while the
	// first page's fate is observable.
	const page = [event(first), ...Array.from({ length: PAGE_LIMIT - 1 }, (_, i) => event(spaceChange(`note-${i}`, `note ${i}`)))];
	let firstPageDelivered = false;
	let secondPageSawFirstImported = false;
	spyOn(internals, "queryRelayPage").mockImplementation(async (_relay, filter) => {
		if (!filter["#h"]) return [];
		if (!firstPageDelivered) {
			firstPageDelivered = true;
			return page;
		}
		// Second call: the walk is still running, so the first page must
		// already be durable rather than sitting in a buffer.
		secondPageSawFirstImported = (await store.changesFor("space")).length === 1;
		return [];
	});

	await internals.backfill(1);
	expect(firstPageDelivered).toBe(true);
	expect((await store.changesFor("space")).length).toBe(1);
	expect(secondPageSawFirstImported).toBe(true);
});
