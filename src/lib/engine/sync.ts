/**
 * sync.ts — relay backfill/live/publish (RelaySyncApi).
 *
 * Wire schema mirrors the desktop daemon (glonOdin/harness/src/nostrsync.ts):
 *   - kind 1078 events, authored by our own key.
 *   - content = NIP-44 self-encryption (conversation key of sk with our own
 *     pk) of the base64 of the raw Change protobuf bytes.
 *   - 'h' tag = blinded object id: sha256(sk || objectId utf8) hex[0:16].
 *   - big changes are split into ≤40k-char base64 parts carried in
 *     ['c', groupId, index, total] tags; groupId = sha256(b64) hex[0:16].
 *   - keyring events (kind 30078, d='roostr-keyring') are NOT handled in
 *     v1 — desktop remains the keyring authority for now.
 *
 * Backfill pages querySync backwards via `until` (since cursor+1), paced
 * between pages so public relays don't rate-limit us; live is a
 * subscribeMany since cursor+1. Publishing is a paced queue (one event per
 * PUBLISH_SPACING_MS) with exponential backoff, mirroring the daemon's
 * publishOnce loop.
 */

import { SimplePool, finalizeEvent, getPublicKey, nip19, nip44, verifyEvent, type Event } from "nostr-tools";
import { unwrapEvent, wrapEvent } from "nostr-tools/nip59";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { ChangeJSON, ChangeStoreApi, PendingPublish, RelaySyncApi, SharedProvenance, SyncEvents } from "./contracts";
import type { ObjectJSON } from "$lib/types";
import { computeObject } from "./replay";
import { proto } from "./proto";
import { loadKey } from "./keys";
import { spaceKeyGet, spaceKeyImport } from "./spacekeys";

export const DEFAULT_RELAYS = ["wss://roostr-relay.fly.dev"];

const CHANGE_KIND = 1078;
const EMPTY_CHANGE_ID_FIELD = new Uint8Array([0x0a, 0x00]);
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
	convKey: Uint8Array;
	spaceTag: string;
	writerSet: Set<string>;
}

const OWNER_FIELDS: Record<string, true> = { members: true, owner: true, keyId: true, key: true, keys: true, served_by: true, machine: true, machine_id: true, machineId: true, bound_object: true };
const CONTROL_TYPES: Record<string, true> = { machine: true, agent: true, program: true, typescript: true, skill: true, peer: true };

function sameValue(a: unknown, b: unknown): boolean {
	if (a === b) return true;
	if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
	if (Array.isArray(a) !== Array.isArray(b)) return false;
	const left = a as Record<string, unknown>, right = b as Record<string, unknown>;
	const keys = Object.keys(left);
	return keys.length === Object.keys(right).length && keys.every((k) => Object.hasOwn(right, k) && sameValue(left[k], right[k]));
}

