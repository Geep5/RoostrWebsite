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

import { SimplePool, finalizeEvent, getPublicKey, nip19, nip44, type Event } from "nostr-tools";
import { unwrapEvent, wrapEvent } from "nostr-tools/nip59";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { ChangeJSON, ChangeStoreApi, RelaySyncApi, SyncEvents } from "./contracts";
import { proto } from "./proto";
import { loadKey } from "./keys";
import { spaceKeyImport } from "./spacekeys";

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
	convKey: Uint8Array;
	spaceTag: string;
	writerSet: Set<string>;
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
	spaceKeyImport(inv.space, inv.key, inv.keyId || 1, ownerHex);
	return true;
}
const PUBLISH_SPACING_MS = 120;
const PAGE_SPACING_MS = 400;
const PAGE_LIMIT = 500;
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
	private readonly chunkGroups = new Map<string, { total: number; parts: Map<number, string> }>();

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
		if (fresh.length > 0) void this.backfillTags(fresh);
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

	/** Full walk of specific #h tags (joining a space mid-session). */
	private async backfillTags(tags: string[]): Promise<void> {
		const byId = new Map<string, Event>();
		await Promise.all(
			this.relays.map(async (relay) => {
				let until: number | undefined;
				for (;;) {
					if (this.stopped) return;
					let batch: Event[];
					try {
						batch = await this.pool.querySync([relay], { kinds: [CHANGE_KIND], "#h": tags, until, limit: PAGE_LIMIT });
					} catch {
						return;
					}
					if (batch.length === 0) return;
					let fresh = 0;
					for (const e of batch) {
						if (!byId.has(e.id)) {
							byId.set(e.id, e);
							fresh++;
						}
					}
					if (fresh === 0) return;
					until = Math.min(...batch.map((e) => e.created_at));
					await sleep(PAGE_SPACING_MS);
				}
			}),
		);
		const collected = [...byId.values()].sort((a, b) => a.created_at - b.created_at);
		const batch: Array<{ bytes: Uint8Array; change: ChangeJSON; space?: SharedSpace }> = [];
		for (const event of collected) {
			const item = await this.eventToChange(event);
			if (item) batch.push(item);
		}
		await this.importBatch(batch, true);
		await this.persistCursor();
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
		this.events.onStatus({ phase: "live", imported: this.stats.imported });
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
				this.events.onStatus({ phase: "live", imported: this.stats.imported });
				return;
			}
			const head = [...res[0], ...res[1]].reduce((max, e) => Math.max(max, e.created_at), 0);
			if (head > this.cursor) {
				this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: "catching up" });
				await this.catchupSince(this.cursor + 1);
				this.subscribeLive();
				this.events.onStatus({ phase: "live", imported: this.stats.imported });
			}
		} finally {
			this.watchdogBusy = false;
		}
	}

	private async catchupSince(since: number): Promise<void> {
		const spaceTags = [...this.sharedSpaces.values()].map((sp) => sp.spaceTag);
		const byId = new Map<string, Event>();
		const filters: Array<Parameters<SimplePool["querySync"]>[1]> = [{ kinds: [CHANGE_KIND], authors: [this.pk], since }];
		if (spaceTags.length > 0) filters.push({ kinds: [CHANGE_KIND], "#h": spaceTags, since });
		for (const filter of filters) {
			try {
				for (const e of await this.pool.querySync(this.relays, filter)) byId.set(e.id, e);
			} catch {
				/* next watchdog tick retries */
			}
		}
		const events = [...byId.values()].sort((a, b) => a.created_at - b.created_at);
		const batch: Array<{ bytes: Uint8Array; change: ChangeJSON; space?: SharedSpace }> = [];
		for (const event of events) {
			const item = await this.eventToChange(event);
			if (item) batch.push(item);
		}
		await this.importBatch(batch, true);
		await this.persistCursor();
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
	private async backfill(since: number): Promise<boolean> {
		const byId = new Map<string, Event>();
		let complete = true;

		// Per-relay backwards pagination. Merged multi-relay paging is gappy:
		// each relay truncates to `limit` independently, so taking
		// `until = min(merged page)` jumps below another relay's truncation
		// point and permanently skips whatever that relay still held between
		// the two stamps. Paging each relay by its own oldest-returned stamp
		// (kept inclusive, deduped by event id) is gapless.
		let pages = 0;
		const pageRelay = async (relay: string): Promise<void> => {
			const seenHere = new Set<string>();
			let until: number | undefined;
			for (;;) {
				if (this.stopped) {
					complete = false;
					return;
				}
				let batch: Event[];
				try {
					const spaceTags = [...this.sharedSpaces.values()].map((sp) => sp.spaceTag);
					const pages = await Promise.all([
						this.pool.querySync([relay], { kinds: [CHANGE_KIND], authors: [this.pk], since, until, limit: PAGE_LIMIT }),
						spaceTags.length > 0
							? this.pool.querySync([relay], { kinds: [CHANGE_KIND], "#h": spaceTags, since, until, limit: PAGE_LIMIT })
							: Promise.resolve([] as Event[]),
					]);
					batch = [...pages[0], ...pages[1]];
				} catch (err) {
					// One relay failing must not abort the others - but an
					// errored walk is not a complete one.
					complete = false;
					this.events.onStatus({ phase: "backfill", detail: `${relay}: ${String(err).slice(0, 80)}` });
					return;
				}
				if (batch.length === 0) return;
				let freshCount = 0;
				for (const e of batch) {
					if (seenHere.has(e.id)) continue;
					seenHere.add(e.id);
					freshCount++;
					if (!byId.has(e.id)) byId.set(e.id, e);
				}
				// A page of nothing but already-seen boundary events means this
				// relay is exhausted down to `since`.
				if (freshCount === 0) return;
				// `until` stays INCLUSIVE (no -1): boundary events re-arrive on
				// the next page and dedup by id, so equal adjacent timestamps
				// cannot fall through the crack.
				until = Math.min(...batch.map((e) => e.created_at));
				pages++;
				this.events.onStatus({
					phase: "backfill",
					imported: this.stats.imported,
					detail: `page ${pages} (${relay}): ${byId.size} event(s) so far`,
				});
				await sleep(PAGE_SPACING_MS); // pace REQs — public relays rate-limit bursts
			}
		};
		await Promise.all(this.relays.map(pageRelay));
		const collected = [...byId.values()];

		// Ascending so multi-part chunk groups assemble in one pass and the
		// cursor advances monotonically.
		collected.sort((a, b) => a.created_at - b.created_at);
		const batch: Array<{ bytes: Uint8Array; change: ChangeJSON }> = [];
		for (const event of collected) {
			const item = await this.eventToChange(event);
			if (item) batch.push(item);
		}
		await this.importBatch(batch, /*immediateNotify=*/ true);
		await this.persistCursor();
		this.events.onStatus({ phase: "backfill", imported: this.stats.imported, detail: `backfill done: ${collected.length} event(s)` });
		return complete;
	}

	// ── Event → change ─────────────────────────────────────────────

	/** Decrypt one relay event; returns decoded change bytes when a full
	 * change (possibly reassembled from chunks) becomes available. */
	private async eventToChange(event: Event): Promise<{ bytes: Uint8Array; change: ChangeJSON; space?: SharedSpace } | null> {
		this.stats.events++;
		this.onRawEvent?.(event);
		const hTag = event.tags.find((t) => t[0] === "h")?.[1];
		if (hTag) this.stats.blindedTags.add(hTag);

		let part: string | null = null;
		let space: SharedSpace | undefined;
		try {
			part = nip44.decrypt(event.content, this.conversationKey);
		} catch {
			for (const sp of this.sharedSpaces.values()) {
				try {
					part = nip44.decrypt(event.content, sp.convKey);
					// Writer gate: viewers (and leaked keys) can produce valid
					// ciphertext - only allowed writers get applied.
					if (!sp.writerSet.has(event.pubkey)) return null;
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
		if (event.created_at > this.cursor) this.cursor = event.created_at;

		const chunkTag = event.tags.find((t) => t[0] === "c");
		let full: string;
		if (!chunkTag) {
			full = part;
		} else {
			const [, gid, idxStr, totalStr] = chunkTag;
			const total = parseInt(totalStr, 10);
			if (!gid || !Number.isFinite(total) || total < 2 || total > 64) return null;
			let group = this.chunkGroups.get(gid);
			if (!group) {
				group = { total, parts: new Map() };
				this.chunkGroups.set(gid, group);
			}
			group.parts.set(parseInt(idxStr, 10), part);
			if (group.parts.size !== group.total) return null;
			this.chunkGroups.delete(gid);
			full = "";
			for (let i = 0; i < group.total; i++) full += group.parts.get(i) ?? "";
		}

		let bytes: Uint8Array;
		try {
			bytes = b64ToBytes(full);
		} catch {
			this.stats.decodeFailures++;
			return null;
		}
		const decode = await this.getDecode();
		const change = decode(bytes);
		if (!change) {
			this.stats.decodeFailures++;
			return null;
		}
		return { bytes, change, space };
	}

	private async importBatch(batch: Array<{ bytes: Uint8Array; change: ChangeJSON; space?: SharedSpace }>, immediateNotify = false): Promise<void> {
		if (batch.length > 0) {
			const added = await this.store.addChanges(batch);
			this.stats.imported += added;
			// Anything the relays hold must never be echoed back.
			for (const item of batch) {
				await this.store.markPublished(item.space ? `${item.space.spaceId}/${item.space.keyId}/${item.change.id}` : item.change.id);
			}
			for (const item of batch) this.pendingObjects.add(item.change.objectId);
		}
		if (immediateNotify) this.flushObjectNotify();
		else this.scheduleObjectNotify();
	}

	private async persistCursor(): Promise<void> {
		if (this.cursor > (await this.store.getCursor())) await this.store.setCursor(this.cursor);
	}

	private async handleLiveEvent(event: Event): Promise<void> {
		const item = await this.eventToChange(event);
		if (item) await this.importBatch([item]);
		await this.persistCursor();
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

	publish(bytes: Uint8Array, changeId: string, objectId: string): void {
		const space = this.sharedSpaces.get(this.spaceOf(objectId));
		const key = space ? `${space.spaceId}/${space.keyId}/${changeId}` : changeId;
		if (this.queued.has(key)) return;
		this.queued.add(key);
		this.queue.push({ objectId, changeId, b64: bytesToB64(bytes), attempts: 0, notBefore: 0, space });
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
			const key = item.space ? `${item.space.spaceId}/${item.space.keyId}/${item.changeId}` : item.changeId;
			if (await this.store.isPublished(key)) {
				this.queued.delete(key);
				continue;
			}
			if (await this.publishOnce(item)) {
				await this.store.markPublished(key);
				this.queued.delete(key);
			} else {
				item.attempts++;
				item.notBefore = Date.now() + Math.min(300_000, 2000 * 2 ** item.attempts);
				this.queue.push(item);
			}
			await sleep(PUBLISH_SPACING_MS);
		}
		this.queueRunning = false;
	}

	private async publishOnce(item: PublishItem): Promise<boolean> {
		const parts: string[] = [];
		for (let i = 0; i < item.b64.length; i += CHUNK_CHARS) parts.push(item.b64.slice(i, i + CHUNK_CHARS));
		// Content-derived group id: retries republish identical parts and
		// receivers dedupe naturally.
		const gid = parts.length > 1 ? bytesToHex(sha256(new TextEncoder().encode(item.b64))).slice(0, 16) : "";
		try {
			for (let i = 0; i < parts.length; i++) {
				const tags: string[][] = item.space
					? [["h", blindShared(item.space.keyHex, item.objectId)], ["h", item.space.spaceTag]]
					: [["h", this.blind(item.objectId)]];
				if (gid) tags.push(["c", gid, String(i), String(parts.length)]);
				const event = finalizeEvent(
					{
						kind: CHANGE_KIND,
						created_at: Math.floor(Date.now() / 1000),
						tags,
						content: nip44.encrypt(parts[i], item.space ? item.space.convKey : this.conversationKey),
					},
					this.sk,
				);
				await Promise.any(this.pool.publish(this.relays, event));
				if (parts.length > 1 && i < parts.length - 1) await sleep(PUBLISH_SPACING_MS);
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
