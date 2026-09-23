/**
 * The replica must never wait forever on IndexedDB, and a database that
 * stopped answering must not silently delete objects from the view.
 *
 * Both were live on iOS Safari: a send stuck mid-flight (promise never
 * settled) and a space list / discussion missing rows for the rest of the
 * session (a caught read error dropped the object from the retry set).
 */
import { beforeAll, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { getPublicKey } from "nostr-tools";
import { IDB_TIMEOUT_MS, req, StorageUnavailableError } from "../src/lib/engine/store";
import { initCore } from "../src/lib/engine/core";
import { backend } from "../src/lib/engine/backend";
import { changeId } from "../src/lib/engine/proto";
import type { ChangeJSON, ChangeStoreApi } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

/** An IndexedDB request that never fires success or error - WebKit's silent failure. */
function wedgedRequest(): IDBRequest<number> {
	const handlers: Record<string, unknown> = {};
	// Unchecked by necessity: a hand-rolled stand-in for a host object that
	// only ever has its event handlers assigned.
	return handlers as unknown as IDBRequest<number>;
}

test("a request that never fires rejects instead of hanging", async () => {
	const started = Date.now();
	await expect(req(wedgedRequest(), 50)).rejects.toThrow(StorageUnavailableError);
	expect(Date.now() - started).toBeLessThan(5_000);
});

test("the timeout names the failure so the UI can report it", async () => {
	await expect(req(wedgedRequest(), 50)).rejects.toThrow(/local storage stopped responding/);
});

test("the default budget is the one the store documents", () => {
	expect(IDB_TIMEOUT_MS).toBe(12_000);
});

/** Test seam: the replica's private store, so a read can be made to fail. */
interface BackendInternals {
	store: ChangeStoreApi;
	allDirty: boolean;
	states: Map<string, ObjectJSON>;
}

function creation(objectId: string, name: string): ChangeJSON {
	const value: ChangeJSON = {
		id: "",
		objectId,
		ops: [{ objectCreate: { typeKey: "task" } }, { fieldSet: { key: "name", value: { stringValue: name } } }],
		parentIds: [],
		timestamp: 100,
		author: getPublicKey(new Uint8Array(32).fill(5)),
	};
	value.id = changeId(value);
	return value;
}

test("a database that stops answering is retried, not silently dropped", async () => {
	// Unchecked by necessity: WebBackend exposes only its singleton.
	const internals = backend as unknown as BackendInternals;
	let failures = 1;
	const rows = new Map<string, ChangeJSON[]>([
		["good", [creation("good", "loads")]],
		["flaky", [creation("flaky", "the other space")]],
	]);
	internals.store = {
		changeCounts: async () => new Map([...rows].map(([id, list]) => [id, list.length])),
		getStates: async () => new Map(),
		allCheckpoints: async () => new Map(),
		getCheckpoint: async () => undefined,
		changesFor: async (id: string) => {
			if (id === "flaky" && failures-- > 0) throw new StorageUnavailableError("local storage stopped responding (read); reload the page");
			return rows.get(id) ?? [];
		},
		putState: async () => {},
		// The replica replaces a wedged connection before retrying.
		close: () => {},
		open: async () => {},
	} as unknown as ChangeStoreApi;
	internals.allDirty = true;
	internals.states = new Map();

	// Loud, not partial: the caller learns the view is incomplete.
	await expect(backend.fetchObjects()).rejects.toThrow(StorageUnavailableError);

	// The next attempt replays what the failure left owing.
	const names = (await backend.fetchObjects()).map((o) => o.name).sort();
	expect(names).toEqual(["loads", "the other space"]);
});
