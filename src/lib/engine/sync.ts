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

import { SimplePool, finalizeEvent, getPublicKey, nip44, type Event } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import type { ChangeJSON, ChangeStoreApi, RelaySyncApi, SyncEvents } from "./contracts";
import { proto } from "./proto";

export const DEFAULT_RELAYS = ["wss://roostr-relay.fly.dev"];

const CHANGE_KIND = 1078;
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
	/** Debug hook: every raw relay event before decrypt. */
	onRawEvent?: (event: Event) => void;
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
}

export class RelaySync implements RelaySyncApi {
	readonly stats: SyncStats = { events: 0, decryptFailures: 0, decodeFailures: 0, imported: 0, blindedTags: new Set() };

	private readonly pk: string;
	private readonly pool = new SimplePool();
	private readonly conversationKey: Uint8Array;
	private decode: ((bytes: Uint8Array) => ChangeJSON | null) | null;
	private readonly onRawEvent?: (event: Event) => void;

	private cursor = 0;
	private stopped = false;
	private sub: { close(): void } | null = null;

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
		this.sub = this.pool.subscribeMany(
			this.relays,
			{ kinds: [CHANGE_KIND], authors: [this.pk], since: this.cursor + 1 },
			{
				onevent: (event) => {
					this.liveChain = this.liveChain.then(() => this.handleLiveEvent(event)).catch(() => {});
				},
			},
		);
		this.events.onStatus({ phase: "live", imported: this.stats.imported });
	}

	stop(): void {
		this.stopped = true;
		// nostr-tools can race an in-flight REQ against connection teardown;
		// swallow so a stop() never throws into the caller.
		try {
			this.sub?.close();
		} catch {
			/* already closed */
		}
		this.sub = null;
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
					batch = await this.pool.querySync([relay], {
						kinds: [CHANGE_KIND],
						authors: [this.pk],
						since,
						until,
						limit: PAGE_LIMIT,
					});
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
	private async eventToChange(event: Event): Promise<{ bytes: Uint8Array; change: ChangeJSON } | null> {
		this.stats.events++;
		this.onRawEvent?.(event);
		const hTag = event.tags.find((t) => t[0] === "h")?.[1];
		if (hTag) this.stats.blindedTags.add(hTag);

		let part: string;
		try {
			part = nip44.decrypt(event.content, this.conversationKey);
		} catch {
			this.stats.decryptFailures++;
			return null;
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
		return { bytes, change };
	}

	private async importBatch(batch: Array<{ bytes: Uint8Array; change: ChangeJSON }>, immediateNotify = false): Promise<void> {
		if (batch.length > 0) {
			const added = await this.store.addChanges(batch);
			this.stats.imported += added;
			// Anything the relays hold must never be echoed back.
			for (const item of batch) await this.store.markPublished(item.change.id);
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
		if (this.queued.has(changeId)) return;
		this.queued.add(changeId);
		this.queue.push({ objectId, changeId, b64: bytesToB64(bytes), attempts: 0, notBefore: 0 });
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
			if (await this.store.isPublished(item.changeId)) {
				this.queued.delete(item.changeId);
				continue;
			}
			if (await this.publishOnce(item)) {
				await this.store.markPublished(item.changeId);
				this.queued.delete(item.changeId);
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
				const tags: string[][] = [["h", this.blind(item.objectId)]];
				if (gid) tags.push(["c", gid, String(i), String(parts.length)]);
				const event = finalizeEvent(
					{
						kind: CHANGE_KIND,
						created_at: Math.floor(Date.now() / 1000),
						tags,
						content: nip44.encrypt(parts[i], this.conversationKey),
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
