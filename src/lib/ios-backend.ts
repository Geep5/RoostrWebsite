import type { ObjectJSON, ObjectSummary, SpaceJSON, RelationDefJSON } from "./types";
import type { QueryBody } from "./engine/contracts";
import type { SyncStatus } from "./engine/backend";
import type { JoinRequest } from "./engine/sync";
import type { NativeSettings } from "./local-backend";
import { initCore } from "./engine/core";

/**
 * The Swift host owns the key, store, relays and engine; this layer keeps no
 * data. Every method is one message over the WKWebView bridge:
 *   JS → Swift  window.webkit.messageHandlers.roostr.postMessage({ id, method, args })
 *   Swift → JS  window.__roostrReply(id, ok, payload)   window.__roostrEvent(name, payload)
 */

interface BridgeWindow {
	webkit?: { messageHandlers?: { roostr?: { postMessage(message: { id: number; method: string; args: unknown[] }): void } } };
	__roostrReply?: (id: number, ok: boolean, payload: unknown) => void;
	__roostrEvent?: (name: string, payload: unknown) => void;
}

interface Pending { resolve: (value: unknown) => void; reject: (error: Error) => void }

const pending = new Map<number, Pending>();
const eventListeners: Record<string, Set<(payload: unknown) => void>> = { status: new Set(), commit: new Set() };
let nextId = 1;
// WKWebView installs `webkit.messageHandlers` on the page window; the DOM lib does not know it.
const host: BridgeWindow | undefined = typeof window === "undefined" ? undefined : (window as unknown as BridgeWindow);

function call<T>(method: string, ...args: unknown[]): Promise<T> {
	const handler = host?.webkit?.messageHandlers?.roostr;
	if (!handler) return Promise.reject(new Error("The Roostr iOS bridge is not available in this page."));
	const id = nextId++;
	return new Promise<T>((resolve, reject) => {
		pending.set(id, { resolve: (value) => resolve(value as T), reject });
		try {
			// Callers hand over Svelte `$state` proxies (join links, relay lists); WebKit's structured clone
			// rejects proxies, and the host reads JSON anyway.
			handler.postMessage({ id, method, args: JSON.parse(JSON.stringify(args)) as unknown[] });
		} catch (error) {
			pending.delete(id);
			reject(error instanceof Error ? error : new Error(String(error)));
		}
	});
}

if (host) {
	host.__roostrReply = (id, ok, payload) => {
		const entry = pending.get(id);
		if (!entry) return;
		pending.delete(id);
		if (ok) { entry.resolve(payload); return; }
		const message = payload && typeof payload === "object" && "message" in payload && typeof payload.message === "string" ? payload.message : `bridge call ${id} failed`;
		entry.reject(new Error(message));
	};
	host.__roostrEvent = (name, payload) => {
		const set = eventListeners[name];
		if (!set) return;
		for (const fn of set) fn(payload);
	};
}

function isStatus(value: unknown): value is SyncStatus {
	return !!value && typeof value === "object" && "phase" in value && typeof value.phase === "string" && "imported" in value && typeof value.imported === "number" && "bootstrapped" in value && typeof value.bootstrapped === "boolean";
}

function isIdentity(value: unknown): value is { pubkey: string; npub: string } {
	return !!value && typeof value === "object" && "pubkey" in value && typeof value.pubkey === "string" && "npub" in value && typeof value.npub === "string";
}

function isJoinRequest(value: unknown): value is JoinRequest {
	return !!value && typeof value === "object"
		&& "key" in value && typeof value.key === "string"
		&& "space" in value && typeof value.space === "string"
		&& "spaceName" in value && typeof value.spaceName === "string"
		&& "requester" in value && typeof value.requester === "string"
		&& "requesterNpub" in value && typeof value.requesterNpub === "string"
		&& "at" in value && typeof value.at === "number";
}