/** Pure authority gate. Existing scope and privileges never come from the candidate. */
export function authorizeSharedChange(change: ChangeJSON, provenance: SharedProvenance, space: SharedSpaceInfo,
	localPk: string, trustedSpace: ObjectJSON | null, existing: ObjectJSON | null): boolean {
	if (provenance.spaceId !== space.spaceId || provenance.keyId !== space.keyId) return false;
	const owner = provenance.signer === (space.owner || localPk);
	if (trustedSpace && (trustedSpace.typeKey !== "channel" || trustedSpace.id !== space.spaceId || trustedSpace.deleted)) return false;
	if (!owner) {
		const members = trustedSpace?.fields.members?.valuesValue?.items ?? [];
		if (!members.some((m) => m.mapValue?.entries?.role?.stringValue === "writer" &&
			npubToHex(m.mapValue?.entries?.npub?.stringValue ?? "") === provenance.signer)) return false;
	}
	if (!change.objectId || change.objectId === "__vanished__") return false;
	if (existing) {
		const scope = existing.typeKey === "channel" ? existing.id : existing.fields.channel?.stringValue;
		if (scope !== space.spaceId || existing.typeKey === "vanish_log") return false;
		if (!owner && Object.hasOwn(CONTROL_TYPES, existing.typeKey)) return false;
	}
	let type = existing?.typeKey ?? "";
	let scoped = !!existing;
	if (change.snapshot != null) {
		const snap = change.snapshot as { id?: string; typeKey?: string; fields?: Record<string, unknown>; deleted?: boolean };
		if (typeof snap !== "object" || Array.isArray(snap) || snap.id !== change.objectId || !snap.typeKey) return false;
		if (existing && snap.typeKey !== existing.typeKey) return false;
		type = snap.typeKey;
		const state = computeObject([{ ...change, ops: [] }]);
		if (!state) return false;
		scoped = type === "channel" ? change.objectId === space.spaceId : state.fields.channel?.stringValue === space.spaceId;
		if (type === "channel" && Object.hasOwn(state.fields, "channel")) return false;
		if (!scoped) return false;
		if (!owner) {
			if (type === "channel" && (!existing || !!snap.deleted !== !!existing.deleted)) return false;
			for (const key of Object.keys(OWNER_FIELDS)) if (!sameValue(state.fields[key], existing?.fields[key])) return false;
		}
	}
	for (const op of change.ops) {
		if (Object.keys(op).length !== 1) return false;
		if (op.objectCreate) {
			if (!op.objectCreate.typeKey || (type && op.objectCreate.typeKey !== type)) return false;
			type = op.objectCreate.typeKey;
			if (type === "channel") {
				if (!owner || change.objectId !== space.spaceId) return false;
				scoped = true;
			}
		}
		if (op.fieldSet || op.fieldDelete) {
			const key = op.fieldSet?.key ?? op.fieldDelete!.key;
			if (!owner && Object.hasOwn(OWNER_FIELDS, key)) return false;
			if (key === "channel") {
				if (change.objectId === space.spaceId || !op.fieldSet || op.fieldSet.value.stringValue !== space.spaceId) return false;
				scoped = true;
			}
		}
		if (op.objectDelete && type === "channel" && !owner) return false;
	}
	return scoped && !!type && type !== "vanish_log" && (owner || !Object.hasOwn(CONTROL_TYPES, type)) &&
		(type !== "channel" || change.objectId === space.spaceId);
}

interface ImportItem {
	bytes: Uint8Array;
	change: ChangeJSON;
	provenance?: SharedProvenance;
	chunkKey?: string;
}

function utf8(s: string): Uint8Array {
	return new TextEncoder().encode(s);
}

export function blindShared(keyHex: string, id: string): string {
	const a = utf8(keyHex);
	const b = utf8(id);
	const buf = new Uint8Array(a.length + b.length);
	buf.set(a);
	buf.set(b, a.length);
	return bytesToHex(sha256(buf)).slice(0, 16);
}

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
const CHUNK_CHARS = 40_000;
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
	/** Override change decoding (tests / proto not yet loaded). */
	decode?: (bytes: Uint8Array) => ChangeJSON | null;
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

interface PublishItem {
	objectId: string;
	changeId: string;
	b64: string;
	attempts: number;
	notBefore: number;
	space?: SharedSpace;
	pending: PendingPublish;
}

export class RelaySync implements RelaySyncApi {
	readonly stats: SyncStats = { events: 0, decryptFailures: 0, decodeFailures: 0, imported: 0, blindedTags: new Set() };

	private readonly pk: string;
	private pool = new SimplePool();
	private readonly conversationKey: Uint8Array;
	private decode: ((bytes: Uint8Array) => ChangeJSON | null) | null;
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

