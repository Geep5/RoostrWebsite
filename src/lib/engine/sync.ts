/**
 * sync.ts — relay backfill/live/publish (RelaySyncApi).
 *
 * Wire schema mirrors the desktop daemon (glonOdin/harness/src/nostrsync.ts)
 * and lives in the shared Odin core (glonOdin/core/wire.odin, `wire` method):
 *   - kind 1078 events, authored by our own key.
 *   - content = NIP-44 self-encryption (conversation key of sk with our own
 *     pk) of the base64 of the raw Change protobuf bytes.
 *   - 'h' tag = blinded object id: sha256(sk || objectId utf8) hex[0:16].
 *   - big changes are split into ≤40k-char base64 parts carried in
 *     ['c', groupId, index, total] tags; groupId = sha256(b64) hex[0:16].
 *   - keyring events (kind 30078, d='roostr-keyring') are NOT handled in
 *     v1 — desktop remains the keyring authority for now.
 * The host keeps secp256k1: event signing, signature verification and the
 * ECDH shared secret. Receiving (decrypt, chunk reassembly, cursor, replay
 * bookkeeping) is the core's `sync` session (glonOdin/core/sync_session.odin);
 * this class feeds it verified events and persists what it reports.
 * Publishing is the same session's outbox (glonOdin/core/sync_outbox.odin):
 * the core owns queue order, dedupe, the rotated-key rule, backoff and
 * seals a change on its first attempt; this class signs the sealed parts,
 * persists the exact signed events, sends them and reports the outcome.
 *
 * History sync is NIP-77 (Negentropy) per relay and stream filter: the
 * events this device holds (store `relayEvents`) are reconciled against the
 * relay's set, every needed id is fetched with REQ {ids} in batches and
 * imported. A relay that refuses or ignores NEG-OPEN gets the paged walk
 * instead: querySync backwards via `until` (since cursor+1), paced between
 * pages so public relays don't rate-limit us. Live is a subscribeMany since
 * cursor+1, which never carries an event a relay accepted at or before the
 * cursor: a tab back from the background or a network that returns
 * reconnects and reconciles (`resume`). Sends are paced one event per
 * PUBLISH_SPACING_MS.
 *
 * State sync (glonOdin/docs/state-sync.md): every pass covers bases first
 * (kind 31078 + their kind-1080 parts by `#c`), so each object renders from
 * its base as soon as it lands; then legacy 1079 checkpoints; then deltas.
 * With no base on the relays (a pre-migration vault) the kind-1078 history is
 * covered whole, exactly as before (dual read). Once bases exist, deltas are
 * the live deltas of the held current bases (`#b`) plus a 1078 cover bounded
 * to the newest base's created_at minus one day - never history from zero.
 * Local deltas carry the base they were written on (`b`); orphans of this
 * identity's writable objects are rebased by the core and published. The
 * web app never compacts.
 */

import { SimplePool, finalizeEvent, getPublicKey, nip19, nip44, verifyEvent, type Event, type Filter } from "nostr-tools";
import { unwrapEvent, wrapEvent } from "nostr-tools/nip59";
import type { AbstractRelay } from "nostr-tools/abstract-relay";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { BaseRow, ChangeJSON, ChangeStoreApi, CheckpointRow, PendingPublish, RelayEventRow, RelaySyncApi, SharedProvenance, SyncEvents } from "./contracts";
import type { ObjectJSON } from "$lib/types";
import { pickCurrentBase, rebaseObject, replayObject, type Rebased } from "./replay";
import { CoreError, coreCall } from "./core";
import { unpackCoreValueMaps } from "./core-values";
import { base64ToBytes, bytesToBase64, decodeChange } from "./proto";
import { loadKey } from "./keys";
import { Negentropy, NegentropyStorage } from "./negentropy";
import { spaceKeyGet, spaceKeyImport } from "./spacekeys";

export const DEFAULT_RELAYS = ["wss://roostr-relay.fly.dev"];

const CHANGE_KIND = 1078;
/** Kind-1079 checkpoint: one sealed Checkpoint protobuf per object, a replay cache (docs/checkpoint-sync.md). */
const CHECKPOINT_KIND = 1079;
/** Kind-31078 base (addressable) and its kind-1080 parts 1..n-1 (docs/state-sync.md). */
const BASE_KIND = 31078;
const BASE_PART_KIND = 1080;
/** Every relay filter that pulls DAG events; the core session ingests all four kinds. */
const DAG_KINDS = [CHANGE_KIND, CHECKPOINT_KIND, BASE_KIND, BASE_PART_KIND];
/** NIP-09 deletion. On a space stream it is an h-deletion: from the space's
 * owner, the signal that the space was deleted for everyone. */
const DELETION_KIND = 5;
/** Every #h filter of a shared space: its DAG events plus h-deletions. */
const SPACE_KINDS = [...DAG_KINDS, DELETION_KIND];
/** Base hashes per `#b` delta filter. */
export const BASE_HASH_BATCH = 200;
/** Once bases exist, 1078 history reaches back to the newest base's created_at minus this. */
const BASE_LOOKBACK_S = 86_400;
/** The synced vanish ledger object (src/vanish.odin). It never gets a base, so its stream is always read whole. */
export const VANISH_LOG_ID = "__vanished__";
/** A device with no cursor yet subscribes live from this long before now (history covers the rest). */
const LIVE_COLD_START_LOOKBACK_S = 300;
/** Spaces whose h-deletion a relay accepted: published once per space. */
const SPACE_DELETIONS_STORAGE = "roostr-space-deletions-sent";
const ALLOWLIST_KIND = 30100;
const ALLOWLIST_D = "roostr-allowlist";
/** NIP-59 gift wrap: space-key invites and join requests, addressed by npub. */
const WRAP_KIND = 1059;
const INVITE_RUMOR_KIND = 24891;
const JOINREQ_RUMOR_KIND = 24892;
/** Gift wraps randomize created_at up to ~2 days back; look back further. */
const WRAP_LOOKBACK_S = 3 * 86_400;

// ── Join requests (this device is a space admin) ─────────────────────

export interface JoinRequest {
	/** "spaceId/requesterHex" - stable dedupe key. */
	key: string;
	space: string;
	spaceName: string;
	requester: string;
	requesterNpub: string;
	/** kind-0 profile at request time, when the relays had one. */
	name?: string;
	picture?: string;
	at: number;
}

const JOINREQ_STORAGE = "roostr-join-requests";

export function listJoinRequests(): JoinRequest[] {
	if (typeof localStorage === "undefined") return [];
	try {
		return (JSON.parse(localStorage.getItem(JOINREQ_STORAGE) ?? "[]") as JoinRequest[]) ?? [];
	} catch {
		return [];
	}
}

function writeJoinRequests(requests: JoinRequest[]): void {
	localStorage.setItem(JOINREQ_STORAGE, JSON.stringify(requests));
	window.dispatchEvent(new CustomEvent("roostr-join-requests"));
}

export function clearJoinRequest(key: string): void {
	writeJoinRequests(listJoinRequests().filter((r) => r.key !== key));
}

function recordJoinRequest(r: Omit<JoinRequest, "key">): void {
	const key = `${r.space}/${r.requester}`;
	const rest = listJoinRequests().filter((x) => x.key !== key);
	rest.push({ ...r, key });
	writeJoinRequests(rest);
}

/** Send a join request to a space owner - used by the public /j page. */
export async function sendJoinRequest(link: { space: string; owner: string; relays: string[] }): Promise<void> {
	const key = loadKey();
	if (!key) throw new Error("no local identity - open the app once first");
	const ownerHex = npubToHex(link.owner);
	if (!ownerHex) throw new Error("bad owner key in the link");
	const relays = link.relays.length > 0 ? link.relays : DEFAULT_RELAYS;
	const pool = new SimplePool();
	try {
		const wrap = wrapEvent(
			{ kind: JOINREQ_RUMOR_KIND, tags: [], content: JSON.stringify({ t: "join-request", space: link.space }) },
			key.sk,
			ownerHex,
		);
		await Promise.any(pool.publish(relays, wrap));
	} finally {
		pool.close(relays);
	}
}

// ── Shared spaces ────────────────────────────────────────────────────
//
// Wire-compatible with the desktop daemon: a shared space syncs under
// its 32-byte space key as the NIP-44 conversation key; tags are
// blinded (spaceKey, objectId) plus a space-stream tag blinded
// (spaceKey, "space:"+spaceId) so a joiner pulls the whole space with
// one #h filter. Writers = owner + non-viewer members; anything else
// is dropped on receipt.

export interface SharedSpaceInfo {
	spaceId: string;
	keyHex: string;
	keyId: number;
	/** hex pubkeys allowed to author events. */
	writers: string[];
	owner?: string;
}

interface SharedSpace extends SharedSpaceInfo {
	spaceTag: string;
	writerSet: Set<string>;
}

/**
 * Pure authority gate, shared with the daemon and iOS through the core's
 * `sync`/`authorize` method. Existing scope and privileges never come from
 * the candidate. A malformed change or state is a rejection, not a fault.
 */
export function authorizeSharedChange(change: ChangeJSON, provenance: SharedProvenance, space: SharedSpaceInfo,
	localPk: string, trustedSpace: ObjectJSON | null, existing: ObjectJSON | null): boolean {
	return authorizeShared({ action: "authorize", change }, provenance, space, localPk, trustedSpace, existing);
}

/** Same gate for a checkpoint: only the space owner may checkpoint a shared object. `checkpoint` is the base64 protobuf. */
export function authorizeSharedCheckpoint(checkpoint: string, provenance: SharedProvenance, space: SharedSpaceInfo,
	localPk: string, trustedSpace: ObjectJSON | null, existing: ObjectJSON | null): boolean {
	return authorizeShared({ action: "authorizeCheckpoint", checkpoint }, provenance, space, localPk, trustedSpace, existing);
}

