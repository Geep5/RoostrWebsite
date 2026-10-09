/**
 * Objects this device opened, most recent first. Device-local on purpose:
 * "what I was just looking at" is a per-screen habit, not vault data, so it
 * never syncs or writes a change.
 */

const KEY = "roostr-recent-opened";
const LIMIT = 200;

function read(): string[] {
	try {
		const v = JSON.parse(localStorage.getItem(KEY) ?? "[]");
		return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
	} catch {
		return [];
	}
}

/** Record that `id` was just opened. */
export function markOpened(id: string): void {
	if (typeof localStorage === "undefined") return;
	const ids = [id, ...read().filter((x) => x !== id)].slice(0, LIMIT);
	try {
		localStorage.setItem(KEY, JSON.stringify(ids));
	} catch {
		/* storage full or blocked: recency is a nicety */
	}
}

/** id → position (0 = most recently opened); unopened ids are absent. */
export function openedRanks(): Map<string, number> {
	if (typeof localStorage === "undefined") return new Map();
	return new Map(read().map((id, i) => [id, i]));
}
