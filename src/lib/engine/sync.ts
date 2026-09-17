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
 * Backfill pages querySync backwards via `until` (since cursor+1), paced
 * between pages so public relays don't rate-limit us; live is a
 * subscribeMany since cursor+1. Sends are paced one event per
 * PUBLISH_SPACING_MS.
 */

import { SimplePool, finalizeEvent, getPublicKey, nip19, nip44, verifyEvent, type Event } from "nostr-tools";
import { unwrapEvent, wrapEvent } from "nostr-tools/nip59";
import { bytesToHex } from "@noble/hashes/utils.js";
import type { ChangeJSON, ChangeStoreApi, PendingPublish, RelaySyncApi, SharedProvenance, SyncEvents } from "./contracts";
import type { ObjectJSON } from "$lib/types";
import { computeObject } from "./replay";
import { CoreError, coreCall } from "./core";
import { unpackCoreValueMaps } from "./core-values";
import { loadKey } from "./keys";
import { spaceKeyGet, spaceKeyImport } from "./spacekeys";

export const DEFAULT_RELAYS = ["wss://roostr-relay.fly.dev"];

const CHANGE_KIND = 1078;
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
	try {
		return coreCall<{ ok: boolean; reason: string }>("sync", {
			action: "authorize",
			change,
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

interface ImportItem {
	bytes: Uint8Array;
	change: ChangeJSON;
	provenance?: SharedProvenance;
	chunkKey?: string;
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
	item?: { bytes: string; change: unknown; chunkKey?: string; provenance?: SharedProvenance };
	faultAt?: number;
	replayGroups?: Array<[string, number]>;
	decryptFailure?: boolean;
	decodeFailure?: boolean;
	hTag?: string;
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
// Keep full-sized 40k-character encrypted chunks inside the 8 MiB relay budget.
const PAGE_LIMIT = 128;
const NOTIFY_DEBOUNCE_MS = 100;

function sleep(ms: number): Promise<void> {
	const { promise, resolve } = Promise.withResolvers<void>();
	setTimeout(resolve, ms);
	return promise;
}

function b64ToBytes(b64: string): Uint8Array {
	const bin = atob(b64);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}

function bytesToB64(bytes: Uint8Array): string {
	let bin = "";
	for (let i = 0; i < bytes.length; i += 0x8000) {
		bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
	}
	return btoa(bin);
}

export interface RelaySyncOptions {
	/** A gift-wrapped space key arrived and was imported. */
	onSpaceKey?: () => void;
	/** Debug hook: every raw relay event before decrypt. */
	onRawEvent?: (event: Event) => void;
	/** objectId -> owning space id ("" = personal). Channels own themselves. */
	spaceOf?: (objectId: string) => string;
}

export interface SyncStats {
	events: number;
	decryptFailures: number;
	decodeFailures: number;
	imported: number;
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
	readonly stats: SyncStats = { events: 0, decryptFailures: 0, decodeFailures: 0, imported: 0, blindedTags: new Set() };

	private readonly pk: string;
	private pool = new SimplePool();
	/** NIP-44 self conversation key, hex, as the core's `wire` method takes it. */
	private readonly conversationKey: string;
	private readonly secretHex: string;
	private readonly onRawEvent?: (event: Event) => void;
	private readonly onSpaceKey?: () => void;
	private wrapSub: { close(): void } | null = null;
	private readonly spaceOf: (objectId: string) => string;
	private sharedSpaces = new Map<string, SharedSpace>();
	private spaceSub: { close(): void } | null = null;

	private cursor = 0;
	private stopped = false;
	private sub: { close(): void } | null = null;
	private watchdogTimer: ReturnType<typeof setInterval> | null = null;
	private watchdogBusy = false;

	/** Host mirror of the core session's replay obligations: chunk key → earliest created_at. */
	private replayGroups = new Map<string, number>();
	private sessionOpening: Promise<void> | null = null;
	private importChain: Promise<void> = Promise.resolve();
	private historyComplete = false;
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
	}

	private get sessionOpen(): boolean {
		return sessionOwner === this;
	}

	/** (Re)open the core receive session from this instance's cursor and the persisted obligations. */
	private async openSession(): Promise<void> {
		const replayGroups = await this.store.getReplayGroups();
		const state = coreCall<SyncSessionState>("sync", {
			action: "session",
			pk: this.pk,
			conversationKey: this.conversationKey,
			secret: this.secretHex,
			spaces: [...this.sharedSpaces.values()].map((sp) => ({ spaceId: sp.spaceId, keyHex: sp.keyHex, keyId: sp.keyId })),
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
			coreCall("sync", { action: "spaces", spaces: [...next.values()].map((sp) => ({ spaceId: sp.spaceId, keyHex: sp.keyHex, keyId: sp.keyId })) });
		}
		const tags = [...next.values()].map((sp) => sp.spaceTag);
		const fresh = tags.filter((t) => !prevTags.has(t));
		if (this.stopped || !this.sub) return; // start() wires subscriptions itself
		try {
			this.spaceSub?.close();
		} catch {
			/* already closed */
		}
		this.spaceSub = null;
		if (tags.length > 0) {
			this.spaceSub = this.pool.subscribeMany(this.relays, { kinds: [CHANGE_KIND], "#h": tags, since: this.cursor + 1 }, {
				onevent: (event) => {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
				},
			});
		}
		if (fresh.length > 0) void this.backfill(1).catch((err) => this.events.onStatus({ phase: "error", detail: String(err) }));
	}


	/**
	 * The dot answers "am I connected and is my work published" - historical
	 * verification runs in the background and belongs in the detail text, not
	 * in the phase. Green with an incomplete history is honest: live changes
	 * flow; the tooltip says history is still being verified.
	 */
	private statusDetail(): string | undefined {
		if (this.historyComplete) return undefined;
		return `verifying full history in background · ${this.stats.imported} changes so far`;
	}

	private emitLiveStatus(): void {
		this.events.onStatus({
			phase: this.liveUp ? "live" : "backfill",
			imported: this.stats.imported,
			detail: this.liveUp ? this.statusDetail() : undefined,
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
		this.emitLiveStatus();
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
			{ kinds: [CHANGE_KIND], authors: [this.pk], since: this.cursor + 1 },
			{
				onevent: (event) => {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
				},
			},
		);
		const spaceTags = [...this.sharedSpaces.values()].map((sp) => sp.spaceTag);
		this.spaceSub = spaceTags.length > 0
			? this.pool.subscribeMany(this.relays, { kinds: [CHANGE_KIND], "#h": spaceTags, since: this.cursor + 1 }, {
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
		if (this.stopped || this.watchdogBusy || this.bootstrapping) return;
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
				this.pool.querySync(this.relays, { kinds: [CHANGE_KIND], authors: [this.pk], limit: 1 }),
				spaceTags.length > 0
					? this.pool.querySync(this.relays, { kinds: [CHANGE_KIND], "#h": spaceTags, limit: 1 })
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
				await this.catchupSince(this.cursor + 1);
				this.subscribeLive();
				this.emitLiveStatus();
				return;
			}
			const head = [...res[0], ...res[1]].reduce((max, e) => Math.max(max, e.created_at), 0);
			if (head > this.cursor) {
				this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: "catching up" });
				await this.catchupSince(this.cursor + 1);
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

	private async catchupSince(since: number): Promise<void> {
		await this.backfill(since);
		// Gift wraps have randomized created_at: re-query on every catchup;
		// the seen-set dedupes. Recovers knocks lost to dropped sockets.
		try {
			const wraps = await this.pool.querySync(this.relays, { kinds: [WRAP_KIND], "#p": [this.pk] });
			wraps.sort((a, b) => a.created_at - b.created_at);
			for (const w of wraps) await this.handleWrap(w);
		} catch {
			/* next watchdog tick */
		}
	}

	stop(): void {
		this.stopped = true;
		this.liveUp = false;
		this.closeSession();
		if (this.watchdogTimer) {
			clearInterval(this.watchdogTimer);
			this.watchdogTimer = null;
		}
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

	/** Returns true only when EVERY relay was walked to exhaustion. */
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
		const byId = new Map<string, Event>();
		let complete = this.relays.length > 0;
		const filters: Array<Parameters<SimplePool["querySync"]>[1]> = [{ kinds: [CHANGE_KIND], authors: [this.pk], since }];
		for (const sp of this.sharedSpaces.values()) filters.push({ kinds: [CHANGE_KIND], "#h": [sp.spaceTag], since });
		await Promise.all(this.relays.flatMap((relay) => filters.map(async (filter) => {
			let until: number | undefined = resumeUntil;
			try {
				for (;;) {
					if (this.stopped) { complete = false; return; }
					const page = await this.queryRelayPage(relay, { ...filter, until, limit: PAGE_LIMIT });
					for (const event of page) byId.set(event.id, event);
					if (page.length < PAGE_LIMIT) return;
					const oldest = Math.min(...page.map((e) => e.created_at));
					if (until !== undefined && oldest >= until) throw new Error("saturated same-timestamp history page");
					until = oldest;
					// Coverage is only as deep as the slowest concurrent walker.
					if (trackFloor) {
						coveredUntil = coveredUntil === undefined ? until : Math.min(coveredUntil, until);
						await this.store.setBootstrapFloor(coveredUntil);
					}
					await sleep(PAGE_SPACING_MS);
				}
			} catch (err) {
				complete = false;
				this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 100)}` });
			}
		})));
		const batch: ImportItem[] = [];
		for (const event of [...byId.values()].sort((a, b) => a.created_at - b.created_at || a.id.localeCompare(b.id))) {
			const item = await this.eventToChange(event);
			if (item) batch.push(item);
		}
		await this.importBatch(batch, true);
		this.historyComplete = complete && !this.stopped && this.replayGroups.size === 0 &&
			this.activeLiveEvents === 0 && this.activeImports === 0 &&
			this.replayFaultGeneration === checkpoint && since <= this.discardedChunkFloor;
		if (this.historyComplete) {
			this.discardedChunkFloor = Infinity;
			if (trackFloor) await this.store.setBootstrapFloor(undefined);
		} else this.recordReplayFault(since);
		const repaired = this.historyComplete;
		await this.persistCursor();
		// IndexedDB persistence yields to live handlers. Restore the obligation
		// if a new group or fault appeared while committing the repaired cursor.
		if (repaired && (!this.historyComplete || this.stopped || this.replayGroups.size > 0 ||
			this.activeLiveEvents > 0 || this.activeImports > 0 || this.replayFaultGeneration !== checkpoint)) {
			this.recordReplayFault(since);
			await this.persistCursor();
		}
		return this.historyComplete;
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

	// ── Event → change ─────────────────────────────────────────────

	/** Feed one signature-verified relay event to the core session; returns the
	 * decoded change when a full change (possibly reassembled from chunks) is
	 * available. The core owns reassembly, the cursor and replay bookkeeping. */
	private async eventToChange(event: Event): Promise<ImportItem | null> {
		this.stats.events++;
		this.onRawEvent?.(event);
		if (event.kind !== CHANGE_KIND || !verifyEvent(event)) return null;
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
		if (!r.item) return null;
		return {
			bytes: b64ToBytes(r.item.bytes),
			change: unpackCoreValueMaps<ChangeJSON>(r.item.change),
			chunkKey: r.item.chunkKey,
			provenance: r.item.provenance,
		};
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
			for (const item of batch) {
				if (item.provenance) {
					const p = item.provenance;
					const space = this.sharedSpaces.get(p.spaceId);
					if (!space) continue;
					const trustedSpace = computeObject(await this.store.changesFor(p.spaceId));
					const existing = computeObject(await this.store.changesFor(item.change.objectId));
					if (!authorizeSharedChange(item.change, p, space, this.pk, trustedSpace, existing)) continue;
				}
				this.stats.imported += await this.store.addChanges([item]);
				const p = item.provenance;
				await this.store.markPublished(p ? `${p.spaceId}/${p.keyId}/${item.change.id}` : item.change.id);
				if (item.chunkKey) {
					imported.add(item.chunkKey);
					this.settle(item.chunkKey, true);
				}
				this.pendingObjects.add(item.change.objectId);
			}
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
			const item = await this.eventToChange(event);
			if (item) await this.importBatch([item]);
		} catch (err) {
			this.recordReplayFault(1);
			throw err;
		} finally {
			this.activeLiveEvents--;
			await this.persistCursor();
		}
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
	 * re-offers every stored obligation on the next load.
	 */
	async publish(bytes: Uint8Array, changeId: string, objectId: string): Promise<void> {
		const candidates: PendingPublish[] = [{ key: changeId, changeId, objectId, bytes }];
		const space = this.sharedSpaces.get(this.spaceOf(objectId));
		if (space) candidates.push({ key: `${space.spaceId}/${space.keyId}/${changeId}`, changeId, objectId, bytes, spaceId: space.spaceId, keyId: space.keyId });
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
				bytes: bytesToB64(pending.bytes),
				spaceId: pending.spaceId,
				keyId: pending.keyId,
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
