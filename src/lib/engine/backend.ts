/**
 * The browser replica backend: ties store + replay + query + sync + mutate
 * together behind the same surface the desktop app gets from the Odin
 * server. All state lives on-device; relays are the only network.
 */

import type { ObjectJSON, ObjectSummary, SpaceJSON, RelationDefJSON, ValueJSON } from "$lib/types";
import type { ChangeJSON, QueryBody } from "./contracts";
import { decodeChange, encodeChange, changeId } from "./proto";
import { sha256 } from "@noble/hashes/sha2.js";

const HEX = "0123456789abcdef";
const toHex = (b: Uint8Array): string => {
	let out = "";
	for (const x of b) out += HEX[x >> 4] + HEX[x & 15];
	return out;
};
import { computeObject } from "./replay";
import { runQuery } from "./query";
import { ChangeStore, destroyDatabase } from "./store";
import { RelaySync, DEFAULT_RELAYS, npubToHex, type SharedSpaceInfo } from "./sync";
import { spaceKeyAll } from "./spacekeys";
import { getPublicKey } from "nostr-tools";
import { loadKey, authorIdFor } from "./keys";
import { runMutation } from "./mutate";

function fstr(fields: Record<string, ValueJSON> | undefined, k: string): string {
	return fields?.[k]?.stringValue ?? "";
}

/** The synced vanish ledger object and its field prefix (src/vanish.odin). */
const VANISH_LOG_ID = "__vanished__";
const VANISH_FIELD = "vanished:";

export interface SyncStatus {
	phase: "idle" | "backfill" | "live" | "error";
	imported: number;
	detail?: string;
	/** One full history walk has completed on this device. */
	bootstrapped: boolean;
}

class WebBackend {
	private store = new ChangeStore();
	private sync: RelaySync | null = null;
	private states = new Map<string, ObjectJSON>();
	/** Object ids the synced ledger says are gone; rebuilt when it commits. */
	private vanished = new Set<string>();
	private dirty = new Set<string>();
	private allDirty = true;
	private commitListeners = new Set<(ids: string[]) => void>();
	status: SyncStatus = { phase: "idle", imported: 0, bootstrapped: false };
	private statusListeners = new Set<(s: SyncStatus) => void>();
	author = "";
	private started = false;

	async start(): Promise<void> {
		if (this.started) return;
		const key = loadKey();
		if (!key) throw new Error("no key");
		this.started = true;
		this.author = authorIdFor(key);
		await this.store.open();
		this.allDirty = true;
		this.sync = new RelaySync(key.sk, this.relays(), this.store, {
			onObjects: (ids) => {
				for (const id of ids) this.dirty.add(id);
				const cb = [...this.commitListeners];
				for (const fn of cb) fn(ids);
				// A synced commit on a space object can change members/keys.
				if (ids.some((id) => this.states.get(id)?.typeKey === "channel")) void this.refreshShared();
			},
			onStatus: (s) => {
				void this.store.getBootstrapped().then((b) => {
					this.status = { phase: s.phase, imported: s.imported ?? this.status.imported, detail: s.detail, bootstrapped: b };
					for (const fn of this.statusListeners) fn(this.status);
				});
			},
		}, {
			onSpaceKey: () => void this.refreshShared(),
			spaceOf: (objectId) => {
				const o = this.states.get(objectId);
				if (!o) return "";
				return o.typeKey === "channel" ? o.id : (o.fields["channel"]?.stringValue ?? "");
			},
		});
		await this.refreshShared();
		void this.sync.start().then(() => this.refreshShared());
	}

