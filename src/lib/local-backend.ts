import type { ObjectJSON, ObjectSummary, SpaceJSON, RelationDefJSON } from "./types";
import type { QueryBody } from "./engine/contracts";
import type { SyncStatus } from "./engine/backend";
import { localJSON, streamLocalEvents, pairedSession, unpairLocal, onPairingChange } from "./local-transport";

export interface NativeSettings { hasKey: boolean; relays: string[]; authorId: string; npub?: string }
const post = (body: unknown): RequestInit => ({ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

/** The daemon owns persistence, signing, replay and publication in local mode. */
export class LocalBackend {
	author = "";
	npub = "";
	status: SyncStatus = { phase: "idle", imported: 0, bootstrapped: false };
	private relayList: string[] = [];
	private stream?: AbortController;
	private offPairing?: () => void;
	private commits = new Set<(ids: string[]) => void>();
	private statuses = new Set<(status: SyncStatus) => void>();
	private update(status: SyncStatus): void { this.status = status; for (const fn of this.statuses) fn(status); }
	async start(): Promise<void> {
		if (this.stream) return;
		const settings = await this.fetchSettings();
		if (!settings.hasKey) throw new Error("Set up the daemon identity in its terminal before opening the app.");
		this.author = settings.authorId;
		this.npub = settings.npub ?? "";
		this.relayList = settings.relays;
		this.stream = new AbortController();
		this.offPairing = onPairingChange(() => { if (!pairedSession()) { this.stop(); this.author = ""; this.update({ phase: "error", imported: 0, bootstrapped: false, detail: "Pairing expired or was removed. Reconnect with a terminal code." }); } });
		void this.events(this.stream.signal);
	}
	private async events(signal: AbortSignal): Promise<void> {
		while (!signal.aborted) {
			try {
				await streamLocalEvents("/api/events", signal, (_event, data) => {
					const event = JSON.parse(data) as { hello?: boolean; objectId?: string };
					if (event.hello) {
						// Daemon connectivity does not prove relay history has finished backfilling.
						this.update({ phase: "live", imported: this.status.imported, bootstrapped: false });
						for (const fn of this.commits) fn([]);
					}
					if (event.objectId) for (const fn of this.commits) fn([event.objectId]);
				});
			} catch (error) {
				if (signal.aborted) return;
				this.update({ phase: "error", imported: this.status.imported, bootstrapped: false, detail: error instanceof Error ? error.message : String(error) });
				if (!pairedSession()) return;
				await new Promise<void>((resolve) => {
					const done = () => { clearTimeout(timer); signal.removeEventListener("abort", done); resolve(); };
					const timer = setTimeout(done, 2000);
					signal.addEventListener("abort", done, { once: true });
				});
			}
		}
	}
	stop(): void { this.stream?.abort(); this.stream = undefined; this.offPairing?.(); this.offPairing = undefined; }
	async logout(): Promise<void> { this.stop(); this.author = ""; unpairLocal(); }
	relays(): string[] { return [...this.relayList]; }
	async setRelays(relays: string[]): Promise<void> { await this.mutate("nostr_relays_set", { relays }); this.relayList = [...relays]; }
	onStatus(fn: (status: SyncStatus) => void): () => void { this.statuses.add(fn); return () => this.statuses.delete(fn); }
	onCommit(fn: (ids: string[]) => void): () => void { this.commits.add(fn); return () => this.commits.delete(fn); }
	fetchSettings(): Promise<NativeSettings> { return localJSON("/api/settings"); }
	fetchObject(id: string): Promise<ObjectJSON> { return localJSON(`/api/objects/${encodeURIComponent(id)}`); }
	fetchObjects(): Promise<ObjectSummary[]> { return localJSON("/api/objects"); }
	fetchChannels(): Promise<SpaceJSON[]> { return localJSON("/api/channels"); }
	fetchRelations(): Promise<RelationDefJSON[]> { return localJSON("/api/relations"); }
	fetchQuery(body: QueryBody): Promise<{ total: number; records: never[] }> { return localJSON("/api/query", post(body)); }
	syncDigest(): Promise<{ digest: string; objects: number; changes: number }> { return localJSON("/api/sync/digest"); }
	async mutate(action: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
		const result = await localJSON<Record<string, unknown>>("/api/mutate", post({ ...params, action }));
		if (!result.ok) throw new Error(String(result.error ?? `Mutation ${action} failed`));
		return result;
	}
}