	/** Reassembly buffer for chunked changes: gid → parts. */
	private readonly chunkGroups = new Map<string, { total: number; parts: Map<number, string>; bytes: number; expires: number; at: number }>();
	// Metadata survives buffer expiry/limits; unlike ciphertext, its cardinality
	// follows unresolved changes. Successful group keys suppress replay suffixes.
	private readonly replayGroups = new Map<string, number>();
	private readonly importedChunkGroups = new Set<string>();
	private readonly importingChunkGroups = new Set<string>();
	private replayGroupsLoaded: Promise<void> | null = null;
	private chunkBytes = 0;
	private importChain: Promise<void> = Promise.resolve();
	private historyComplete = false;
	private discardedChunkFloor = Infinity;
	private replayFaultGeneration = 0;
	private activeLiveEvents = 0;
	private activeImports = 0;
	private cursorChain: Promise<void> = Promise.resolve();
	private backfillChain: Promise<boolean> = Promise.resolve(false);

	private readonly queue: PublishItem[] = [];
	private readonly queued = new Set<string>();
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
		this.conversationKey = nip44.getConversationKey(sk, this.pk);
		this.decode = options.decode ?? null;
		this.onRawEvent = options.onRawEvent;
		this.onSpaceKey = options.onSpaceKey;
		this.spaceOf = options.spaceOf ?? (() => "");
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
				convKey: hexToBytes(info.keyHex),
				spaceTag: blindShared(info.keyHex, `space:${info.spaceId}`),
				writerSet: new Set(info.writers),
			});
		}
		this.sharedSpaces = next;
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


	/** Blinded object tag: sha256(sk || objectId) hex prefix, as the daemon. */
	private blind(objectId: string): string {
		const idBytes = new TextEncoder().encode(objectId);
		const buf = new Uint8Array(this.sk.length + idBytes.length);
		buf.set(this.sk);
		buf.set(idBytes, this.sk.length);
		return bytesToHex(sha256(buf)).slice(0, 16);
	}

	private async getDecode(): Promise<(bytes: Uint8Array) => ChangeJSON | null> {
		// backend.ts already loads proto statically; no circularity exists
		// (proto has no sync import), so the static graph is honest here.
		this.decode ??= (bytes: Uint8Array) => proto.decodeChange(bytes);
		return this.decode;
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
		for (const pending of await this.store.pendingPublishes()) this.enqueue(pending);
		this.events.onStatus({ phase: "backfill", imported: 0 });

		// The incremental `since = cursor+1` shortcut is only sound once ONE
		// full history walk has completed on this device: the cursor tracks
		// the NEWEST imported event, so an interrupted or partially-failed
		// first bootstrap would otherwise skip everything older, forever.
		const bootstrapped = await this.store.getBootstrapped();
		try {
			const complete = await this.backfill(bootstrapped ? this.cursor + 1 : 1);
			if (!bootstrapped && complete) await this.store.setBootstrapped();
		} catch (err) {
			this.events.onStatus({ phase: "error", detail: err instanceof Error ? err.message : String(err) });
			// fall through to live anyway — partial backfill is still progress
		}

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
		this.events.onStatus({ phase: this.historyComplete ? "live" : "backfill", imported: this.stats.imported });
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
	}

	/**
	 * Deafness watchdog: subscriptions do not survive socket drops and
	 * nostr-tools reports nothing when they die - the dot would stay
	 * green forever. Compare the relay's head against our cursor; when
	 * we are behind (or the relay stops answering), surface an honest
	 * "catching up" status, recover, and go live again.
	 */
	private async watchdog(): Promise<void> {
		if (this.stopped || this.watchdogBusy) return;
		this.watchdogBusy = true;
		try {
			if (!this.historyComplete || this.chunkGroups.size > 0 || this.discardedChunkFloor !== Infinity) {
				const complete = await this.backfill(await this.store.getBootstrapped() ? await this.store.getCursor() : 1);
				if (complete) await this.store.setBootstrapped();
				this.subscribeLive();
				this.events.onStatus({ phase: complete ? "live" : "backfill", imported: this.stats.imported });
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
				this.events.onStatus({ phase: this.historyComplete ? "live" : "backfill", imported: this.stats.imported });
				return;
			}
			const head = [...res[0], ...res[1]].reduce((max, e) => Math.max(max, e.created_at), 0);
			if (head > this.cursor) {
				this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: "catching up" });
				await this.catchupSince(this.cursor + 1);
				this.subscribeLive();
				this.events.onStatus({ phase: this.historyComplete ? "live" : "backfill", imported: this.stats.imported });
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
		this.chunkGroups.clear();
		this.chunkBytes = 0;
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
	private backfill(since: number): Promise<boolean> {
		const run = this.backfillChain.then(async () => {
			try {
				return await this.walkHistory(since);
			} catch (err) {
				this.recordReplayFault(1);
				await this.persistCursor();
				throw err;
			}
		});
		this.backfillChain = run.catch(() => false);
		return run;
	}

	private async walkHistory(since: number): Promise<boolean> {
		this.historyComplete = false;
		await this.loadReplayGroups();
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
			let until: number | undefined;
			try {
				for (;;) {
					if (this.stopped) { complete = false; return; }
					const page = await this.queryRelayPage(relay, { ...filter, until, limit: PAGE_LIMIT });
					for (const event of page) byId.set(event.id, event);
					if (page.length < PAGE_LIMIT) return;
					const oldest = Math.min(...page.map((e) => e.created_at));
					if (until !== undefined && oldest >= until) throw new Error("saturated same-timestamp history page");
					until = oldest;
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
		if (this.historyComplete) this.discardedChunkFloor = Infinity;
		else this.recordReplayFault(since);
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

	/** Decrypt one relay event; returns decoded change bytes when a full
	 * change (possibly reassembled from chunks) becomes available. */
	private async eventToChange(event: Event): Promise<ImportItem | null> {
		this.stats.events++;
		this.onRawEvent?.(event);
		if (event.kind !== CHANGE_KIND || !verifyEvent(event)) return null;
		const hTag = event.tags.find((t) => t[0] === "h")?.[1];
		if (hTag) this.stats.blindedTags.add(hTag);

		let part: string | null = null;
		let space: SharedSpace | undefined;
		try {
			part = nip44.decrypt(event.content, this.conversationKey);
			if (event.pubkey !== this.pk) return null;
		} catch {
			for (const sp of this.sharedSpaces.values()) {
				try {
				if (!event.tags.some((tag) => tag[0] === "h" && tag[1] === sp.spaceTag)) continue;
					part = nip44.decrypt(event.content, sp.convKey);
					space = sp;
					break;
				} catch {
					/* next key */
				}
			}
			if (part === null) {
				this.stats.decryptFailures++;
				return null;
			}
		}
		if (part.length > CHUNK_CHARS || !/^[A-Za-z0-9+/]*={0,2}$/.test(part)) return null;
		if (event.tags.filter((t) => t[0] === "c").length > 1) return null;
		if (event.created_at > this.cursor) this.cursor = event.created_at;

		const chunkTag = event.tags.find((t) => t[0] === "c");
		let full: string;
		let replayAt = event.created_at;
		let chunkKey: string | undefined;
		if (!chunkTag) {
			full = part;
		} else {
			const [, gid, idxStr, totalStr] = chunkTag;
			const total = Number(totalStr), index = Number(idxStr);
			if (!/^[0-9a-f]{16}$/.test(gid ?? "") || !/^[0-9]+$/.test(totalStr ?? "") || !/^[0-9]+$/.test(idxStr ?? "") ||
				!Number.isInteger(total) || total < 2 || total > 64 || !Number.isInteger(index) || index < 0 || index >= total || part.length > CHUNK_CHARS) return null;
			const now = Date.now();
			for (const [key, group] of this.chunkGroups) if (group.expires <= now) {
				this.recordReplayFault(group.at);
				this.chunkBytes -= group.bytes;
				this.chunkGroups.delete(key);
			}
			const key = JSON.stringify([event.pubkey, space?.spaceId ?? "", space?.keyId ?? 0, gid]);
			if (this.importedChunkGroups.has(key) || this.importingChunkGroups.has(key)) return null;
			this.replayGroups.set(key, Math.min(this.replayGroups.get(key) ?? Infinity, event.created_at));
			chunkKey = key;
			let group = this.chunkGroups.get(key);
			if (group && (group.total !== total || (group.parts.has(index) && group.parts.get(index) !== part))) {
				this.recordReplayFault(Math.min(group.at, event.created_at));
				this.chunkBytes -= group.bytes;
				this.chunkGroups.delete(key);
				return null;
			}
			if (!group) {
				if (this.chunkGroups.size >= 128) {
					this.recordReplayFault(event.created_at);
					return null;
				}
				group = { total, parts: new Map(), bytes: 0, expires: now + 300_000, at: this.replayGroups.get(key)! };
				this.chunkGroups.set(key, group);
			}
			group.at = Math.min(group.at, event.created_at);
			if (!group.parts.has(index)) {
				if (this.chunkBytes + part.length > 16 * 1024 * 1024) {
					this.recordReplayFault(group.at);
					return null;
				}
				group.parts.set(index, part);
				group.bytes += part.length;
				this.chunkBytes += part.length;
			}
			if (group.parts.size !== group.total) return null;
			replayAt = group.at;
			this.chunkGroups.delete(key);
			this.chunkBytes -= group.bytes;
			full = Array.from({ length: group.total }, (_, i) => group!.parts.get(i)!).join("");
			if (bytesToHex(sha256(utf8(full))).slice(0, 16) !== gid) {
				this.recordReplayFault(group.at);
				return null;
			}
		}

		let bytes: Uint8Array;
		try {
			bytes = b64ToBytes(full);
		} catch {
			this.stats.decodeFailures++;
			this.recordReplayFault(replayAt);
			return null;
		}
		const decode = await this.getDecode();
		const change = decode(bytes);
		// The content address covers the received wire representation, not a
		// canonical re-encoding that can drop legacy explicit default fields.
		const rawId = bytes.length >= 34 && bytes[0] === 0x0a && bytes[1] === 0x20
			? bytesToHex(sha256.create().update(EMPTY_CHANGE_ID_FIELD).update(bytes.subarray(34)).digest())
			: null;
		if (!change || change.id !== rawId) {
			this.stats.decodeFailures++;
			this.recordReplayFault(replayAt);
			return null;
		}
		if (chunkKey) this.importingChunkGroups.add(chunkKey);
		return { bytes, change, chunkKey, provenance: space ? { spaceId: space.spaceId, keyId: space.keyId, signer: event.pubkey } : undefined };
	}

	private importBatch(batch: ImportItem[], immediateNotify = false): Promise<void> {
		this.activeImports++;
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
					this.importedChunkGroups.add(item.chunkKey);
					this.replayGroups.delete(item.chunkKey);
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
			for (const item of batch) if (item.chunkKey) this.importingChunkGroups.delete(item.chunkKey);
			this.activeImports--;
		});
		this.importChain = run.catch(() => {});
		return run;
	}

	private recordReplayFault(at: number): void {
		this.replayFaultGeneration++;
		this.discardedChunkFloor = Math.min(this.discardedChunkFloor, at);
		this.historyComplete = false;
	}

	private loadReplayGroups(): Promise<void> {
		return this.replayGroupsLoaded ??= this.store.getReplayGroups().then((groups) => {
			for (const [key, at] of groups) {
				if (!this.importedChunkGroups.has(key)) this.replayGroups.set(key, Math.min(this.replayGroups.get(key) ?? Infinity, at));
			}
		});
	}

	private persistCursor(): Promise<void> {
		const run = this.cursorChain.then(async () => {
			await this.loadReplayGroups();
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

	async publish(bytes: Uint8Array, changeId: string, objectId: string): Promise<void> {
		const pending: PendingPublish[] = [{ key: changeId, changeId, objectId, bytes }];
		const space = this.sharedSpaces.get(this.spaceOf(objectId));
		if (space) pending.push({ key: `${space.spaceId}/${space.keyId}/${changeId}`, changeId, objectId, bytes, spaceId: space.spaceId, keyId: space.keyId });
		for (const item of pending) {
			if (this.queued.has(item.key) || await this.store.isPublished(item.key)) continue;
			const saved = await this.store.getPending(item.key);
			if (!saved) await this.store.savePending(item);
			this.enqueue(saved ?? item);
		}
	}

	private enqueue(pending: PendingPublish): void {
		if (this.stopped || this.queued.has(pending.key)) return;
		const space = pending.spaceId ? this.sharedSpaces.get(pending.spaceId) : undefined;
		// A rotated key cannot recreate old ciphertext; retain the obligation for export.
		if (pending.spaceId && (!space || space.keyId !== pending.keyId) && !pending.events) return;
		this.queued.add(pending.key);
		this.queue.push({ objectId: pending.objectId, changeId: pending.changeId, b64: bytesToB64(pending.bytes), attempts: 0, notBefore: 0, space, pending });
		if (!this.queueRunning) {
			this.queueRunning = true;
			void this.runPublishQueue();
		}
	}

	/** Paced, eventually-durable publish loop (daemon's publishOnce shape):
	 * one event per PUBLISH_SPACING_MS, failures re-queued with exponential
	 * backoff, never dropped while the sync lives. */
	private async runPublishQueue(): Promise<void> {
		while (!this.stopped) {
			const now = Date.now();
			const idx = this.queue.findIndex((q) => q.notBefore <= now);
			if (idx === -1) {
				if (this.queue.length === 0) break;
				await sleep(500);
				continue;
			}
			const [item] = this.queue.splice(idx, 1);
			const key = item.pending.key;
			try {
				if (await this.store.isPublished(key)) {
					await this.store.markPublished(key);
					this.queued.delete(key);
					continue;
				}
				if (!await this.publishOnce(item)) throw new Error("publication pending");
				await this.store.markPublished(key);
				this.queued.delete(key);
			} catch {
				item.attempts++;
				item.notBefore = Date.now() + Math.min(300_000, 2000 * 2 ** item.attempts);
				this.queue.push(item);
			}
			await sleep(PUBLISH_SPACING_MS);
		}
		this.queueRunning = false;
	}

	private async publishOnce(item: PublishItem): Promise<boolean> {
		try {
			if (!item.pending.events) {
				const parts: string[] = [];
				for (let i = 0; i < item.b64.length; i += CHUNK_CHARS) parts.push(item.b64.slice(i, i + CHUNK_CHARS));
				if (parts.length > 64) throw new Error("change exceeds chunk limit");
				const gid = parts.length > 1 ? bytesToHex(sha256(utf8(item.b64))).slice(0, 16) : "";
				item.pending.events = parts.map((part, i) => {
					const tags: string[][] = item.space
						? [["h", blindShared(item.space.keyHex, item.objectId)], ["h", item.space.spaceTag]]
						: [["h", this.blind(item.objectId)]];
					if (gid) tags.push(["c", gid, String(i), String(parts.length)]);
					return finalizeEvent({ kind: CHANGE_KIND, created_at: Math.floor(Date.now() / 1000), tags,
						content: nip44.encrypt(part, item.space ? item.space.convKey : this.conversationKey) }, this.sk);
				});
			}
			// Persist BEFORE sending, including after any failed IndexedDB attempt.
			await this.store.savePending(item.pending);
			for (const event of item.pending.events) {
				await Promise.any(this.pool.publish(this.relays, event));
				if (item.pending.events.length > 1) await sleep(PUBLISH_SPACING_MS);
			}
			return true;
		} catch (err) {
			const detail =
				err instanceof AggregateError
					? err.errors.map((e) => String(e).slice(0, 80)).join(" | ")
					: String(err).slice(0, 120);
			this.events.onStatus({ phase: "error", detail: `publish rejected (attempt ${item.attempts + 1}): ${detail}` });
			return false;
		}
	}
}