	/**
	 * Shared-space reconcile: build the shared view from local space keys +
	 * member fields, hand it to the sync layer, publish the relay
	 * write-allowlist for owned spaces, and (re)queue a space's whole
	 * history under its key the first time it becomes shared (or its key
	 * rotates) - that republish is what makes joiners see the space.
	 */
	private async refreshShared(): Promise<void> {
		if (!this.sync) return;
		await this.ensure();
		const key = loadKey();
		if (!key) return;
		const myPk = getPublicKey(key.sk);
		const infos: SharedSpaceInfo[] = [];
		const memberHexesBySpace = new Map<string, string[]>();
		const nameBySpace = new Map<string, string>();
		for (const [spaceId, entry] of Object.entries(spaceKeyAll())) {
			if (!entry.key || entry.key.length !== 64) continue;
			const stateObj = this.states.get(spaceId);
			const writers = new Set<string>([entry.owner ?? myPk]);
			const memberHexes: string[] = [];
			const items = stateObj?.fields["members"]?.valuesValue?.items ?? [];
			for (const item of items) {
				const entries = (item as { mapValue?: { entries?: Record<string, { stringValue?: string }> } }).mapValue?.entries ?? {};
				const npub = entries["npub"]?.stringValue ?? "";
				const role = entries["role"]?.stringValue ?? "writer";
				const hex = npubToHex(npub);
				if (hex) {
					memberHexes.push(hex);
					if (role !== "viewer") writers.add(hex);
				}
			}
			if (memberHexes.length === 0 && !entry.owner) continue;
			memberHexesBySpace.set(spaceId, memberHexes);
			nameBySpace.set(spaceId, stateObj?.fields["name"]?.stringValue ?? "");
			infos.push({ spaceId, keyHex: entry.key, keyId: entry.keyId ?? 1, writers: [...writers], owner: entry.owner });
		}
		this.sync.setSharedSpaces(infos);

		// Owner duty: every member holds the current key - gift-wrap it to
		// anyone that hasn't received this keyId yet (adds and rotations).
		for (const info of infos) {
			if (info.owner && info.owner !== myPk) continue;
			const members = memberHexesBySpace.get(info.spaceId) ?? [];
			if (members.length === 0) continue;
			void this.sync.sendInviteWraps(info.spaceId, info.keyHex, info.keyId, nameBySpace.get(info.spaceId) ?? "", members);
		}

		const owned = infos.filter((i) => !i.owner || i.owner === myPk);
		if (owned.length > 0) {
			await this.sync.publishAllowlist([...new Set(owned.flatMap((i) => i.writers))]);
		}

		// History republish markers.
		let markers: Record<string, number> = {};
		try {
			markers = JSON.parse(localStorage.getItem("roostr-shared-queued") ?? "{}") as Record<string, number>;
		} catch {
			/* fresh */
		}
		for (const info of infos) {
			if (markers[info.spaceId] === info.keyId) continue;
			markers[info.spaceId] = info.keyId;
			let queued = 0;
			for (const [objectId, state] of this.states) {
				const space = state.typeKey === "channel" ? state.id : (state.fields["channel"]?.stringValue ?? "");
				if (space !== info.spaceId) continue;
				const changes = await this.store.changesFor(objectId);
				for (const change of changes) {
					this.sync.publish(encodeChange(change), change.id, objectId);
					queued++;
				}
			}
			if (queued > 0) console.log(`[backend] space ${info.spaceId.slice(0, 8)} shared (key #${info.keyId}): queued ${queued} change(s)`);
		}
		localStorage.setItem("roostr-shared-queued", JSON.stringify(markers));
	}

	/** Log out: stop sync and destroy the local replica. The relays keep
	 *  the encrypted history; a different key must never see this data. */
	async logout(): Promise<void> {
		this.stop();
		this.store.close();
		await destroyDatabase();
		localStorage.removeItem("roostr-shared-queued");
	}

	stop(): void {
		this.sync?.stop();
		this.sync = null;
		this.started = false;
	}

	relays(): string[] {
		try {
			const v = JSON.parse(localStorage.getItem("roostr-relays") ?? "null") as string[] | null;
			// Never-configured devices get the default; a stored list (even
			// empty) is the user's choice - Settings toggles public relays.
			return v === null ? [...DEFAULT_RELAYS] : v;
		} catch {
			return [...DEFAULT_RELAYS];
		}
	}

	setRelays(relays: string[]): void {
		localStorage.setItem("roostr-relays", JSON.stringify(relays));
	}

	onStatus(cb: (s: SyncStatus) => void): () => void {
		this.statusListeners.add(cb);
		return () => this.statusListeners.delete(cb);
	}

	onCommit(cb: (ids: string[]) => void): () => void {
		this.commitListeners.add(cb);
		return () => this.commitListeners.delete(cb);
	}

