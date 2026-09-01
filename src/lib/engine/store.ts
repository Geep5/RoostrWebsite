/**
 * store.ts — IndexedDB change cache (ChangeStoreApi).
 *
 * Database 'roostr':
 *   changes  keyed by change id (hex content address) →
 *            { objectId, bytes, json }, with an 'objectId' index.
 *   meta     keyed by string → cursor at 'cursor', published change ids
 *            as individual 'published:<id>' keys.
 *   states   keyed by objectId → { n: change count, state: ObjectJSON } -
 *            the replayed object, persisted so boots don't re-replay the
 *            whole vault; invalidated per object when its change count
 *            grows (changes are append-only and content-addressed).
 *
 * Runs on raw IndexedDB. Under bun (no global indexedDB) it lazily pulls
 * fake-indexeddb; the specifier goes through a variable so Vite never
 * bundles the dev dependency.
 */

import type { ChangeJSON, ChangeStoreApi } from "./contracts";

const DB_NAME = "roostr";
const DB_VERSION = 2;
const CHANGES = "changes";
const META = "meta";
const STATES = "states";
const CURSOR_KEY = "cursor";

interface ChangeRow {
	objectId: string;
	bytes: Uint8Array;
	json: ChangeJSON;
}

function req<T>(r: IDBRequest<T>): Promise<T> {
	const { promise, resolve, reject } = Promise.withResolvers<T>();
	r.onsuccess = () => resolve(r.result);
	r.onerror = () => reject(r.error);
	return promise;
}

function txDone(tx: IDBTransaction): Promise<void> {
	const { promise, resolve, reject } = Promise.withResolvers<void>();
	tx.oncomplete = () => resolve();
	tx.onerror = () => reject(tx.error);
	tx.onabort = () => reject(tx.error ?? new Error("transaction aborted"));
	return promise;
}

async function idbFactory(): Promise<IDBFactory> {
	if (typeof indexedDB !== "undefined") return indexedDB;
	// Platform-specific module: only exists (and is only wanted) outside the
	// browser; a variable specifier keeps Vite from bundling the dev dep.
	const specifier = "fake-indexeddb";
	const mod = (await import(/* @vite-ignore */ specifier)) as { indexedDB: IDBFactory };
	return mod.indexedDB;
}

export class ChangeStore implements ChangeStoreApi {
	private db: IDBDatabase | null = null;

	constructor(private readonly name: string = DB_NAME) {}

	async open(): Promise<void> {
		if (this.db) return;
		const factory = await idbFactory();
		const { promise, resolve, reject } = Promise.withResolvers<IDBDatabase>();
		const r = factory.open(this.name, DB_VERSION);
		r.onupgradeneeded = () => {
			const db = r.result;
			if (!db.objectStoreNames.contains(CHANGES)) {
				const store = db.createObjectStore(CHANGES);
				store.createIndex("objectId", "objectId", { unique: false });
			}
			if (!db.objectStoreNames.contains(META)) db.createObjectStore(META);
			if (!db.objectStoreNames.contains(STATES)) db.createObjectStore(STATES);
		};
		r.onsuccess = () => resolve(r.result);
		r.onerror = () => reject(r.error);
		this.db = await promise;
	}

	close(): void {
		this.db?.close();
		this.db = null;
	}

	private handle(): IDBDatabase {
		if (!this.db) throw new Error("ChangeStore not open — call open() first");
		return this.db;
	}

	async addChanges(changes: Array<{ bytes: Uint8Array; change: ChangeJSON }>): Promise<number> {
		if (changes.length === 0) return 0;
		const db = this.handle();
		// Dedupe within the batch itself, then against the store.
		const byId = new Map<string, { bytes: Uint8Array; change: ChangeJSON }>();
		for (const item of changes) if (!byId.has(item.change.id)) byId.set(item.change.id, item);
		const items = [...byId.values()];

		const readStore = db.transaction(CHANGES, "readonly").objectStore(CHANGES);
		const existing = await Promise.all(items.map((it) => req(readStore.getKey(it.change.id))));
		const fresh = items.filter((_, i) => existing[i] === undefined);
		if (fresh.length === 0) return 0;

		const tx = db.transaction(CHANGES, "readwrite");
		const writeStore = tx.objectStore(CHANGES);
		for (const it of fresh) {
			const row: ChangeRow = { objectId: it.change.objectId, bytes: it.bytes, json: it.change };
			writeStore.put(row, it.change.id);
		}
		await txDone(tx);
		return fresh.length;
	}

	async changesFor(objectId: string): Promise<ChangeJSON[]> {
		const store = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES);
		const rows = (await req(store.index("objectId").getAll(objectId))) as ChangeRow[];
		return rows.map((r) => r.json);
	}

	async objectIds(): Promise<string[]> {
		const index = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES).index("objectId");
		const { promise, resolve, reject } = Promise.withResolvers<string[]>();
		const ids: string[] = [];
		const cursorReq = index.openKeyCursor(null, "nextunique");
		cursorReq.onsuccess = () => {
			const cursor = cursorReq.result;
			if (!cursor) {
				resolve(ids);
				return;
			}
			ids.push(String(cursor.key));
			cursor.continue();
		};
		cursorReq.onerror = () => reject(cursorReq.error);
		return promise;
	}

	/**
	 * objectId -> change count. A full key-cursor walk pays one microtask
	 * round-trip PER ROW (tens of seconds at 20k+ changes), so instead:
	 * unique-key walk (one step per object) + a parallel count() per id.
	 */
	async changeCounts(): Promise<Map<string, number>> {
		const ids = await this.objectIds();
		const index = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES).index("objectId");
		const counts = await Promise.all(ids.map((id) => req(index.count(id))));
		return new Map(ids.map((id, i) => [id, counts[i]]));
	}

	async getStates<T>(): Promise<Map<string, { n: number; state: T }>> {
		const store = this.handle().transaction(STATES, "readonly").objectStore(STATES);
		const [keys, values] = await Promise.all([req(store.getAllKeys()), req(store.getAll())]);
		const out = new Map<string, { n: number; state: T }>();
		keys.forEach((k, i) => out.set(String(k), values[i] as { n: number; state: T }));
		return out;
	}

	async putState<T>(objectId: string, n: number, state: T): Promise<void> {
		const tx = this.handle().transaction(STATES, "readwrite");
		tx.objectStore(STATES).put({ n, state }, objectId);
		await txDone(tx);
	}

	async getBootstrapped(): Promise<boolean> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		return (await req(store.get("bootstrapped"))) === true;
	}

	async setBootstrapped(): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(true, "bootstrapped");
		await txDone(tx);
	}

	async getCursor(): Promise<number> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		const value = await req(store.get(CURSOR_KEY));
		return typeof value === "number" ? value : 0;
	}

	async setCursor(v: number): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(v, CURSOR_KEY);
		await txDone(tx);
	}

	async isPublished(changeId: string): Promise<boolean> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		return (await req(store.getKey(`published:${changeId}`))) !== undefined;
	}

	async markPublished(changeId: string): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(true, `published:${changeId}`);
		await txDone(tx);
	}
}

/** Drop the whole replica database (logout / identity switch). */
export async function destroyDatabase(name: string = DB_NAME): Promise<void> {
	const factory = await idbFactory();
	await new Promise<void>((resolve) => {
		const req = factory.deleteDatabase(name);
		// blocked still completes once our closed connection is reaped -
		// and a reload follows immediately either way.
		req.onsuccess = req.onerror = req.onblocked = () => resolve();
	});
}
