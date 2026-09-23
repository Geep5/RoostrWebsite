/**
 * store.ts — IndexedDB change cache (ChangeStoreApi).
 *
 * Database 'roostr':
 *   changes      keyed by change id (hex content address) →
 *                { objectId, bytes, json }, with an 'objectId' index.
 *   checkpoints  keyed by objectId → { objectId, bytes, hash, heads }: the
 *                one kind-1079 checkpoint held per object (covers a superset
 *                wins, then the larger hash - core.checkpoint_supersedes).
 *   meta         keyed by string → cursor at 'cursor', published change ids
 *                as individual 'published:<id>' keys.
 *   states       keyed by objectId → { n: change count, cp: checkpoint hash,
 *                state: ObjectJSON } - the replayed object, persisted so
 *                boots don't re-replay the whole vault; invalidated per
 *                object when its change count grows or its checkpoint moves
 *                (changes are append-only and content-addressed).
 *
 * Runs on raw IndexedDB. Under bun (no global indexedDB) it lazily pulls
 * fake-indexeddb; the specifier goes through a variable so Vite never
 * bundles the dev dependency.
 */

import type { ChangeJSON, ChangeStoreApi, CheckpointRow, ObjectHistory, PendingPublish } from "./contracts";
import { checkpointSupersedes } from "./proto";

const DB_NAME = "roostr";
const DB_VERSION = 3;
const CHANGES = "changes";
const CHECKPOINTS = "checkpoints";
const META = "meta";
const STATES = "states";
const CURSOR_KEY = "cursor";
/** Pre-cache-contract manifest floors; see forgetCheckpointFloors. */
const CHECKPOINT_FLOORS_KEY = "checkpoint-floors";

/** Persisted replay memo; see getStates/putState. */
export interface StateMemo<T> {
	n: number;
	/** Checkpoint hash the state was replayed on top of; "" for none. */
	cp: string;
	state: T;
}

interface ChangeRow {
	objectId: string;
	bytes: Uint8Array;
	json: ChangeJSON;
}

/** The database itself is unreachable - as opposed to a change that will not replay. */
export class StorageUnavailableError extends Error {}

/**
 * WebKit can leave an IndexedDB request pending with no event at all -
 * after storage pressure, an eviction mid-session, or a backgrounded tab
 * whose database process went away. An unbounded wait then propagates
 * upward as a promise that never settles: a send button stuck mid-flight,
 * a discussion that never finishes loading, a space list missing rows.
 * A bounded wait turns that silence into an error a caller can report.
 */
export const IDB_TIMEOUT_MS = 12_000;

async function withTimeout<T>(promise: Promise<T>, what: string, ms = IDB_TIMEOUT_MS): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	try {
		return await Promise.race([
			promise,
			new Promise<never>((_, reject) => {
				timer = setTimeout(() => reject(new StorageUnavailableError(`local storage stopped responding (${what}); reload the page`)), ms);
			}),
		]);
	} finally {
		clearTimeout(timer);
	}
}

export function req<T>(r: IDBRequest<T>, timeoutMs = IDB_TIMEOUT_MS): Promise<T> {
	const { promise, resolve, reject } = Promise.withResolvers<T>();
	r.onsuccess = () => resolve(r.result);
	r.onerror = () => reject(r.error);
	return withTimeout(promise, "read", timeoutMs);
}

