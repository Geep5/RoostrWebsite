import type { SyncStatus } from "./engine/backend";

const count = (n: number) => n.toLocaleString("en-US");

/**
 * The history pass in flight, if any: what it has received and, when its
 * reconciles made the total knowable, the expected count and a percentage
 * (capped at 99 until the dot is live - the total is approximate).
 */
export function syncProgress(s: SyncStatus): { received: number; expected?: number; pct?: number } | null {
	const syncing = s.phase === "backfill" || (s.phase === "live" && s.received !== undefined);
	if (!syncing) return null;
	const received = s.received ?? 0;
	if (!s.expected || s.expected <= 0) return { received };
	const pct = Math.min(Math.floor((received / s.expected) * 100), s.phase === "live" ? 100 : 99);
	return { received, expected: s.expected, pct };
}

/** "Syncing · 1,234 of 5,000 (24%)" or "Syncing · 1,234 received". */
export function progressText(p: { received: number; expected?: number; pct?: number }): string {
	return p.expected ? `Syncing · ${count(p.received)} of ${count(p.expected)} (${p.pct}%)` : `Syncing · ${count(p.received)} received`;
}

/**
 * The sync dot's words. Unsent changes come with what sync is saying
 * right now - an error, "relay unresponsive - reconnecting", a relay's
 * failure - so a stuck queue explains itself instead of only counting.
 */
export function syncTip(s: SyncStatus): string {
	const pending = s.pending ? `${count(s.pending)} change(s) not yet published` : "";
	const progress = syncProgress(s);
	const state =
		s.phase === "error" ? `Error: ${s.detail ?? "sync stopped"}`
		: s.phase === "live" ? [s.detail ?? (pending ? "" : `Synced · ${count(s.imported)} changes`), progress ? progressText(progress) : ""].filter(Boolean).join(" · ")
		: s.phase === "backfill" ? (s.detail && s.detail !== "catching up" ? s.detail : progressText(progress!))
		: "Not syncing";
	return [pending, state].filter(Boolean).join(" · ");
}