/** Same gate for a base (core `authorizeBase`): owner only, epoch >= 1. `base` is the base64 protobuf. */
export function authorizeSharedBase(base: string, provenance: SharedProvenance, space: SharedSpaceInfo,
	localPk: string, trustedSpace: ObjectJSON | null, existing: ObjectJSON | null): boolean {
	return authorizeShared({ action: "authorizeBase", base }, provenance, space, localPk, trustedSpace, existing);
}

function authorizeShared(candidate: Record<string, unknown>, provenance: SharedProvenance, space: SharedSpaceInfo,
	localPk: string, trustedSpace: ObjectJSON | null, existing: ObjectJSON | null): boolean {
	try {
		return coreCall<{ ok: boolean; reason: string }>("sync", {
			...candidate,
			provenance,
			space: { spaceId: space.spaceId, keyId: space.keyId, owner: space.owner || localPk },
			trustedSpace,
			existing,
		}).ok;
	} catch (err) {
		if (err instanceof CoreError && err.code === "domain") return false;
		throw err;
	}
}

/** What the core session reports about a decoded kind-1079 payload. */
interface CheckpointSummary {
	objectId: string;
	headIds: string[];
	hash: string;
}

/** What the core session reports about a completed base (kind 31078 + its 1080 parts). */
interface BaseSummary {
	objectId: string;
	hash: string;
	epoch: number;
	prevBase: string;
	/** Unix ms. */
	createdAt: number;
	headIds: string[];
	covered: number;
}

/** One decrypted relay payload: a change (kind 1078), a checkpoint (kind 1079) or a base (kind 31078). */
interface ImportItem {
	objectId: string;
	bytes: Uint8Array;
	/** `bytes` as the core handed them over; the checkpoint/base gate takes them back verbatim. */
	b64: string;
	change?: ChangeJSON;
	checkpoint?: CheckpointSummary;
	base?: BaseSummary;
	provenance?: SharedProvenance;
	chunkKey?: string;
	/** Relay events this payload came from (every part of a chunk group), recorded once it imports. */
	held?: RelayEventRow[];
	/** Host identity of the chunk group (see `partKey`), remembered once it imports. */
	partKey?: string;
}

export function blindShared(keyHex: string, id: string): string {
	return coreCall<string>("wire", { action: "blind", keyHex, id });
}

interface SyncSessionState {
	cursor: number;
	/** Open (partially assembled) chunk groups. */
	groups: number;
	replayGroups: Array<[string, number]>;
	replayFloor?: number;
}

interface IngestResult {
	cursor: number;
	item?: { bytes: string; change?: unknown; b?: string; checkpoint?: CheckpointSummary; base?: BaseSummary; chunkKey?: string; provenance?: SharedProvenance };
	faultAt?: number;
	replayGroups?: Array<[string, number]>;
	decryptFailure?: boolean;
	decodeFailure?: boolean;
	hTag?: string;
	/** A kind-5 h-deletion by the owner of this installed space. */
	spaceVanished?: string;
}

/** The core's receive session is process-global and single-flight; only its
 * owner may feed or close it, so a newer instance silently retires an older one. */
let sessionOwner: RelaySync | null = null;

export function npubToHex(npub: string): string | null {
	try {
		const d = nip19.decode(npub.trim());
		if (d.type === "npub") return d.data as string;
	} catch {
		/* not bech32 */
	}
	const t = npub.trim().toLowerCase();
	return /^[0-9a-f]{64}$/.test(t) ? t : null;
}

/** This device's npub, or null before key setup. */
export function myNpub(): string | null {
	const key = loadKey();
	return key ? nip19.npubEncode(getPublicKey(key.sk)) : null;
}

/** Accept an invite link: store the space key; the next app boot (or
 * shared-space refresh) backfills the space from the relays. */
export function importSpaceInvite(inv: { space: string; owner: string; key: string; keyId: number }): boolean {
	const ownerHex = npubToHex(inv.owner);
	if (!ownerHex || !/^[0-9a-f]{64}$/.test(inv.key) || !inv.space) return false;
	if (!Number.isSafeInteger(inv.keyId) || inv.keyId < 1) return false;
	const previous = spaceKeyGet(inv.space);
	const local = loadKey();
	if (previous && (previous.owner || local?.pk) !== ownerHex) return false;
	spaceKeyImport(inv.space, inv.key, inv.keyId || 1, ownerHex);
	return true;
}
const PUBLISH_SPACING_MS = 120;
const PAGE_SPACING_MS = 400;
/** Floor for the adaptive page delay: enough to interleave, not enough to stall a cold load. */
const MIN_PAGE_SPACING_MS = 40;
// Keep full-sized 40k-character encrypted chunks inside the 8 MiB relay budget.
export const PAGE_LIMIT = 128;
/** Ids per REQ {ids} fetch after a reconcile: checkpoint parts run ~40k chars, so stay well inside the 8 MiB budget. */
export const FETCH_BATCH = 100;
/** Ids per fetch for a checkpoint stream: each event can be ~40k chars. */
export const CHECKPOINT_FETCH_BATCH = 8;
/** Negentropy frame cap in bytes; hex doubles it, keeping each NEG-MSG under the relay's 1 MiB message cap. */
const NEG_FRAME_LIMIT = 250_000;
/** No NEG-MSG within this window: before the first answer the relay is taken not to speak NIP-77. */
const NEG_TIMEOUT_MS = 15_000;
const NOTIFY_DEBOUNCE_MS = 100;
/** A tab hidden at least this long may hold dead sockets: showing it again resumes sync. */
const RESUME_AFTER_HIDDEN_MS = 30_000;

function sleep(ms: number): Promise<void> {
	const { promise, resolve } = Promise.withResolvers<void>();
	setTimeout(resolve, ms);
	return promise;
}

/** A reconcile that never got a NEG-MSG answer: the caller walks this filter instead. */
class NegentropyFallback extends Error {
	/** The relay said no (NEG-ERR, NOTICE) or stayed silent: don't ask it again this session. */
	constructor(message: string, readonly unsupported: boolean) {
		super(message);
	}
}

/** NOTICE is per connection, not per subscription: every reconcile on a relay listens through one hook. */
const noticeListeners = new WeakMap<AbstractRelay, Set<(notice: string) => void>>();

/** One relay stream filter the history sync covers, with the store scope of its held events. */
interface StreamFilter {
	scope: string;
	filter: Filter;
}

export interface RelaySyncOptions {
	/** A gift-wrapped space key arrived and was imported. */
	onSpaceKey?: () => void;
	/** Debug hook: every raw relay event before decrypt. */
	onRawEvent?: (event: Event) => void;
	/** objectId -> owning space id ("" = personal). Channels own themselves. */
	spaceOf?: (objectId: string) => string;
	/** The owner of an installed shared space deleted it for everyone (their kind-5 h-deletion). */
	onSpaceVanished?: (spaceId: string) => void;
	/** The space is vanished or left here: its invite keys are ignored. */
	spaceGone?: (spaceId: string) => boolean;
}

export interface SyncStats {
	events: number;
	decryptFailures: number;
	decodeFailures: number;
	imported: number;
	/** Checkpoints accepted into the store (superseding or first). */
	checkpoints: number;
	/** Bases newly held. */
	bases: number;
	/** Orphans rebased onto their object's current base. */
	rebased: number;
	blindedTags: Set<string>;
}

interface SealedParts {
	gid: string;
	parts: Array<{ content: string; tags: string[][] }>;
}

/** One publish obligation handed out by the core outbox; `sealed` only until the host reports `sealed: true`. */
interface OutboxItem {
	key: string;
	objectId: string;
	changeId: string;
	attempts: number;
	spaceId?: string;
	keyId?: number;
	sealed?: SealedParts;
}

interface OutboxNext {
	item?: OutboxItem;
	waitMs: number;
	pending: number;
}

export class RelaySync implements RelaySyncApi {
	readonly stats: SyncStats = { events: 0, decryptFailures: 0, decodeFailures: 0, imported: 0, checkpoints: 0, bases: 0, rebased: 0, blindedTags: new Set() };

	private readonly pk: string;
	private pool = new SimplePool();
	/** NIP-44 self conversation key, hex, as the core's `wire` method takes it. */
	private readonly conversationKey: string;
	private readonly secretHex: string;
	private readonly onRawEvent?: (event: Event) => void;
	private readonly onSpaceKey?: () => void;
	private wrapSub: { close(): void } | null = null;
	private readonly spaceOf: (objectId: string) => string;
	private readonly onSpaceVanished?: (spaceId: string) => void;
	private readonly spaceGone: (spaceId: string) => boolean;
	private sharedSpaces = new Map<string, SharedSpace>();
	private spaceSub: { close(): void } | null = null;

	private cursor = 0;
	private stopped = false;
	private sub: { close(): void } | null = null;
	private watchdogTimer: ReturnType<typeof setInterval> | null = null;
	private watchdogBusy = false;
	/** The resume pass in flight; concurrent calls share it. */
	private resuming: Promise<void> | null = null;
	/** Resume passes running: the dot says catching up, not live, until they finish. */
	private catchingUp = 0;
	/** When the tab was last hidden (ms); 0 while visible. */
	private hiddenAt = 0;
	private readonly onVisibility = (): void => {
		if (document.visibilityState === "hidden") {
			this.hiddenAt = Date.now();
			return;
		}
		if (this.hiddenAt && Date.now() - this.hiddenAt >= RESUME_AFTER_HIDDEN_MS) void this.resume();
		this.hiddenAt = 0;
	};
	private readonly onOnline = (): void => void this.resume();