	/**
	 * Recompute dirty object states. Boot path loads the persisted replay
	 * cache and replays ONLY objects whose change count grew since it was
	 * written - a warm boot does zero replay work.
	 */
	private async ensure(): Promise<void> {
		let rebuilt = false;
		if (this.allDirty) {
			this.allDirty = false;
			rebuilt = true;
			this.states.clear();
			const [counts, cached] = await Promise.all([this.store.changeCounts(), this.store.getStates<ObjectJSON>()]);
			for (const [id, n] of counts) {
				const hit = cached.get(id);
				if (hit && hit.n === n) this.states.set(id, hit.state);
				else this.dirty.add(id);
			}
		}
		const ids = [...this.dirty];
		this.dirty.clear();
		for (const id of ids) {
			try {
				const changes = await this.store.changesFor(id);
				if (changes.length === 0) continue;
				const obj = computeObject(changes);
				if (obj) {
					this.states.set(id, obj);
					void this.store.putState(id, changes.length, obj);
				}
			} catch (err) {
				// One malformed legacy object must never brick the vault.
				console.warn(`[replica] replay failed for ${id}:`, err);
			}
		}
		this.enforceVanished(rebuilt, ids);
	}

	/**
	 * The ledger wins over whatever replayed. A relay copy of a vanished
	 * object can arrive ahead of — or entirely without — the delete change
	 * that tombstones it, since NIP-09 is advisory and a peer may republish
	 * after the deletion was requested. Mirrors enforce_vanished_locked()
	 * in the desktop's store.odin, which is why the two agree on what
	 * exists.
	 */
	private enforceVanished(rebuilt: boolean, touched: string[]): void {
		const ledgerChanged = touched.includes(VANISH_LOG_ID);
		if (rebuilt || ledgerChanged) {
			this.vanished = new Set<string>();
			const ledger = this.states.get(VANISH_LOG_ID);
			if (ledger) {
				for (const k of Object.keys(ledger.fields)) {
					if (k.startsWith(VANISH_FIELD)) this.vanished.add(k.slice(VANISH_FIELD.length));
				}
			}
		}
		if (this.vanished.size === 0) return;
		// Ledger (re)loaded: sweep everything it names — O(vanished), boot
		// only. Otherwise just the objects that were replayed can have come
		// back, so the steady-state cost is the size of that batch.
		if (rebuilt || ledgerChanged) for (const id of this.vanished) this.states.delete(id);
		else for (const id of touched) if (this.vanished.has(id)) this.states.delete(id);
	}

	/**
	 * State fingerprint matching the desktop's GET /api/sync/digest:
	 * sha256 over "<objectId>:<sorted change hex ids>\n" lines, objects sorted,
	 * vanished objects excluded (read from the synced vanish ledger object).
	 */
	async syncDigest(): Promise<{ digest: string; objects: number; changes: number }> {
		await this.ensure();
		// ensure() keeps this.vanished in step with the ledger.
		const ids = (await this.store.objectIds()).filter((id) => !this.vanished.has(id)).sort();
		let text = "";
		let changes = 0;
		let objects = 0;
		for (const oid of ids) {
			const cids = (await this.store.changesFor(oid)).map((c) => c.id).sort();
			if (cids.length === 0) continue;
			text += `${oid}:${cids.join(",")}\n`;
			changes += cids.length;
			objects++;
		}
		return { digest: toHex(sha256(new TextEncoder().encode(text))), objects, changes };
	}

	async fetchObject(id: string): Promise<ObjectJSON> {
		await this.ensure();
		const obj = this.states.get(id);
		if (!obj) throw new Error(`unknown object ${id}`);
		return obj;
	}

	/** The Odin server's GET /api/objects summary exclusions, verbatim. */
	private static readonly HIDDEN_LIST_TYPES = new Set(["program", "typescript", "json", "proto", "relation", "channel", "skill", "peer", "pinned_fact", "milestone", "agent", "vanish_log"]);

	async fetchObjects(): Promise<ObjectSummary[]> {
		await this.ensure();
		const out: ObjectSummary[] = [];
		for (const o of this.states.values()) {
			if (o.deleted || WebBackend.HIDDEN_LIST_TYPES.has(o.typeKey)) continue;
			out.push({
				id: o.id,
				typeKey: o.typeKey,
				name: fstr(o.fields, "name"),
				updatedAt: o.updatedAt,
				channelId: fstr(o.fields, "channel"),
				icon: fstr(o.fields, "iconEmoji"),
			});
		}
		out.sort((a, b) => b.updatedAt - a.updatedAt);
		return out;
	}

