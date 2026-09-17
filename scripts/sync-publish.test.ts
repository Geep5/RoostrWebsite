/**
 * Publishing is an obligation, never part of the write.
 *
 * A phone whose relay session cannot open (iOS Safari suspends sockets
 * aggressively) used to lose the change it was asked to publish: the
 * session error rejected `publish()`, which rejected the mutation, which
 * emptied the composer with nothing posted and no reason given. The
 * obligation must be persisted BEFORE the network is touched, and a
 * session failure must not surface as a failed write.
 */
import { afterEach, beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { getPublicKey } from "nostr-tools";
import { initCore } from "../src/lib/engine/core";
import { RelaySync } from "../src/lib/engine/sync";
import { ChangeStore, destroyDatabase } from "../src/lib/engine/store";
import { changeId, encodeChange } from "../src/lib/engine/proto";
import type { ChangeJSON } from "../src/lib/engine/contracts";

const sk = new Uint8Array(32).fill(7);

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

const cleanup: Array<() => Promise<void> | void> = [];
afterEach(async () => {
	for (const close of cleanup.splice(0).reverse()) await close();
});

async function storeFixture(): Promise<ChangeStore> {
	const name = `publish-test-${crypto.randomUUID()}`;
	const store = new ChangeStore(name);
	await store.open();
	cleanup.push(async () => {
		store.close();
		await destroyDatabase(name);
	});
	return store;
}

function chatChange(): ChangeJSON {
	const value: ChangeJSON = {
		id: "",
		objectId: "task",
		ops: [{ fieldSet: { key: "name", value: { stringValue: "hello from mobile" } } }],
		parentIds: [],
		timestamp: 100,
		author: getPublicKey(sk),
	};
	value.id = changeId(value);
	return value;
}

/** Test seam, like sync-authority.test.ts: drive the private session opener. */
interface SyncInternals {
	openSession(): Promise<void>;
}

/** A sync whose relay session always fails to open - the mobile failure. */
function deadSessionFixture(store: ChangeStore): RelaySync {
	const sync = new RelaySync(sk, [], store, { onObjects() {}, onStatus() {} });
	// Unchecked by necessity: openSession is private and has no public seam.
	const internals = sync as unknown as SyncInternals;
	internals.openSession = () => Promise.reject(new Error("relay unreachable"));
	cleanup.push(() => sync.stop());
	return sync;
}

test("a session that cannot open defers the publish instead of failing the write", async () => {
	const store = await storeFixture();
	const sync = deadSessionFixture(store);
	const change = chatChange();
	await sync.publish(encodeChange(change), change.id, change.objectId);

	// Stored, unsent: start() re-offers every stored obligation.
	const owed = await store.pendingPublishes();
	expect(owed.map((p) => p.changeId)).toEqual([change.id]);
	expect(await store.isPublished(change.id)).toBe(false);
});

test("an already-published change is not re-queued", async () => {
	const store = await storeFixture();
	const sync = deadSessionFixture(store);
	const change = chatChange();
	await store.markPublished(change.id);
	await sync.publish(encodeChange(change), change.id, change.objectId);

	expect(await store.pendingPublishes()).toEqual([]);
});
