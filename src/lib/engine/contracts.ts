/**
 * Roostr Web engine contracts.
 *
 * The desktop app talks to the local Odin server (127.0.0.1:7333). Roostr
 * Web replaces that server with a browser-side replica: raw Change
 * protobufs pulled from Nostr relays (kind 1078, NIP-44 self-encrypted),
 * cached in IndexedDB, replayed to object states, queried locally.
 *
 * Modules implementing these contracts:
 *   proto.ts   - Change encode/decode and hashing via the portable Odin core
 *   replay.ts  - portable Odin core change replay -> ObjectJSON
 *   query.ts   - portable Odin core /api/query semantics
 *   store.ts   - IndexedDB change cache + replayed-state memo
 *   sync.ts    - relay backfill/live/publish (schema of harness nostrsync.ts)
 *   keys.ts    - nsec handling, on-device storage
 *   mutate.ts  - portable Odin core mutation plans + platform persistence
 *   backend.ts - ties it together behind the app's api.ts surface
 */

import type { ObjectJSON, ValueJSON } from "$lib/types";
import type { Event } from "nostr-tools";

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
	 * serves from /api/objects/:id. With `checkpoint` (raw kind-1079
	 * Checkpoint protobuf) the core replays only the uncovered tail on top
	 * of it, or ignores it when every covered change is present and the
	 * replay order diverged (core.checkpoint_for_replay).
	 */
	computeObject(changes: ChangeJSON[], checkpoint?: Uint8Array): ObjectJSON | null;
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
	deleted?: boolean;
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

/** Authenticated transport context, never inferred from the inner author. */
export interface SharedProvenance {
	spaceId: string;
	keyId: number;
	signer: string;
}

export interface PendingPublish {
	key: string;
	objectId: string;
	changeId: string;
	bytes: Uint8Array;
	spaceId?: string;
	keyId?: number;
	/** Exact signed ciphertext retained across retries and reloads. */
	events?: Event[];
}

/** The one checkpoint held per object (docs/checkpoint-sync.md): a replay cache, never authority. */
export interface CheckpointRow {
	objectId: string;
	/** Raw Checkpoint protobuf, exactly as received. */
	bytes: Uint8Array;
	/** sha256 hex of `bytes`: dedup and tie-break, never a trust anchor. */
	hash: string;
	/** Sorted hex head ids the checkpoint state sits at. */
	heads: string[];
}

/** One object's raw history for a corpus load. */
export interface ObjectHistory {
	/** Set by the store; ad-hoc callers (tests) may omit it. */
	objectId?: string;
	checkpoint?: Uint8Array;
	changes: Uint8Array[];
}

export interface ChangeStoreApi {
	open(): Promise<void>;
	/** Add raw changes (idempotent by content address). Returns # new. */
	addChanges(changes: Array<{ bytes: Uint8Array; change: ChangeJSON }>): Promise<number>;
	/** Atomically save a local change and its personal publication obligation. */
	addLocalChange(bytes: Uint8Array, change: ChangeJSON): Promise<void>;
	pendingPublishes(): Promise<PendingPublish[]>;
	getPending(key: string): Promise<PendingPublish | undefined>;
	savePending(item: PendingPublish): Promise<void>;
	/** All decoded changes for one object. */
	changesFor(objectId: string): Promise<ChangeJSON[]>;
	/** Exact stored protobuf bytes with decoded metadata, without re-encoding. */
	rawChangesFor(objectId: string): Promise<Array<{ bytes: Uint8Array; change: ChangeJSON }>>;
	/** Every known object id, including checkpoint-only objects. */
	objectIds(): Promise<string[]>;
	getCheckpoint(objectId: string): Promise<CheckpointRow | undefined>;
	/** Store when it supersedes the held one (core.checkpoint_supersedes: covers a superset, then hash). Returns stored. */
	putCheckpoint(row: CheckpointRow): Promise<boolean>;
	allCheckpoints(): Promise<Map<string, CheckpointRow>>;
	/**
	 * One-time migration: builds before the cache contract walked kind-1078
	 * from a publisher's manifest cursor and never held the older history.
	 * Drops that record and, when it shortened a walk, the bootstrapped flag
	 * with it, so the next start walks from event zero.
	 */
	forgetCheckpointFloors(): Promise<void>;
	/** Relay cursor (unix seconds of newest imported event). */
	getCursor(): Promise<number>;
	/** Atomically save recovery identities with the cursor when supplied. */
	setCursor(v: number, replayGroups?: Array<[string, number]>): Promise<void>;
	/** Unresolved canonical chunk group keys and earliest event timestamps. */
	getReplayGroups(): Promise<Array<[string, number]>>;
	/** True once one COMPLETE history walk finished on this device. */
	getBootstrapped(): Promise<boolean>;
	setBootstrapped(): Promise<void>;
	/**
	 * Bootstrap resume point: the timestamp down to which a full-history walk
	 * has imported cleanly. Saved per page so an interrupted first walk (a
	 * phone locking its screen) resumes instead of restarting from zero.
	 * Cleared once the walk completes or a replay fault forces a full re-walk.
	 */
	getBootstrapFloor(): Promise<number | undefined>;
	setBootstrapFloor(v: number | undefined): Promise<void>;
	/** Change ids already published to relays. */
	isPublished(changeId: string): Promise<boolean>;
	markPublished(changeId: string): Promise<void>;
}

// ── sync.ts ───────────────────────────────────────────────────────

export interface SyncEvents {
	/** Object ids whose change sets grew (batched). */
	onObjects(ids: string[]): void;
	onStatus(status: { phase: "backfill" | "live" | "error"; detail?: string; imported?: number; pending?: number }): void;
}

export interface RelaySyncApi {
	/** Backfill since cursor then stay live. Resolves once live. */
	start(): Promise<void>;
	stop(): void;
	/** Encrypt + publish one change (paced, retried). */
	publish(bytes: Uint8Array, changeId: string, objectId: string): Promise<void>;
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