export class IOSBackend {
	author = "";
	npub = "";
	status: SyncStatus = { phase: "idle", imported: 0, bootstrapped: false };
	private relayList: string[] = [];
	private started = false;
	private commits = new Set<(ids: string[]) => void>();
	private statuses = new Set<(status: SyncStatus) => void>();
	private readonly onStatusEvent = (payload: unknown): void => { if (isStatus(payload)) this.update(payload); };
	private readonly onCommitEvent = (payload: unknown): void => {
		if (!Array.isArray(payload) || !payload.every((id) => typeof id === "string")) return;
		for (const fn of this.commits) fn(payload);
	};
	private update(status: SyncStatus): void { this.status = status; for (const fn of this.statuses) fn(status); }

	async start(): Promise<void> {
		if (this.started) return;
		// Data comes over the bridge; the WASM core still serves UI-side helpers
		// (markdown, previews) that run against fetched objects.
		await initCore();
		eventListeners.status.add(this.onStatusEvent);
		eventListeners.commit.add(this.onCommitEvent);
		const status = await call<unknown>("start");
		this.started = true;
		await this.identity();
		this.relayList = await call<string[]>("relays");
		if (isStatus(status)) this.update(status);
	}
	stop(): void {
		if (!this.started) return;
		this.started = false;
		eventListeners.status.delete(this.onStatusEvent);
		eventListeners.commit.delete(this.onCommitEvent);
		void call<void>("stop").catch(() => {});
	}
	async logout(exportPending?: boolean): Promise<void> {
		await call<void>("logout", exportPending ?? false);
		this.stop();
		this.author = "";
		this.npub = "";
	}
	relays(): string[] { return [...this.relayList]; }
	async setRelays(relays: string[]): Promise<void> { await call<void>("setRelays", relays); this.relayList = [...relays]; }
	onStatus(fn: (status: SyncStatus) => void): () => void { this.statuses.add(fn); return () => this.statuses.delete(fn); }
	onCommit(fn: (ids: string[]) => void): () => void { this.commits.add(fn); return () => this.commits.delete(fn); }
	async identity(): Promise<{ pubkey: string; npub: string }> {
		const id = await call<unknown>("identity");
		if (!isIdentity(id)) throw new Error("Invalid identity from the iOS host");
		this.author = id.pubkey;
		this.npub = id.npub;
		return id;
	}
	async fetchSettings(): Promise<NativeSettings> {
		const id = await this.identity();
		this.relayList = await call<string[]>("relays");
		return { hasKey: true, relays: this.relays(), authorId: id.pubkey, npub: id.npub };
	}
	exportKey(): Promise<{ nsec: string; hex: string }> { return call("exportKey"); }
	importKey(key: string): Promise<void> { return call("importKey", key); }
	fetchStatus(): Promise<SyncStatus> { return call("status"); }
	fetchObject(id: string): Promise<ObjectJSON> { return call("fetchObject", id); }
	fetchObjects(): Promise<ObjectSummary[]> { return call("fetchObjects"); }
	fetchChannels(): Promise<SpaceJSON[]> { return call("fetchChannels"); }
	fetchRelations(): Promise<RelationDefJSON[]> { return call("fetchRelations"); }
	fetchQuery(body: QueryBody): Promise<{ total: number; records: never[] }> { return call("fetchQuery", body); }
	syncDigest(): Promise<{ digest: string; objects: number; changes: number }> { return call("syncDigest"); }
	mutate(action: string, params: Record<string, unknown>): Promise<Record<string, unknown>> { return call("mutate", action, params); }
	async joinRequests(): Promise<JoinRequest[]> {
		const rows = await call<unknown>("joinRequests");
		if (!Array.isArray(rows) || !rows.every(isJoinRequest)) throw new Error("Invalid join requests from the iOS host");
		return rows;
	}
	clearJoinRequest(key: string): Promise<void> { return call("clearJoinRequest", key); }
	sendJoinRequest(link: { space: string; owner: string; relays: string[] }): Promise<void> {
		return call("sendJoinRequest", { space: link.space, owner: link.owner, relays: link.relays });
	}
	async importSpaceInvite(inv: { space: string; owner: string; key: string; keyId: number }): Promise<boolean> {
		return (await call<unknown>("importSpaceInvite", inv)) === true;
	}
}