	/** Host mirror of the core session's replay obligations: chunk key → earliest created_at. */
	private replayGroups = new Map<string, number>();
	private sessionOpening: Promise<void> | null = null;
	private importChain: Promise<void> = Promise.resolve();
	private historyComplete = false;
	/** A walk's checkpoint pass has run: every object has its cached state, history is still streaming in. */
	private checkpointsLoaded = false;
	private discardedChunkFloor = Infinity;
	private replayFaultGeneration = 0;
	private activeLiveEvents = 0;
	private activeImports = 0;
	/** Live subscriptions established (survives a long background history walk). */
	private liveUp = false;
	/** The first full-history walk is in flight; the watchdog must not queue another. */
	private bootstrapping = false;
	private cursorChain: Promise<void> = Promise.resolve();
	private backfillChain: Promise<boolean> = Promise.resolve(false);
	/** Mirror of the core outbox's queued-not-in-flight count, for the status dot. */
	private pendingCount = 0;
	private queueRunning = false;
	/** Relays that answered NEG-OPEN with NEG-ERR/NOTICE or silence: walked instead, for this session. */
	private readonly negentropyUnsupported = new Set<string>();
	/** Per relay: the reconcile negotiation in flight, so a pass opens one NEG session at a time. */
	private readonly negotiations = new Map<string, Promise<unknown>>();
	/** Ingested parts of chunk groups not yet imported, by `partKey`: recorded as held once the group imports. */
	private readonly pendingParts = new Map<string, RelayEventRow[]>();
	/** `partKey`s whose group imported here: a late duplicate part is held at once. */
	private readonly importedParts = new Set<string>();
	/** Chunk group ids of 31078 events whose 1080 parts have not all arrived: fetched by `#c`. */
	private readonly openBaseGroups = new Set<string>();
	/** Objects holding a base: only they can have orphans to rebase. */
	private basedObjects = new Set<string>();

	private readonly pendingObjects = new Set<string>();
	private notifyTimer: number | null = null;

	/** Serializes live event handling against store writes. */
	private liveChain: Promise<void> = Promise.resolve();

	constructor(
		private readonly sk: Uint8Array,
		private readonly relays: string[],
		private readonly store: ChangeStoreApi,
		private readonly events: SyncEvents,
		options: RelaySyncOptions = {},
	) {
		this.pk = getPublicKey(sk);
		this.conversationKey = bytesToHex(nip44.getConversationKey(sk, this.pk));
		this.secretHex = bytesToHex(sk);
		this.onRawEvent = options.onRawEvent;
		this.onSpaceKey = options.onSpaceKey;
		this.spaceOf = options.spaceOf ?? (() => "");
		this.onSpaceVanished = options.onSpaceVanished;
		this.spaceGone = options.spaceGone ?? (() => false);
	}

	private get sessionOpen(): boolean {
		return sessionOwner === this;
	}

	/** (Re)open the core receive session from this instance's cursor and the persisted obligations. */
	private async openSession(): Promise<void> {
		const [replayGroups, bases] = await Promise.all([this.store.getReplayGroups(), this.store.currentBases()]);
		this.basedObjects = new Set(bases.keys());
		const state = coreCall<SyncSessionState>("sync", {
			action: "session",
			pk: this.pk,
			conversationKey: this.conversationKey,
			secret: this.secretHex,
			// A keyring entry without an imported owner is this identity's own space.
			spaces: [...this.sharedSpaces.values()].map((sp) => ({ spaceId: sp.spaceId, keyHex: sp.keyHex, keyId: sp.keyId, owner: sp.owner || this.pk })),
			cursor: this.cursor,
			replayGroups,
		});
		sessionOwner = this;
		this.replayGroups = new Map(state.replayGroups);
	}

	/** Paths that run before start() (tests drive them directly) open the session on first need. */
	private ensureSession(): Promise<void> {
		if (this.sessionOpen || this.stopped) return Promise.resolve();
		return (this.sessionOpening ??= this.openSession().finally(() => {
			this.sessionOpening = null;
		}));
	}

	private closeSession(): void {
		if (!this.sessionOpen) return;
		coreCall("sync", { action: "close" });
		sessionOwner = null;
	}

	/** Replace the shared-space view. New spaces get a full backfill of
	 * their stream tag plus a live subscription. */
	setSharedSpaces(infos: SharedSpaceInfo[]): void {
		const prevTags = new Set([...this.sharedSpaces.values()].map((sp) => sp.spaceTag));
		const next = new Map<string, SharedSpace>();
		for (const info of infos) {
			if (!/^[0-9a-f]{64}$/.test(info.keyHex)) continue;
			next.set(info.spaceId, {
				...info,
				spaceTag: blindShared(info.keyHex, `space:${info.spaceId}`),
				writerSet: new Set(info.writers),
			});
		}
		this.sharedSpaces = next;
		if (this.sessionOpen) {
			coreCall("sync", { action: "spaces", spaces: [...next.values()].map((sp) => ({ spaceId: sp.spaceId, keyHex: sp.keyHex, keyId: sp.keyId, owner: sp.owner || this.pk })) });
		}
		const tags = [...next.values()].map((sp) => sp.spaceTag);
		const fresh = [...next.values()].some((sp) => !prevTags.has(sp.spaceTag));
		if (this.stopped || !this.sub) return; // start() wires subscriptions itself
		try {
			this.spaceSub?.close();
		} catch {
			/* already closed */
		}
		this.spaceSub = null;
		if (tags.length > 0) {
			this.spaceSub = this.pool.subscribeMany(this.relays, { kinds: SPACE_KINDS, "#h": tags, since: this.liveSince() }, {
				onevent: (event) => {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
				},
			});
		}
		// A newly joined space's whole stream is unseen. The walk is queued
		// synchronously so a second call in the same tick (the space list
		// re-rendering) cannot cancel it - it only ever adds another.
		if (fresh) void this.backfill(1).catch((err) => this.events.onStatus({ phase: "error", detail: String(err) }));
	}


	/**
	 * The dot answers "am I connected and is my work published" - historical
	 * verification runs in the background and belongs in the detail text, not
	 * in the phase. Green with an incomplete history is honest: live changes
	 * flow; the tooltip says history is still being verified.
	 */
	private statusDetail(): string | undefined {
		if (this.historyComplete) return undefined;
		return `${this.checkpointsLoaded ? "verifying full history in background" : "loading spaces"} · ${this.stats.imported} changes so far`;
	}

	private emitLiveStatus(): void {
		const live = this.liveUp && this.catchingUp === 0;
		this.events.onStatus({
			phase: live ? "live" : "backfill",
			imported: this.stats.imported,
			detail: !this.liveUp ? undefined : live ? this.statusDetail() : "catching up",
			pending: this.pendingCount,
		});
	}

	/** Owner duty: publish the relay write-allowlist. */
	async publishAllowlist(writers: string[]): Promise<void> {
		const others = writers.filter((w) => w !== this.pk);
		try {
			const event = finalizeEvent(
				{
					kind: ALLOWLIST_KIND,
					created_at: Math.floor(Date.now() / 1000),
					tags: [["d", ALLOWLIST_D], ...others.map((w) => ["p", w])],
					content: "",
				},
				this.sk,
			);
			await Promise.any(this.pool.publish(this.relays, event));
		} catch {
			/* retried on next refresh */
		}
	}

	private spaceDeletionsSent(): Set<string> {
		try {
			return new Set(JSON.parse(localStorage.getItem(SPACE_DELETIONS_STORAGE) ?? "[]") as string[]);
		} catch {
			return new Set();
		}
	}

	/**
	 * h-deletion of a vanished space: asks the relays to drop every event in
	 * the space this identity authored - and, from the owner, every event that
	 * names it as owner, which is also the signal members vanish the space on.
	 * Sent until a relay accepts it, then never again.
	 */
	async publishSpaceDeletion(spaceId: string, keyHex: string): Promise<void> {
		if (this.stopped || this.spaceDeletionsSent().has(spaceId)) return;
		try {
			const event = finalizeEvent(
				{
					kind: DELETION_KIND,
					created_at: Math.floor(Date.now() / 1000),
					tags: [["h", blindShared(keyHex, `space:${spaceId}`)], ...DAG_KINDS.map((kind) => ["k", String(kind)])],
					content: "space deleted",
				},
				this.sk,
			);
			await Promise.any(this.pool.publish(this.relays, event));
			// Re-read: another space's deletion may have been recorded meanwhile.
			const sent = this.spaceDeletionsSent();
			sent.add(spaceId);
			localStorage.setItem(SPACE_DELETIONS_STORAGE, JSON.stringify([...sent]));
		} catch {
			/* no relay accepted it; retried on next refresh */
		}
	}

	// ── Gift wraps: key invites in, join requests in, key invites out ──

	private wrapsSeen(): Set<string> {
		try {
			return new Set(JSON.parse(localStorage.getItem("roostr-wraps-seen") ?? "[]") as string[]);
		} catch {
			return new Set();
		}
	}

	private async handleWrap(event: Event): Promise<void> {
		const seen = this.wrapsSeen();
		if (seen.has(event.id)) return;
		seen.add(event.id);
		localStorage.setItem("roostr-wraps-seen", JSON.stringify([...seen].slice(-2000)));
		try {
			const rumor = unwrapEvent(event, this.sk);
			if (rumor.kind === INVITE_RUMOR_KIND) {
				const p = JSON.parse(rumor.content) as { t?: string; space?: string; name?: string; key?: string; keyId?: number };
				if (p.t !== "space-invite" || !p.space || !/^[0-9a-f]{64}$/.test(p.key ?? "")) return;
				if (!Number.isSafeInteger(p.keyId) || p.keyId! < 1) return;
				// A vanished or left space stays gone: a late key must not
				// replace the one its h-deletion is addressed with.
				if (this.spaceGone(p.space)) return;
				const previous = spaceKeyGet(p.space);
				if (previous && rumor.pubkey !== (previous.owner || this.pk)) return;
				spaceKeyImport(p.space, p.key!, typeof p.keyId === "number" && p.keyId > 0 ? p.keyId : 1, rumor.pubkey);
				this.onSpaceKey?.();
			} else if (rumor.kind === JOINREQ_RUMOR_KIND) {
				const p = JSON.parse(rumor.content) as { t?: string; space?: string };
				if (p.t !== "join-request" || !p.space) return;
				// Only the administrator of the space collects requests.
				const space = this.sharedSpaces.get(p.space);
				if (!space || (space.owner && space.owner !== this.pk)) return;
				// The requester's public kind-0 profile, so the owner can put a
				// face to the knock before approving.
				let profile: { name?: string; picture?: string } = {};
				try {
					const events = await this.pool.querySync(this.relays, { kinds: [0], authors: [rumor.pubkey], limit: 3 });
					events.sort((a, b) => b.created_at - a.created_at);
					if (events[0]) {
						const meta = JSON.parse(events[0].content) as { name?: string; display_name?: string; picture?: string };
						profile = { name: meta.display_name || meta.name, picture: meta.picture };
					}
				} catch {
					/* no profile on our relays - npub alone */
				}
				recordJoinRequest({
					space: p.space,
					spaceName: "",
					requester: rumor.pubkey,
					requesterNpub: nip19.npubEncode(rumor.pubkey),
					name: profile.name,
					picture: profile.picture,
					at: Date.now(),
				});
			}
		} catch {
			/* not ours / garbled */
		}
	}

