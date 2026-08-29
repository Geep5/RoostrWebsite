/**
 * Roostr Web engine contracts.
 *
 * The desktop app talks to the local Odin server (127.0.0.1:7333). Roostr
 * Web replaces that server with a browser-side replica: raw Change
 * protobufs pulled from Nostr relays (kind 1078, NIP-44 self-encrypted),
 * cached in IndexedDB, replayed to object states, queried locally.
 *
 * Modules implementing these contracts:
 *   proto.ts   - Change encode/decode (protobufjs over engine-proto.proto)
 *   replay.ts  - change list -> ObjectJSON (parity with the Odin server)
 *   query.ts   - /api/query semantics (parity with src/query.odin)
 *   store.ts   - IndexedDB change cache + replayed-state memo
 *   sync.ts    - relay backfill/live/publish (schema of harness nostrsync.ts)
 *   keys.ts    - nsec handling, on-device storage
 *   mutate.ts  - /api/mutate actions as local Change builders + publish
 *   backend.ts - ties it together behind the app's api.ts surface
 */

import type { ObjectJSON, ValueJSON } from "$lib/types";

// ── proto.ts ──────────────────────────────────────────────────────

/** Decoded Change, JSON-shaped like the server's /api/changes output. */
export interface ChangeJSON {
	/** hex content address (sha256 of encoding with id zeroed) */
	id: string;
	objectId: string;
	/** hex parent ids */
	parentIds: string[];
	ops: OpJSON[];
	timestamp: number;
	author: string;
	snapshot?: unknown;
}

/** One operation; exactly one member set (mirrors glon.Operation oneof). */
export interface OpJSON {
	objectCreate?: { typeKey: string };
	objectDelete?: Record<string, never>;
	fieldSet?: { key: string; value: ValueJSON };
	fieldDelete?: { key: string };
	blockAdd?: { parentId?: string; afterId?: string; block: BlockWire; targetId?: string; position?: number };
	blockRemove?: { blockId: string };
	blockUpdate?: { blockId: string; content: unknown };
	blockMove?: { blockId: string; newParentId?: string; afterId?: string; targetId?: string; position?: number };
	blockSetAlign?: { blockId: string; align: number };
	blockSetBackground?: { blockId: string; color: string };
}

export interface BlockWire {
	id: string;
	childrenIds: string[];
	content: unknown;
	align?: number;
	backgroundColor?: string;
}

export interface ProtoApi {
	/** Decode raw .pb bytes; returns null on parse failure. */
	decodeChange(bytes: Uint8Array): ChangeJSON | null;
	/** Encode a change (id field ignored) to canonical bytes WITH id set. */
	encodeChange(change: ChangeJSON): Uint8Array;
	/** Content address: sha256 hex of the encoding with id zeroed. */
	changeId(change: ChangeJSON): string;
}

// ── replay.ts ─────────────────────────────────────────────────────

export interface ReplayApi {
	/**
	 * Replay an object's full change set (any order; topological sort +
	 * hex-id tie-break inside) into the same ObjectJSON the Odin server
	 * serves from /api/objects/:id.
	 */
	computeObject(changes: ChangeJSON[]): ObjectJSON | null;
}

// ── query.ts ──────────────────────────────────────────────────────

/** Body of /api/query — same JSON the app already sends. */
export type QueryBody = Record<string, unknown>;

export interface QueryResultRowJSON {
	id: string;
	typeKey: string;
	createdAt: number;
	updatedAt: number;
	fields: Record<string, ValueJSON>;
}

export interface QueryApi {
	/**
	 * Run /api/query semantics (src/query.odin): filters (equal on lists =
	 * whole-list equality; in/notIn intersect; allIn/exactIn; ordering
	 * comparisons; empty/notEmpty), setId -> setOf resolution, textQuery,
	 * hierarchical sorts with emptyPlacement + id tie-break, offset/limit.
	 */
	runQuery(states: Iterable<ObjectJSON>, body: QueryBody): { total: number; records: QueryResultRowJSON[] };
}

// ── store.ts ──────────────────────────────────────────────────────

export interface ChangeStoreApi {
	open(): Promise<void>;
	/** Add raw changes (idempotent by content address). Returns # new. */
	addChanges(changes: Array<{ bytes: Uint8Array; change: ChangeJSON }>): Promise<number>;
	/** All decoded changes for one object. */
	changesFor(objectId: string): Promise<ChangeJSON[]>;
	/** Every known object id. */
	objectIds(): Promise<string[]>;
	/** Relay cursor (unix seconds of newest imported event). */
	getCursor(): Promise<number>;
	setCursor(v: number): Promise<void>;
	/** Change ids already published to relays. */
	isPublished(changeId: string): Promise<boolean>;
	markPublished(changeId: string): Promise<void>;
}

// ── sync.ts ───────────────────────────────────────────────────────

export interface SyncEvents {
	/** Object ids whose change sets grew (batched). */
	onObjects(ids: string[]): void;
	onStatus(status: { phase: "backfill" | "live" | "error"; detail?: string; imported?: number }): void;
}

export interface RelaySyncApi {
	/** Backfill since cursor then stay live. Resolves once live. */
	start(): Promise<void>;
	stop(): void;
	/** Encrypt + publish one change (paced, retried). */
	publish(bytes: Uint8Array, changeId: string, objectId: string): void;
}

// ── keys.ts ───────────────────────────────────────────────────────

export interface KeyInfo {
	/** 32-byte secret */
	sk: Uint8Array;
	/** hex pubkey */
	pk: string;
	npub: string;
}

// ── backend.ts (consumed by $lib/api.ts) ──────────────────────────

export interface BackendApi {
	fetchObject(id: string): Promise<ObjectJSON>;
	fetchObjects(): Promise<unknown[]>; // ObjectSummary[]
	fetchChannels(): Promise<unknown[]>;
	fetchRelations(): Promise<unknown[]>;
	fetchQuery(body: QueryBody): Promise<{ total: number; records: QueryResultRowJSON[] }>;
	mutate(action: string, params: Record<string, unknown>): Promise<Record<string, unknown>>;
	/** Replaces the desktop SSE: fires with changed object ids. */
	onCommit(cb: (ids: string[]) => void): () => void;
}