	async fetchChannels(): Promise<SpaceJSON[]> {
		await this.ensure();
		const out: SpaceJSON[] = [];
		for (const o of this.states.values()) {
			if (o.deleted || o.typeKey !== "channel") continue;
			const members = (o.fields["members"]?.valuesValue?.items ?? [])
				.map((i) => {
					const e = i.mapValue?.entries;
					return e ? { npub: e["npub"]?.stringValue ?? "", role: e["role"]?.stringValue ?? "" } : null;
				})
				.filter((m): m is { npub: string; role: string } => !!m);
			out.push({
				id: o.id,
				name: fstr(o.fields, "name"),
				icon: fstr(o.fields, "iconEmoji"),
				pinnedIds: (o.fields["pinnedIds"]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean),
				members,
				keyId: o.fields["keyId"]?.intValue ?? 1,
				createdAt: o.createdAt,
				// Display order for the rail, set by drag-reorder. Absent means
				// "use createdAt", so both live in one number space and an
				// unordered vault needs no migration. Deliberately does NOT
				// affect the sort below: this list's order is the protocol's
				// (oldest first) and the UI layers the user's on top.
				order: o.fields["order"]?.floatValue ?? o.fields["order"]?.intValue,
			});
		}
		// Creation order, oldest first: the first channel is the stable
		// default space owning unassigned legacy objects (name order let
		// any new early-alphabet space steal them).
		const created = new Map(out.map((sp) => [sp.id, this.states.get(sp.id)?.createdAt ?? 0]));
		out.sort((a, b) => (created.get(a.id)! - created.get(b.id)!) || a.id.localeCompare(b.id));
		return out;
	}

	async fetchRelations(): Promise<RelationDefJSON[]> {
		await this.ensure();
		const out: RelationDefJSON[] = [];
		for (const o of this.states.values()) {
			if (o.deleted || o.typeKey !== "relation") continue;
			const options = (o.fields["options"]?.valuesValue?.items ?? [])
				.map((i) => {
					const e = i.mapValue?.entries;
					return e
						? {
								id: e["id"]?.stringValue ?? "",
								text: e["text"]?.stringValue ?? "",
								color: e["color"]?.stringValue ?? "",
								orderId: e["orderId"]?.stringValue ?? "",
							}
						: null;
				})
				.filter((x): x is NonNullable<typeof x> => !!x);
			out.push({
				id: o.id,
				key: fstr(o.fields, "key"),
				format: fstr(o.fields, "format") || "shorttext",
				name: fstr(o.fields, "name"),
				iconEmoji: fstr(o.fields, "iconEmoji"),
				space: fstr(o.fields, "channel"),
				hidden: o.fields["hidden"]?.boolValue === true,
				readOnly: o.fields["readOnly"]?.boolValue === true,
				maxCount: o.fields["maxCount"]?.intValue ?? 0,
				options,
			});
		}
		return out.filter((r) => r.key);
	}

	async fetchQuery(body: QueryBody): Promise<{ total: number; records: never[] }> {
		await this.ensure();
		return runQuery(this.states.values(), body) as { total: number; records: never[] };
	}

	async mutate(action: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
		const out = await runMutation(
			{
				author: this.author,
				changesFor: (id) => this.store.changesFor(id),
				getObject: async (id) => {
					await this.ensure();
					return this.states.get(id) ?? null;
				},
				instancesOf: async (typeKey, channel) => {
					await this.ensure();
					const out: string[] = [];
					for (const [id, state] of this.states) {
						if (state.deleted || state.typeKey !== typeKey) continue;
						if ((state.fields["channel"]?.stringValue ?? "") !== channel) continue;
						out.push(id);
					}
					return out;
				},
				objectsWithField: async (key, channel) => {
					await this.ensure();
					const out: string[] = [];
					for (const [id, state] of this.states) {
						if (state.deleted || !(key in state.fields)) continue;
						if ((state.fields["channel"]?.stringValue ?? "") !== channel) continue;
						out.push(id);
					}
					return out;
				},
				commit: async (change: ChangeJSON) => {
					change.id = changeId(change);
					const bytes = encodeChange(change);
					// Round-trip through decode so the stored JSON matches
					// relay-imported changes byte-for-byte.
					const decoded = decodeChange(bytes) ?? change;
					await this.store.addChanges([{ bytes, change: decoded }]);
					this.dirty.add(change.objectId);
					this.sync?.publish(bytes, change.id, change.objectId);
					const cb = [...this.commitListeners];
					for (const fn of cb) fn([change.objectId]);
					return change.id;
				},
			},
			action,
			params,
		);
		return { ok: true, ...out };
	}
}

export const backend = new WebBackend();