function txDone(tx: IDBTransaction): Promise<void> {
	const { promise, resolve, reject } = Promise.withResolvers<void>();
	tx.oncomplete = () => resolve();
	tx.onerror = () => reject(tx.error);
	tx.onabort = () => reject(tx.error ?? new Error("transaction aborted"));
	return withTimeout(promise, "write");
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
			if (!db.objectStoreNames.contains(CHECKPOINTS)) db.createObjectStore(CHECKPOINTS);
			if (!db.objectStoreNames.contains(META)) db.createObjectStore(META);
			if (!db.objectStoreNames.contains(STATES)) db.createObjectStore(STATES);
		};
		r.onsuccess = () => resolve(r.result);
		r.onerror = () => reject(r.error);
		// Another tab still holding an older version fires `blocked` and then
		// NOTHING: no success, no error. Boot would wait forever on an event
		// that never comes, so name the cause instead.
		r.onblocked = () => reject(new StorageUnavailableError("another Roostr tab is holding an older local database; close it (or reload every Roostr tab) and try again"));
		this.db = await withTimeout(promise, "open");
		// A logout in ANY tab deletes this database; a connection that
		// doesn't yield here blocks that delete forever - and every later
		// open() queues behind the pending delete. Yield and reload: the
		// identity changed under this tab, so it must re-gate anyway.
		this.db.onversionchange = () => {
			this.db?.close();
			this.db = null;
			if (typeof location !== "undefined") location.reload();
		};
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

	async addLocalChange(bytes: Uint8Array, change: ChangeJSON): Promise<void> {
		const tx = this.handle().transaction([CHANGES, META], "readwrite");
		tx.objectStore(CHANGES).put({ objectId: change.objectId, bytes, json: change } satisfies ChangeRow, change.id);
		tx.objectStore(META).put({ key: change.id, changeId: change.id, objectId: change.objectId, bytes } satisfies PendingPublish, `pending:${change.id}`);
		await txDone(tx);
	}

	async pendingPublishes(): Promise<PendingPublish[]> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		const [keys, values] = await Promise.all([req(store.getAllKeys()), req(store.getAll())]);
		return values.filter((_, i) => String(keys[i]).startsWith("pending:")) as PendingPublish[];
	}

	async getPending(key: string): Promise<PendingPublish | undefined> {
		return await req(this.handle().transaction(META, "readonly").objectStore(META).get(`pending:${key}`)) as PendingPublish | undefined;
	}

	async savePending(item: PendingPublish): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(item, `pending:${item.key}`);
		await txDone(tx);
	}

	async changesFor(objectId: string): Promise<ChangeJSON[]> {
		const store = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES);
		const rows = (await req(store.index("objectId").getAll(objectId))) as ChangeRow[];
		return rows.map((r) => r.json);
	}

	async rawChangesFor(objectId: string): Promise<Array<{ bytes: Uint8Array; change: ChangeJSON }>> {
		const store = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES);
		const rows = (await req(store.index("objectId").getAll(objectId))) as ChangeRow[];
		return rows.map((row) => ({ bytes: row.bytes, change: row.json }));
	}

	async objectIds(): Promise<string[]> {
		const ids = new Set<string>(await this.checkpointObjectIds());
		// WebKit throws "Unable to open cursor" for a nextunique cursor on an
		// index with no entries (dexie/Dexie.js#1030, still present on iOS 17+),
		// which is exactly a fresh vault on a new origin. Nothing to iterate anyway.
		if ((await req(this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES).count())) === 0) return [...ids];
		const index = this.handle().transaction(CHANGES, "readonly").objectStore(CHANGES).index("objectId");
		const { promise, resolve, reject } = Promise.withResolvers<string[]>();
		const cursorReq = index.openKeyCursor(null, "nextunique");
		cursorReq.onsuccess = () => {
			const cursor = cursorReq.result;
			if (!cursor) {
				resolve([...ids]);
				return;
			}
			ids.add(String(cursor.key));
			cursor.continue();
		};
		cursorReq.onerror = () => reject(cursorReq.error);
		return promise;
	}

	private async checkpointObjectIds(): Promise<string[]> {
		const keys = await req(this.handle().transaction(CHECKPOINTS, "readonly").objectStore(CHECKPOINTS).getAllKeys());
		return keys.map(String);
	}

	/**
	 * Complete protobuf histories, grouped by the stored object id rather than
	 * decoding or serialising changes. A corpus batch must never split one.
	 * An object's checkpoint rides along; the core decides whether it seeds
	 * the replay or IS the history (core.checkpoint_for_replay).
	 */
	async allChangeHistories(): Promise<ObjectHistory[]> {
		const tx = this.handle().transaction([CHANGES, CHECKPOINTS], "readonly");
		const [rows, checkpoints] = await Promise.all([
			req(tx.objectStore(CHANGES).getAll()) as Promise<ChangeRow[]>,
			req(tx.objectStore(CHECKPOINTS).getAll()) as Promise<CheckpointRow[]>,
		]);
		const histories = new Map<string, ObjectHistory>();
		const historyOf = (objectId: string): ObjectHistory => {
			let history = histories.get(objectId);
			if (!history) histories.set(objectId, (history = { changes: [] }));
			return history;
		};
		for (const cp of checkpoints) historyOf(cp.objectId).checkpoint = cp.bytes;
		for (const row of rows) {
			if (!(row.bytes instanceof Uint8Array) || row.bytes.byteLength === 0) {
				throw new Error(`Missing protobuf bytes for object ${row.objectId}`);
			}
			historyOf(row.objectId).changes.push(row.bytes);
		}
		return [...histories.values()];
	}

	async getCheckpoint(objectId: string): Promise<CheckpointRow | undefined> {
		return (await req(this.handle().transaction(CHECKPOINTS, "readonly").objectStore(CHECKPOINTS).get(objectId))) as CheckpointRow | undefined;
	}

	async allCheckpoints(): Promise<Map<string, CheckpointRow>> {
		const rows = (await req(this.handle().transaction(CHECKPOINTS, "readonly").objectStore(CHECKPOINTS).getAll())) as CheckpointRow[];
		return new Map(rows.map((row) => [row.objectId, row]));
	}

	/**
	 * Keep `row` when it beats the stored checkpoint (core.checkpoint_supersedes:
	 * covers a superset of the held change ids, then the larger hash). Never
	 * created_at - the relay is transport, not authority - and never a bare
	 * count: an incomparable fork from another device must not evict the held
	 * one. Returns stored.
	 */
	async putCheckpoint(row: CheckpointRow): Promise<boolean> {
		const tx = this.handle().transaction(CHECKPOINTS, "readwrite");
		const store = tx.objectStore(CHECKPOINTS);
		const existing = (await req(store.get(row.objectId))) as CheckpointRow | undefined;
		if (existing && (existing.hash === row.hash || !checkpointSupersedes(row.bytes, existing.bytes))) return false;
		store.put(row, row.objectId);
		await txDone(tx);
		return true;
	}

	async forgetCheckpointFloors(): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		const store = tx.objectStore(META);
		const floors = await req(store.get(CHECKPOINT_FLOORS_KEY));
		if (floors === undefined) return;
		store.delete(CHECKPOINT_FLOORS_KEY);
		if (Object.values((floors ?? {}) as Record<string, number>).some((v) => v > 0)) store.delete("bootstrapped");
		await txDone(tx);
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

	async getStates<T>(): Promise<Map<string, StateMemo<T>>> {
		const store = this.handle().transaction(STATES, "readonly").objectStore(STATES);
		const [keys, values] = await Promise.all([req(store.getAllKeys()), req(store.getAll())]);
		const out = new Map<string, StateMemo<T>>();
		keys.forEach((k, i) => {
			const memo = values[i] as Partial<StateMemo<T>> & { n: number; state: T };
			out.set(String(k), { n: memo.n, cp: memo.cp ?? "", state: memo.state });
		});
		return out;
	}

	async putState<T>(objectId: string, n: number, cp: string, state: T): Promise<void> {
		const tx = this.handle().transaction(STATES, "readwrite");
		tx.objectStore(STATES).put({ n, cp, state } satisfies StateMemo<T>, objectId);
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

	async getBootstrapFloor(): Promise<number | undefined> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		const value = await req(store.get("bootstrap-floor"));
		return typeof value === "number" ? value : undefined;
	}

	async setBootstrapFloor(v: number | undefined): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		if (v === undefined) tx.objectStore(META).delete("bootstrap-floor");
		else tx.objectStore(META).put(v, "bootstrap-floor");
		await txDone(tx);
	}

	async getCursor(): Promise<number> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		const value = await req(store.get(CURSOR_KEY));
		return typeof value === "number" ? value : 0;
	}

	async setCursor(v: number, replayGroups?: Array<[string, number]>): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(v, CURSOR_KEY);
		if (replayGroups) tx.objectStore(META).put(replayGroups, "replay-groups");
		await txDone(tx);
	}

	async getReplayGroups(): Promise<Array<[string, number]>> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		return (await req(store.get("replay-groups"))) ?? [];
	}

	async isPublished(changeId: string): Promise<boolean> {
		const store = this.handle().transaction(META, "readonly").objectStore(META);
		return (await req(store.getKey(`published:${changeId}`))) !== undefined;
	}

	async markPublished(changeId: string): Promise<void> {
		const tx = this.handle().transaction(META, "readwrite");
		tx.objectStore(META).put(true, `published:${changeId}`);
		tx.objectStore(META).delete(`pending:${changeId}`);
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