	/** Owner duty: gift-wrap the current space key to every member that
	 * hasn't received this keyId yet. Idempotent per (member, keyId). */
	async sendInviteWraps(spaceId: string, keyHex: string, keyId: number, name: string, memberHexes: string[]): Promise<void> {
		let sent: Record<string, number> = {};
		try {
			sent = JSON.parse(localStorage.getItem("roostr-invites-sent") ?? "{}") as Record<string, number>;
		} catch {
			/* fresh */
		}
		for (const hex of memberHexes) {
			if (hex === this.pk) continue;
			const k = `${spaceId}/${hex}`;
			if (sent[k] === keyId) continue;
			try {
				const wrap = wrapEvent(
					{ kind: INVITE_RUMOR_KIND, tags: [], content: JSON.stringify({ t: "space-invite", space: spaceId, name, key: keyHex, keyId }) },
					this.sk,
					hex,
				);
				await Promise.any(this.pool.publish(this.relays, wrap));
				sent[k] = keyId;
				localStorage.setItem("roostr-invites-sent", JSON.stringify(sent));
			} catch {
				/* relay refused; retried on next reconcile */
			}
		}
	}

	async start(): Promise<void> {
		await this.store.open();
		// A device that walked from a manifest floor never held the older
		// history: this also clears its bootstrapped flag, so it walks from zero.
		await this.store.forgetCheckpointFloors();
		this.cursor = await this.store.getCursor();
		await this.openSession();
		for (const pending of await this.store.pendingPublishes()) this.offerToOutbox(pending);
		this.events.onStatus({ phase: "backfill", imported: 0 });

		// The incremental `since = cursor+1` shortcut is only sound once ONE
		// full history walk has completed on this device: the cursor tracks
		// the NEWEST imported event, so an interrupted or partially-failed
		// first bootstrap would otherwise skip everything older, forever.
		//
		// The walk runs in the BACKGROUND. Awaiting it here used to hold the
		// first green dot hostage to a full clean pass over all history - on a
		// phone that is minutes of orange, and a screen lock meant starting
		// over. Live subscriptions come up first (imports dedupe against the
		// walk's pages), the dot goes green as soon as they do, and the walk's
		// progress persists page by page via the bootstrap floor.
		const bootstrapped = await this.store.getBootstrapped();
		const floor = bootstrapped ? undefined : await this.store.getBootstrapFloor();
		this.bootstrapping = true;
		void this.backfill(bootstrapped ? this.cursor + 1 : 1, floor)
			.then(async (complete) => {
				if (this.stopped) return; // a stopped engine must not emit one last stale status
				if (!bootstrapped && complete) await this.store.setBootstrapped();
				this.emitLiveStatus();
			})
			.catch((err) => {
				if (this.stopped) return; // stop() closes the pool under the walk; that is not an error
				this.events.onStatus({ phase: "error", detail: err instanceof Error ? err.message : String(err) });
			})
			.finally(() => {
				this.bootstrapping = false;
			});

		if (this.stopped) return;
		// Gift wraps addressed to us: created_at is randomized, so no
		// cursor - the seen-set dedupes.
		try {
			const wraps = await this.pool.querySync(this.relays, { kinds: [WRAP_KIND], "#p": [this.pk] });
			wraps.sort((a, b) => a.created_at - b.created_at);
			for (const w of wraps) await this.handleWrap(w);
		} catch {
			/* relay unreachable; live sub catches up */
		}
		this.subscribeLive();
		this.watchdogTimer = setInterval(() => void this.watchdog(), 60_000);
		if (typeof document !== "undefined") document.addEventListener("visibilitychange", this.onVisibility);
		if (typeof window !== "undefined") window.addEventListener("online", this.onOnline);
		this.emitLiveStatus();
	}

	/**
	 * The tab came back from the background or the network returned:
	 * sockets that outlived a suspension can be half-open, and the live
	 * subscription (since cursor+1) never carries an event a relay accepted
	 * at or before the cursor - another device publishing late or with a
	 * skewed clock, the same second, or one a dead socket dropped while
	 * another relay moved the cursor on. Drop the sockets, resubscribe, then
	 * run a history pass under start's rules: it reconciles every stream's
	 * whole set (NIP-77) and imports whatever this device lacks. Concurrent
	 * calls share one pass; a no-op until start() has the live subscriptions up.
	 */
	resume(): Promise<void> {
		if (this.stopped || !this.liveUp) return Promise.resolve();
		return (this.resuming ??= this.reconnect().finally(() => {
			this.resuming = null;
		}));
	}

	private async reconnect(): Promise<void> {
		this.catchingUp++;
		this.emitLiveStatus();
		try {
			try {
				this.pool.close(this.relays);
			} catch {
				/* closed */
			}
			this.subscribeLive();
			const bootstrapped = await this.store.getBootstrapped();
			const complete = await this.catchup(bootstrapped ? this.cursor + 1 : 1, bootstrapped ? undefined : await this.store.getBootstrapFloor());
			if (complete && !bootstrapped && !this.stopped) await this.store.setBootstrapped();
		} catch (err) {
			if (!this.stopped) this.events.onStatus({ phase: "error", detail: err instanceof Error ? err.message : String(err) });
			return;
		} finally {
			this.catchingUp--;
		}
		if (!this.stopped) this.emitLiveStatus();
	}

	/**
	 * Live subscriptions start after the cursor. A device that has imported
	 * nothing yet (cursor 0) starts them shortly before now instead of at
	 * event zero: the history pass covers everything older - bounded once
	 * bases exist - and live must not walk the relay's whole 1078 history.
	 */
	private liveSince(): number {
		return this.cursor > 0 ? this.cursor + 1 : Math.floor(Date.now() / 1000) - LIVE_COLD_START_LOOKBACK_S;
	}

	private subscribeLive(): void {
		try {
			this.sub?.close();
		} catch {
			/* gone */
		}
		try {
			this.spaceSub?.close();
		} catch {
			/* gone */
		}
		this.sub = this.pool.subscribeMany(
			this.relays,
			{ kinds: DAG_KINDS, authors: [this.pk], since: this.liveSince() },
			{
				onevent: (event) => {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
				},
			},
		);
		const spaceTags = [...this.sharedSpaces.values()].map((sp) => sp.spaceTag);
		this.spaceSub = spaceTags.length > 0
			? this.pool.subscribeMany(this.relays, { kinds: SPACE_KINDS, "#h": spaceTags, since: this.liveSince() }, {
					onevent: (event) => {
						this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
					},
				})
			: null;
		try {
			this.wrapSub?.close();
		} catch {
			/* gone */
		}
		this.wrapSub = this.pool.subscribeMany(this.relays, { kinds: [WRAP_KIND], "#p": [this.pk], since: Math.floor(Date.now() / 1000) - WRAP_LOOKBACK_S }, {
			onevent: (event) => {
				this.liveChain = this.liveChain.then(() => this.handleWrap(event)).catch(() => {});
			},
		});
		this.liveUp = true;
	}

	/**
	 * Deafness watchdog: subscriptions do not survive socket drops and
	 * nostr-tools reports nothing when they die - the dot would stay
	 * green forever. Compare the relay's head against our cursor; when
	 * we are behind (or the relay stops answering), surface an honest
	 * "catching up" status, recover, and go live again.
	 */
	private async watchdog(): Promise<void> {
		if (this.stopped || this.watchdogBusy || this.bootstrapping || this.resuming) return;
		this.watchdogBusy = true;
		try {
			const groups = this.sessionOpen ? coreCall<SyncSessionState>("sync", { action: "state" }).groups : 0;
			if (!this.historyComplete || groups > 0 || this.discardedChunkFloor !== Infinity) {
				const complete = await this.backfill(
					(await this.store.getBootstrapped()) ? await this.store.getCursor() : 1,
					(await this.store.getBootstrapped()) ? undefined : await this.store.getBootstrapFloor(),
				);
				if (complete) await this.store.setBootstrapped();
				this.subscribeLive();
				this.emitLiveStatus();
				return;
			}
			const spaceTags = [...this.sharedSpaces.values()].map((sp) => sp.spaceTag);
			const query = Promise.all([
				this.pool.querySync(this.relays, { kinds: DAG_KINDS, authors: [this.pk], limit: 1 }),
				spaceTags.length > 0
					? this.pool.querySync(this.relays, { kinds: DAG_KINDS, "#h": spaceTags, limit: 1 })
					: Promise.resolve([] as Event[]),
			]);
			const res = await Promise.race([query, new Promise<null>((r) => setTimeout(() => r(null), 15_000))]);
			if (this.stopped) return;
			if (res === null) {
				this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: "relay unresponsive - reconnecting" });
				try {
					this.pool.close(this.relays);
				} catch {
					/* closed */
				}
				this.pool = new SimplePool();
				await this.catchup(this.cursor + 1);
				this.subscribeLive();
				this.emitLiveStatus();
				return;
			}
			const head = [...res[0], ...res[1]].reduce((max, e) => Math.max(max, e.created_at), 0);
			if (head > this.cursor) {
				this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: "catching up" });
				await this.catchup(this.cursor + 1);
				this.subscribeLive();
				this.emitLiveStatus();
			}
		} catch (err) {
			this.historyComplete = false;
			this.events.onStatus({ phase: "error", detail: String(err) });
		} finally {
			this.watchdogBusy = false;
		}
	}

	/** One history pass, then every gift wrap again; resolves with the pass's completeness. */
	private async catchup(since: number, resumeUntil?: number): Promise<boolean> {
		const complete = await this.backfill(since, resumeUntil);
		// Gift wraps have randomized created_at: re-query on every catchup;
		// the seen-set dedupes. Recovers knocks lost to dropped sockets.
		try {
			const wraps = await this.pool.querySync(this.relays, { kinds: [WRAP_KIND], "#p": [this.pk] });
			wraps.sort((a, b) => a.created_at - b.created_at);
			for (const w of wraps) await this.handleWrap(w);
		} catch {
			/* next watchdog tick */
		}
		return complete;
	}

	stop(): void {
		this.stopped = true;
		this.liveUp = false;
		this.closeSession();
		if (this.watchdogTimer) {
			clearInterval(this.watchdogTimer);
			this.watchdogTimer = null;
		}
		if (typeof document !== "undefined") document.removeEventListener("visibilitychange", this.onVisibility);
		if (typeof window !== "undefined") window.removeEventListener("online", this.onOnline);
		// nostr-tools can race an in-flight REQ against connection teardown;
		// swallow so a stop() never throws into the caller.
		try {
			this.sub?.close();
		} catch {
			/* already closed */
		}
		this.sub = null;
		try {
			this.spaceSub?.close();
		} catch {
			/* already closed */
		}
		this.spaceSub = null;
		if (this.notifyTimer) {
			clearTimeout(this.notifyTimer);
			this.notifyTimer = null;
		}
		try {
			this.pool.close(this.relays);
		} catch {
			/* already closed */
		}
	}

	// ── Backfill ───────────────────────────────────────────────────

	/**
	 * Returns true only when EVERY relay covered every stream: reconciled with
	 * every needed event imported, or (no NIP-77) walked to exhaustion. A
	 * reconcile always covers the whole stream; `since`/`resumeUntil` only
	 * shape the fallback walk.
	 */
	private backfill(since: number, resumeUntil?: number): Promise<boolean> {
		const run = this.backfillChain.then(async () => {
			try {
				return await this.walkHistory(since, resumeUntil);
			} catch (err) {
				this.recordReplayFault(1);
				await this.persistCursor();
				throw err;
			}
		});
		this.backfillChain = run.catch(() => false);
		return run;
	}

	private async walkHistory(since: number, resumeUntil?: number): Promise<boolean> {
		this.historyComplete = false;
		await this.ensureSession();
		// A full-history walk persists its progress page by page: a phone that
		// suspends mid-bootstrap resumes from the floor instead of restarting
		// from event zero. The floor is only meaningful while bootstrapping -
		// completion clears it and any replay fault discards it (a fault means
		// a suspect range that must be re-covered, not skipped).
		const trackFloor = since <= 1;
		let coveredUntil: number | undefined = resumeUntil;
		// An unresolved group may be missing fragments older than any observed
		// part. Only actual repair scans need full history; healthy live assembly
		// removes the identity without making the ordinary cursor sticky.
		since = Math.min(this.replayGroups.size > 0 ? 0 : since, this.discardedChunkFloor);
		// The persisted cursor is also the durable replay floor. Keep this
		// obligation until a covering scan imports every page without new faults.
		this.discardedChunkFloor = Math.min(this.discardedChunkFloor, since);
		const checkpoint = this.replayFaultGeneration;
		await this.persistCursor();
		// Imported page by page, NOT buffered until the walk ends. A phone
		// loses the tab to a lock screen or a memory reclaim mid-walk; a
		// whole-history buffer meant every event fetched so far was thrown
		// away, so the next load restarted and each device ended up with a
		// different arbitrary slice of the vault (one browser showing one
		// set of spaces, another showing a different set). Per-page import
		// also caps peak memory at one page instead of the whole history.
		const seen = new Set<string>();
		const importPage = async (page: Event[]): Promise<void> => {
			const batch: ImportItem[] = [];
			// Checkpoints first within a page: an object renders from its cache
			// while the changes it folds in stream in behind it.
			for (const event of [...page].sort((a, b) => a.created_at - b.created_at || b.kind - a.kind || a.id.localeCompare(b.id))) {
				if (seen.has(event.id)) continue;
				seen.add(event.id);
				const item = await this.ingestEvent(event);
				if (item) batch.push(item);
			}
			if (batch.length > 0) await this.importBatch(batch, true);
		};
		let complete = this.relays.length > 0;
		// Some relay/filter fell back to the paged walk: coverage then only
		// reaches back to `since`. A reconcile always covers the whole stream.
		let walked = false;
		// Bases (31078) come FIRST: every object renders from its base as soon
		// as it lands; a split base's 1080 parts follow by `#c` right after.
		// Checkpoints (1079, legacy) are replay caches covered next, so objects
		// without a base render early too. Without any base, the kind-1078
		// history - including the vanish ledger - is what this device must
		// hold in full (docs/checkpoint-sync.md); once bases exist it is their
		// live deltas plus a bounded window (docs/state-sync.md). The base and
		// checkpoint passes are short and re-walked whole on every resume, so
		// they neither start from nor move the bootstrap floor.
		const baseFilters: StreamFilter[] = [{ scope: "", filter: { kinds: [BASE_KIND], authors: [this.pk] } }];
		const checkpointFilters: StreamFilter[] = [{ scope: "", filter: { kinds: [CHECKPOINT_KIND], authors: [this.pk] } }];
		const changeFilters: StreamFilter[] = [{ scope: "", filter: { kinds: [CHANGE_KIND], authors: [this.pk] } }];
		for (const sp of this.sharedSpaces.values()) {
			baseFilters.push({ scope: sp.spaceTag, filter: { kinds: [BASE_KIND], "#h": [sp.spaceTag] } });
			checkpointFilters.push({ scope: sp.spaceTag, filter: { kinds: [CHECKPOINT_KIND, DELETION_KIND], "#h": [sp.spaceTag] } });
			changeFilters.push({ scope: sp.spaceTag, filter: { kinds: [CHANGE_KIND], "#h": [sp.spaceTag] } });
		}
		const walk = async (relay: string, filter: Filter, resume: number | undefined, track: boolean): Promise<void> => {
			let until: number | undefined = resume;
			for (;;) {
				if (this.stopped) { complete = false; return; }
				// A relay that fails, stalls, or saturates is this walk's
				// problem: note it and stop covering this filter. A STORE
				// failure is the caller's problem - it must propagate so the
				// recovery obligation outlives the scan, so the import below
				// deliberately sits outside this guard.
				let page: Event[];
				const pageStarted = Date.now();
				try {
					page = await this.queryRelayPage(relay, { ...filter, since: Math.max(since, filter.since ?? 0), until, limit: PAGE_LIMIT });
				} catch (err) {
					complete = false;
					this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 100)}` });
					return;
				}
				await importPage(page);
				this.emitLiveStatus();
				if (page.length < PAGE_LIMIT) return;
				const oldest = Math.min(...page.map((e) => e.created_at));
				if (until !== undefined && oldest >= until) {
					complete = false;
					this.events.onStatus({ phase: "backfill", detail: `${relay}: saturated same-timestamp history page` });
					return;
				}
				until = oldest;
				// Coverage is only as deep as the slowest concurrent walker.
				if (track) {
					coveredUntil = coveredUntil === undefined ? until : Math.min(coveredUntil, until);
					await this.store.setBootstrapFloor(coveredUntil);
				}
				// Politeness paced against the relay's own cost, not a flat
				// 400ms. A 12k-change vault is ~97 pages per filter: a fixed
				// delay spent 39 seconds of a cold phone's first load doing
				// nothing at all. Never idle longer than the page took, and
				// never longer than the old ceiling.
				await sleep(Math.min(PAGE_SPACING_MS, Math.max(MIN_PAGE_SPACING_MS, Date.now() - pageStarted)));
			}
		};
		const claimed = new Set<string>();
		const cover = (filters: StreamFilter[], resume: number | undefined, track: boolean) => Promise.all(this.relays.flatMap((relay) => filters.map(async ({ scope, filter }) => {
			if (!this.negentropyUnsupported.has(relay)) {
				const outcome = await this.reconcileStream(relay, scope, filter, claimed, importPage);
				if (outcome !== "unsupported") {
					if (outcome === "incomplete") complete = false;
					return;
				}
			}
			walked = true;
			await walk(relay, filter, resume, track);
		})));
		await cover(baseFilters, undefined, false);
		// A split base's 31078 names its parts' group; fetch the parts it still
		// lacks. One still open after this is re-asked when the next reconcile
		// fetches its 31078 again (its events are only held once it imports).
		const gids = [...this.openBaseGroups];
		this.openBaseGroups.clear();
		await Promise.all(this.relays.map(async (relay) => {
			for (let i = 0; i < gids.length; i += CHECKPOINT_FETCH_BATCH) {
				if (this.stopped) { complete = false; return; }
				let page: Event[];
				try {
					page = await this.queryRelayPage(relay, { kinds: [BASE_PART_KIND], "#c": gids.slice(i, i + CHECKPOINT_FETCH_BATCH) });
				} catch (err) {
					complete = false;
					this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 100)}` });
					return;
				}
				await importPage(page);
			}
		}));
		await cover(checkpointFilters, undefined, false);
		this.checkpointsLoaded = true;
		this.emitLiveStatus();
		const bases = [...(await this.store.currentBases()).values()];
		if (bases.length === 0) {
			// Dual read: a pre-migration vault syncs exactly as before.
			await cover(changeFilters, resumeUntil, trackFloor);
		} else {
			// Live deltas of every held current base, the vanish ledger's whole
			// stream (it never gets a base, and without it nothing vanished is
			// enforced here), then the deltas of objects with no base (and any
			// untagged write) in a window bounded by the newest base: never
			// kind-1078 history from event zero.
			const hashes = [...new Set(bases.map((base) => base.hash))].sort();
			const deltaFilters: StreamFilter[] = [];
			for (let i = 0; i < hashes.length; i += BASE_HASH_BATCH) {
				for (const { scope, filter } of changeFilters) deltaFilters.push({ scope, filter: { ...filter, "#b": hashes.slice(i, i + BASE_HASH_BATCH) } });
			}
			const ledgerTag = coreCall<string>("wire", { action: "blind", secret: this.secretHex, id: VANISH_LOG_ID });
			deltaFilters.push({ scope: "", filter: { kinds: [CHANGE_KIND], authors: [this.pk], "#h": [ledgerTag] } });
			await cover(deltaFilters, undefined, false);
			const newest = bases.reduce((max, base) => Math.max(max, base.createdAt), 0);
			const bound = Math.max(1, Math.floor(newest / 1000) - BASE_LOOKBACK_S);
			await cover(changeFilters.map(({ scope, filter }) => ({ scope, filter: { ...filter, since: bound } })), resumeUntil, trackFloor);
		}
		// This device's unpublished writes whose object moved to a newer base
		// meanwhile (another session, a crash before rebasing) rebase now.
		const owed = (await this.store.pendingPublishes()).map((pending) => pending.objectId);
		const rebasing = this.importChain.then(() => this.rebaseOrphans(owed));
		this.importChain = rebasing.catch(() => {});
		await rebasing;
		const covered = walked ? since : 0;
		// Live subscriptions can deliver the same recent events concurrently
		// and a page import may still be committing.
		// An event mid-flight is not a fault: let it land, then judge the
		// session it produced. Faulting here instead cost every cold start a
		// full second walk 60s later.
		await this.drainInflight();
		// Every relay answered EOSE for every filter and nothing faulted: a
		// CHECKPOINT group still open is missing parts no relay holds (a chunk
		// dropped, or a publish that never finished). A checkpoint is only a
		// cache, so the core retires those - a change group stays until a
		// covering repair, since a missing change is missing data.
		if (complete && !this.stopped && this.replayFaultGeneration === checkpoint && this.sessionOpen) {
			const r = coreCall<{ replayGroups?: Array<[string, number]> }>("sync", { action: "retire", since: covered <= 1 ? 0 : covered });
			if (r.replayGroups) this.replayGroups = new Map(r.replayGroups);
		}
		this.historyComplete = complete && !this.stopped && this.replayGroups.size === 0 &&
			this.replayFaultGeneration === checkpoint && covered <= this.discardedChunkFloor;
		if (this.historyComplete) {
			this.discardedChunkFloor = Infinity;
			if (trackFloor) await this.store.setBootstrapFloor(undefined);
		} else this.recordReplayFault(since);
		const repaired = this.historyComplete;
		await this.persistCursor();
		// IndexedDB persistence yields to live handlers. Restore the obligation
		// if a new group or fault appeared while committing the repaired cursor.
		await this.drainInflight();
		if (repaired && (!this.historyComplete || this.stopped || this.replayGroups.size > 0 || this.replayFaultGeneration !== checkpoint)) {
			this.recordReplayFault(since);
			await this.persistCursor();
		}
		return this.historyComplete;
	}

	/** Settle every live event and import in flight; new arrivals extend the wait, they never fault. */
	private async drainInflight(): Promise<void> {
		while (!this.stopped && (this.activeLiveEvents > 0 || this.activeImports > 0)) {
			await this.liveChain;
			await this.importChain;
		}
	}

	/** querySync treats timeout/closed as empty success; only real EOSE proves exhaustion. */
	private async queryRelayPage(url: string, filter: Parameters<SimplePool["querySync"]>[1]): Promise<Event[]> {
		const relay = await this.pool.ensureRelay(url, { connectionTimeout: 15_000 });
		const done = Promise.withResolvers<Event[]>();
		const events: Event[] = [];
		const sub = relay.prepareSubscription([filter], {
			eoseTimeout: 60_000,
			onevent: (event) => events.push(event),
			oneose: () => done.resolve(events),
			onclose: (reason) => done.reject(new Error(reason)),
		});
		const timer = setTimeout(() => done.reject(new Error("history EOSE timeout")), 15_000);
		try {
			sub.fire();
			return await done.promise;
		} finally {
			clearTimeout(timer);
			sub.oneose = undefined;
			sub.receivedEose(); // Cancel nostr-tools' synthetic EOSE timer.
			sub.close();
		}
	}

	/**
	 * NIP-77 cover of one stream filter on one relay: reconcile the held set,
	 * then fetch every needed id in batches and import it through the walk's
	 * page path. "unsupported" = the relay never answered NEG-OPEN with a
	 * NEG-MSG; the caller walks this filter instead. `claimed` is shared by
	 * every stream of one sync pass: an event in two streams (our own event in
	 * a space) is fetched once, and the run is only complete if that fetch is.
	 */
	private async reconcileStream(relay: string, scope: string, filter: Filter, claimed: Set<string>, importPage: (page: Event[]) => Promise<void>): Promise<"complete" | "incomplete" | "unsupported"> {
		// A store failure propagates: the recovery obligation must outlive the scan.
		const held = await this.store.relayEvents(scope, filter.kinds ?? [], { since: filter.since, b: filter["#b"] });
		let need: string[];
		try {
			// One negotiation at a time per relay (relays cap concurrent NEG sessions); fetching overlaps freely.
			const run = (this.negotiations.get(relay) ?? Promise.resolve()).then(() => {
				if (this.negentropyUnsupported.has(relay)) throw new NegentropyFallback("relay does not speak NIP-77", true);
				return this.reconcileRelay(relay, filter, held);
			}).catch((err: unknown) => {
				if (err instanceof NegentropyFallback && err.unsupported) this.negentropyUnsupported.add(relay);
				throw err;
			});
			this.negotiations.set(relay, run.catch(() => {}));
			const needed = await run;
			need = needed.filter((id) => !claimed.has(id));
			for (const id of need) claimed.add(id);
		} catch (err) {
			if (err instanceof NegentropyFallback) return "unsupported";
			this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 100)}` });
			return "incomplete";
		}
		let missing = 0;
		// Checkpoint events run to ~40k chars: 100 per REQ is a multi-MB burst a
		// slow phone link cannot drain before the relay's send deadline.
		const batchSize = filter.kinds?.includes(CHECKPOINT_KIND) || filter.kinds?.includes(BASE_KIND) ? CHECKPOINT_FETCH_BATCH : FETCH_BATCH;
		for (let i = 0; i < need.length; i += batchSize) {
			if (this.stopped) return "incomplete";
			const ids = need.slice(i, i + batchSize);
			const started = Date.now();
			let page: Event[];
			try {
				page = await this.queryRelayPage(relay, { ids, limit: ids.length });
			} catch (err) {
				this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 100)}` });
				return "incomplete";
			}
			const wanted = new Set(ids);
			page = page.filter((event) => wanted.delete(event.id));
			// A needed id the relay doesn't return (deleted meanwhile, or a response cut short) is incomplete: asked again next run.
			missing += wanted.size;
			await importPage(page);
			this.emitLiveStatus();
			if (i + batchSize < need.length) await sleep(Math.min(PAGE_SPACING_MS, Math.max(MIN_PAGE_SPACING_MS, Date.now() - started)));
		}
		if (missing > 0) {
			this.events.onStatus({ phase: "backfill", detail: `${relay}: ${missing} reconciled events not returned` });
			return "incomplete";
		}
		return "complete";
	}

	/** NIP-77 initiator over one filter: the ids the relay holds that `held` lacks. Ids we hold and it lacks are the outbox's business. */
	private async reconcileRelay(url: string, filter: Filter, held: Array<{ id: string; createdAt: number }>): Promise<string[]> {
		let answered = false;
		try {
			const storage = new NegentropyStorage();
			for (const row of held) storage.insert(row.createdAt, row.id);
			storage.seal();
			const neg = new Negentropy(storage, NEG_FRAME_LIMIT);
			const relay = await this.pool.ensureRelay(url, { connectionTimeout: 15_000 });
			const done = Promise.withResolvers<string[]>();
			done.promise.catch(() => {}); // settled by sub.close() too, after an early throw nobody awaits it
			const need: string[] = [];
			let errored = false;
			const sub = relay.prepareSubscription([filter], { label: "neg", onclose: (reason) => done.reject(new Error(`closed: ${reason}`)) });
			let timer: ReturnType<typeof setTimeout> | undefined;
			const arm = () => {
				clearTimeout(timer);
				timer = setTimeout(() => done.reject(answered ? new Error("NEG-MSG timeout") : new NegentropyFallback("no answer to NEG-OPEN", true)), NEG_TIMEOUT_MS);
			};
			sub.oncustom = (msg) => {
				if (msg[0] === "NEG-MSG") {
					answered = true;
					try {
						const r = neg.reconcile(msg[2]);
						need.push(...r.needIds);
						if (r.next === null) done.resolve(need);
						else {
							arm();
							relay.send(JSON.stringify(["NEG-MSG", sub.id, r.next])).catch((err) => done.reject(err));
						}
					} catch (err) {
						done.reject(err);
					}
				} else if (msg[0] === "NEG-ERR") {
					errored = true;
					done.reject(answered ? new Error(`NEG-ERR ${msg[2]}`) : new NegentropyFallback(`NEG-ERR ${msg[2]}`, true));
				}
			};
			let listeners = noticeListeners.get(relay);
			if (!listeners) {
				const all = new Set<(notice: string) => void>();
				const previous = relay.onnotice;
				relay.onnotice = (notice) => {
					previous(notice);
					for (const listener of all) listener(notice);
				};
				noticeListeners.set(relay, (listeners = all));
			}
			// A relay without NIP-77 typically answers the unknown verb with a NOTICE.
			const onNotice = (notice: string) => {
				if (!answered) done.reject(new NegentropyFallback(`NOTICE ${notice}`, true));
			};
			listeners.add(onNotice);
			try {
				arm();
				await relay.send(JSON.stringify(["NEG-OPEN", sub.id, filter, neg.initiate()]));
				return await done.promise;
			} finally {
				clearTimeout(timer);
				listeners.delete(onNotice);
				if (!errored) relay.send(JSON.stringify(["NEG-CLOSE", sub.id])).catch(() => {});
				sub.closed = true; // NEG-CLOSE ends it relay-side; no REQ CLOSE for a sub that never fired
				sub.close();
			}
		} catch (err) {
			// Anything before the first NEG-MSG (no socket, no NIP-77): walk this filter.
			if (!answered && !(err instanceof NegentropyFallback)) throw new NegentropyFallback(String(err), false);
			throw err;
		}
	}

	/** This event's rows in the held-events table: the self stream when we signed it, and every installed space stream it is tagged into. */
	private heldRows(event: Event): RelayEventRow[] {
		const rows: RelayEventRow[] = [];
		const b = event.kind === CHANGE_KIND ? event.tags.find((t) => t[0] === "b")?.[1] : undefined;
		const row = (scope: string): RelayEventRow => (b ? { id: event.id, createdAt: event.created_at, kind: event.kind, scope, b } : { id: event.id, createdAt: event.created_at, kind: event.kind, scope });
		if (event.pubkey === this.pk) rows.push(row(""));
		for (const sp of this.sharedSpaces.values()) {
			if (event.tags.some((t) => t[0] === "h" && t[1] === sp.spaceTag)) rows.push(row(sp.spaceTag));
		}
		return rows;
	}

	// ── Event → change / checkpoint / base ─────────────────────────

	/** Feed one signature-verified relay event to the core session; returns the
	 * decoded payload when a full change, checkpoint or base (possibly
	 * reassembled from chunks) is available. The core owns reassembly, the
	 * cursor and replay bookkeeping. */
	private async ingestEvent(event: Event): Promise<ImportItem | null> {
		this.stats.events++;
		this.onRawEvent?.(event);
		if (!SPACE_KINDS.includes(event.kind) || !verifyEvent(event)) return null;
		await this.ensureSession();
		if (!this.sessionOpen) return null; // stopped
		const r = coreCall<IngestResult>("sync", {
			action: "ingest",
			event: { pubkey: event.pubkey, created_at: event.created_at, kind: event.kind, tags: event.tags, content: event.content },
			nowMs: Date.now(),
		});
		this.cursor = r.cursor;
		if (r.hTag) this.stats.blindedTags.add(r.hTag);
		if (r.decryptFailure) this.stats.decryptFailures++;
		if (r.decodeFailure) this.stats.decodeFailures++;
		if (r.faultAt !== undefined) this.recordReplayFault(r.faultAt);
		if (r.replayGroups) this.replayGroups = new Map(r.replayGroups);
		// The core only reports an h-deletion signed by the space's owner.
		if (r.spaceVanished) this.onSpaceVanished?.(r.spaceVanished);
		// What this device holds of the relay's streams (the NIP-77 item set).
		// Recorded only once its payload imported: a part waits for its group,
		// a faulted event is never held, so the next reconcile asks again.
		const rows = this.heldRows(event);
		const chunk = event.tags.find((t) => t[0] === "c");
		// A base's 31078 event and its 1080 parts are one group (keyed as the base kind, as the core does).
		const groupKind = event.kind === BASE_PART_KIND ? BASE_KIND : event.kind;
		const partKey = chunk && `${event.pubkey}|${event.tags.filter((t) => t[0] === "h").map((t) => t[1]).join(",")}|${chunk[1]}|${groupKind}`;
		if (event.kind === BASE_KIND && chunk && !r.item && r.faultAt === undefined && !r.decryptFailure && !this.importedParts.has(partKey!)) this.openBaseGroups.add(chunk[1]);
		if (!r.item) {
			if (r.faultAt !== undefined) {
				if (partKey) this.pendingParts.delete(partKey);
			} else if (partKey && !r.decryptFailure && !this.importedParts.has(partKey)) {
				this.pendingParts.set(partKey, [...(this.pendingParts.get(partKey) ?? []), ...rows]);
			} else await this.store.recordRelayEvents(rows);
			return null;
		}
		const held = partKey ? [...(this.pendingParts.get(partKey) ?? []), ...rows] : rows;
		if (partKey) this.pendingParts.delete(partKey);
		const item: ImportItem = {
			objectId: "",
			bytes: base64ToBytes(r.item.bytes),
			b64: r.item.bytes,
			chunkKey: r.item.chunkKey,
			provenance: r.item.provenance,
			held,
			partKey,
		};
		if (r.item.base) {
			item.base = r.item.base;
			item.objectId = r.item.base.objectId;
		} else if (r.item.checkpoint) {
			item.checkpoint = r.item.checkpoint;
			item.objectId = r.item.checkpoint.objectId;
		} else {
			item.change = unpackCoreValueMaps<ChangeJSON>(r.item.change);
			if (r.item.b) item.change.b = r.item.b;
			item.objectId = item.change.objectId;
		}
		return item;
	}

	/** Everything the store holds for one object, replayed the way the backend does it. */
	private async existingState(objectId: string): Promise<ObjectJSON | null> {
		return replayObject(await this.store.objectInputs(objectId));
	}

	/**
	 * Shared authority for either payload kind. Existing scope and privileges
	 * come from what this replica already holds, never from the candidate.
	 */
	private async authorized(item: ImportItem, p: SharedProvenance): Promise<boolean> {
		const space = this.sharedSpaces.get(p.spaceId);
		if (!space) return false;
		const [trustedSpace, existing] = await Promise.all([this.existingState(p.spaceId), this.existingState(item.objectId)]);
		if (item.base) return authorizeSharedBase(item.b64, p, space, this.pk, trustedSpace, existing);
		return item.checkpoint
			? authorizeSharedCheckpoint(item.b64, p, space, this.pk, trustedSpace, existing)
			: authorizeSharedChange(item.change!, p, space, this.pk, trustedSpace, existing);
	}

	/**
	 * Keep a checkpoint when it beats the one held (covers a superset, then
	 * hash - the store asks the core). A superseded or incomparable copy is
	 * still a clean import: re-scanning would only produce it again. The
	 * signer was already checked: the core session only opens personal
	 * payloads from this key, and `authorized` holds shared ones to the owner.
	 */
	private async importCheckpoint(item: ImportItem, summary: CheckpointSummary): Promise<void> {
		const row: CheckpointRow = {
			objectId: summary.objectId,
			bytes: item.bytes,
			hash: summary.hash,
			heads: [...summary.headIds].sort(),
		};
		if (!(await this.store.putCheckpoint(row))) return;
		this.stats.checkpoints++;
		this.pendingObjects.add(summary.objectId);
	}

	/**
	 * Hold a base and re-point the object's current one (core `base_current`).
	 * Bases are never deleted: a superseded one keeps more orphans decidable.
	 * The signer was checked as for checkpoints (`authorized` for spaces).
	 */
	private async importBase(item: ImportItem, summary: BaseSummary): Promise<void> {
		const row: BaseRow = { objectId: summary.objectId, hash: summary.hash, epoch: summary.epoch, createdAt: summary.createdAt, prevBase: summary.prevBase, bytes: item.bytes };
		const fresh = await this.store.putBase(row, pickCurrentBase);
		this.basedObjects.add(summary.objectId);
		if (!fresh) return;
		this.stats.bases++;
		this.pendingObjects.add(summary.objectId);
	}

	/** A shared object is rebased only by an identity that may write its space; anything else is this vault's own. */
	private mayRebase(objectId: string): boolean {
		const space = this.sharedSpaces.get(this.spaceOf(objectId));
		return !space || space.writerSet.has(this.pk) || (space.owner || this.pk) === this.pk;
	}

	/**
	 * Rebase every orphan of these objects onto its current base (core
	 * `rebase`; `pending` = this device's unpublished ids make its own writes
	 * decidable), store each rebased change with its orphan marked, and
	 * publish it carrying `b`. Deterministic: another device rebasing the same
	 * orphan produces the same id, so the second publish dedupes. A core
	 * refusal is reported, never a replay fault: the held data stands.
	 */
	private async rebaseOrphans(objectIds: Iterable<string>): Promise<void> {
		let pending: Set<string> | undefined;
		for (const objectId of new Set(objectIds)) {
			if (this.stopped || !this.basedObjects.has(objectId) || !this.mayRebase(objectId)) continue;
			pending ??= await this.store.pendingChangeIds();
			const input = await this.store.objectInputs(objectId);
			if (input.bases.length === 0) continue;
			let rebased: Rebased;
			try {
				rebased = rebaseObject({ ...input, pending });
			} catch (err) {
				this.events.onStatus({ phase: "error", detail: `rebase of ${objectId} refused: ${String(err).slice(0, 120)}` });
				continue;
			}
			for (const { orphan, change, bytes } of rebased.changes) {
				const raw = base64ToBytes(bytes);
				// Stored as relay imports are: the decoded protobuf, plus the base it was written on.
				const stored: ChangeJSON = { ...(decodeChange(raw) ?? change), b: rebased.base };
				await this.store.addRebased(orphan, raw, stored);
				pending.add(stored.id);
				this.stats.rebased++;
				this.pendingObjects.add(objectId);
				await this.publish(raw, stored.id, objectId, rebased.base);
			}
		}
	}

	/** Report a reassembled group's import outcome to the core session. */
	private settle(chunkKey: string, imported: boolean): void {
		if (!this.sessionOpen) return;
		const r = coreCall<{ replayGroups?: Array<[string, number]> }>("sync", { action: "settle", chunkKey, imported });
		if (r.replayGroups) this.replayGroups = new Map(r.replayGroups);
	}

	private importBatch(batch: ImportItem[], immediateNotify = false): Promise<void> {
		this.activeImports++;
		const imported = new Set<string>();
		const run = this.importChain.then(async () => {
			const held: RelayEventRow[] = [];
			const parts: string[] = [];
			const touched: string[] = [];
			for (const item of batch) {
				if (item.provenance && !(await this.authorized(item, item.provenance))) continue;
				if (item.base) {
					await this.importBase(item, item.base);
				} else if (item.checkpoint) {
					await this.importCheckpoint(item, item.checkpoint);
				} else {
					const change = item.change!;
					this.stats.imported += await this.store.addChanges([{ bytes: item.bytes, change }]);
					const p = item.provenance;
					await this.store.markPublished(p ? `${p.spaceId}/${p.keyId}/${change.id}` : change.id);
					this.pendingObjects.add(change.objectId);
				}
				if (item.chunkKey) {
					imported.add(item.chunkKey);
					this.settle(item.chunkKey, true);
				}
				held.push(...(item.held ?? []));
				if (item.partKey) parts.push(item.partKey);
				touched.push(item.objectId);
			}
			// Unauthorized payloads stay unheld: a later reconcile re-judges them against newer state.
			await this.store.recordRelayEvents(held);
			for (const key of parts) this.importedParts.add(key);
			// A new base can orphan held deltas, and a delta can arrive orphaned.
			await this.rebaseOrphans(touched);
			if (immediateNotify) this.flushObjectNotify();
			else this.scheduleObjectNotify();
		}).catch(async (err) => {
			this.recordReplayFault(1);
			await this.persistCursor();
			throw err;
		}).finally(() => {
			for (const item of batch) if (item.chunkKey && !imported.has(item.chunkKey)) this.settle(item.chunkKey, false);
			this.activeImports--;
		});
		this.importChain = run.catch(() => {});
		return run;
	}

	private recordReplayFault(at: number): void {
		this.replayFaultGeneration++;
		this.discardedChunkFloor = Math.min(this.discardedChunkFloor, at);
		this.historyComplete = false;
		// A closed store (stop() under a running walk) must not surface as an unhandled rejection.
		void this.store.setBootstrapFloor(undefined).catch(() => {});
	}

	private persistCursor(): Promise<void> {
		const run = this.cursorChain.then(async () => {
			await this.ensureSession(); // the mirror must reflect persisted obligations before overwriting them
			const saved = await this.store.getCursor();
			const floor = Math.min(this.discardedChunkFloor, ...this.replayGroups.values());
			const next = Math.min(this.cursor, floor === Infinity ? Infinity : Math.max(0, floor - 1));
			const advance = next < saved || (this.historyComplete && this.activeLiveEvents === 0 && this.activeImports === 0 && next > saved);
			await this.store.setCursor(advance ? next : saved, [...this.replayGroups]);
		});
		this.cursorChain = run.catch(() => {});
		return run;
	}

	private async handleLiveEvent(event: Event): Promise<void> {
		this.activeLiveEvents++;
		try {
			const item = await this.ingestEvent(event);
			if (item) await this.importBatch([item]);
			else if (event.kind === BASE_KIND) {
				// A live split base: its parts went out first, possibly before the cursor.
				const gid = event.tags.find((t) => t[0] === "c")?.[1];
				if (gid && this.openBaseGroups.delete(gid)) void this.fetchLiveBaseParts(gid);
			}
		} catch (err) {
			this.recordReplayFault(1);
			throw err;
		} finally {
			this.activeLiveEvents--;
			await this.persistCursor();
		}
	}

	private async fetchLiveBaseParts(gid: string): Promise<void> {
		await Promise.all(this.relays.map(async (relay) => {
			try {
				for (const part of await this.queryRelayPage(relay, { kinds: [BASE_PART_KIND], "#c": [gid] })) {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(part)).catch(() => {});
				}
			} catch {
				/* the next history pass fetches them */
			}
		}));
	}

	// ── Object-change notification batching ────────────────────────

	private flushObjectNotify(): void {
		if (this.notifyTimer) {
			clearTimeout(this.notifyTimer);
			this.notifyTimer = null;
		}
		if (this.pendingObjects.size === 0) return;
		const ids = [...this.pendingObjects];
		this.pendingObjects.clear();
		this.events.onObjects(ids);
	}

	private scheduleObjectNotify(): void {
		if (this.notifyTimer || this.pendingObjects.size === 0) return;
		this.notifyTimer = setTimeout(() => {
			this.notifyTimer = null;
			this.flushObjectNotify();
		}, NOTIFY_DEBOUNCE_MS);
	}

	// ── Publish ────────────────────────────────────────────────────

	/**
	 * A local write must never depend on the network. The durable
	 * obligation is stored FIRST; only then do we try to open a session and
	 * hand it to the outbox. A phone that suspended its relay session (iOS
	 * Safari does this aggressively) therefore still commits, and `start()`
	 * re-offers every stored obligation on the next load. `base`: the hash of
	 * the base the change was written on; its sealed parts carry ["b", base].
	 */
	async publish(bytes: Uint8Array, changeId: string, objectId: string, base?: string): Promise<void> {
		const candidates: PendingPublish[] = [{ key: changeId, changeId, objectId, bytes, ...(base ? { base } : {}) }];
		const space = this.sharedSpaces.get(this.spaceOf(objectId));
		if (space) candidates.push({ key: `${space.spaceId}/${space.keyId}/${changeId}`, changeId, objectId, bytes, spaceId: space.spaceId, keyId: space.keyId, ...(base ? { base } : {}) });
		const owed: PendingPublish[] = [];
		for (const item of candidates) {
			if (await this.store.isPublished(item.key)) continue;
			const saved = await this.store.getPending(item.key);
			if (!saved) await this.store.savePending(item);
			owed.push(saved ?? item);
		}
		if (owed.length === 0) return;
		try {
			await this.ensureSession();
			for (const item of owed) this.offerToOutbox(item);
		} catch (err) {
			// Stored, unsent: the next start() re-offers it. Surfacing this as
			// a write failure would discard a message the DAG already holds.
			this.events.onStatus({ phase: "error", detail: `publish deferred: ${(err instanceof Error ? err.message : String(err)).slice(0, 120)}` });
		}
	}

	/** Hand a durable obligation to the core outbox, which owns order, dedupe and
	 * the rotated-key rule; the store record stays either way. */
	private offerToOutbox(pending: PendingPublish): void {
		if (!this.sessionOpen) return; // stopped or retired: the next start() re-offers from the store
		const { queued, pending: count } = coreCall<{ queued: boolean; reason?: string; pending: number }>("sync", {
			action: "outbox_enqueue",
			pending: {
				key: pending.key,
				objectId: pending.objectId,
				changeId: pending.changeId,
				bytes: bytesToBase64(pending.bytes),
				spaceId: pending.spaceId,
				keyId: pending.keyId,
				base: pending.base,
				hasEvents: !!pending.events,
			},
		});
		this.pendingCount = count;
		if (!queued) return;
		this.emitLiveStatus();
		if (!this.queueRunning) {
			this.queueRunning = true;
			void this.runPublishQueue();
		}
	}

	/** Drains the core outbox: one event per PUBLISH_SPACING_MS, outcomes
	 * reported back so the core applies backoff and never drops an item while
	 * the session lives. */
	private async runPublishQueue(): Promise<void> {
		while (!this.stopped && this.sessionOpen) {
			let next: OutboxNext;
			try {
				next = coreCall<OutboxNext>("sync", { action: "outbox_next", nowMs: Date.now() });
			} catch (err) {
				// An unsealable change (e.g. beyond the chunk limit): the core backed it off; keep draining the rest.
				this.events.onStatus({ phase: "error", detail: `publish rejected: ${String(err).slice(0, 120)}` });
				await sleep(PUBLISH_SPACING_MS);
				continue;
			}
			this.pendingCount = next.pending;
			if (!next.item) {
				if (next.pending === 0) break;
				await sleep(Math.min(500, Math.max(50, next.waitMs)));
				continue;
			}
			const outcome = await this.publishOnce(next.item);
			if (this.sessionOpen) {
				const { pending } = coreCall<{ pending: number }>("sync", {
					action: "outbox_result",
					key: next.item.key,
					ok: outcome.ok,
					sealed: outcome.sealed,
					nowMs: Date.now(),
				});
				this.pendingCount = pending;
			}
			await sleep(PUBLISH_SPACING_MS);
		}
		this.queueRunning = false;
		this.emitLiveStatus();
	}

	/** Sign the core's sealed parts on the first attempt, persist the exact
	 * signed events before sending, then send. `sealed` tells the core the
	 * ciphertext is on disk so retries reuse it byte for byte. */
	private async publishOnce(item: OutboxItem): Promise<{ ok: boolean; sealed: boolean }> {
		let sealed = false;
		try {
			if (await this.store.isPublished(item.key)) return { ok: true, sealed };
			const pending = await this.store.getPending(item.key);
			if (!pending) {
				this.events.onStatus({ phase: "error", detail: `publish skipped: no stored obligation for ${item.key}` });
				return { ok: true, sealed };
			}
			if (!pending.events) {
				if (!item.sealed) throw new Error("no signed events to resend");
				const created_at = Math.floor(Date.now() / 1000);
				pending.events = item.sealed.parts.map((part) =>
					finalizeEvent({ kind: CHANGE_KIND, created_at, tags: part.tags, content: part.content }, this.sk));
			}
			// Persist BEFORE sending, including after any failed IndexedDB attempt.
			await this.store.savePending(pending);
			sealed = true;
			for (const event of pending.events) {
				await Promise.any(this.pool.publish(this.relays, event));
				if (pending.events.length > 1) await sleep(PUBLISH_SPACING_MS);
			}
			// Their ids are known at signing: the next reconcile need not fetch our own writes back.
			await this.store.recordRelayEvents(pending.events.flatMap((event) => this.heldRows(event)));
			await this.store.markPublished(item.key);
			return { ok: true, sealed };
		} catch (err) {
			const detail =
				err instanceof AggregateError
					? err.errors.map((e) => String(e).slice(0, 80)).join(" | ")
					: String(err).slice(0, 120);
			this.events.onStatus({ phase: "error", detail: `publish rejected (attempt ${item.attempts + 1}): ${detail}` });
			return { ok: false, sealed };
		}
	}
}
