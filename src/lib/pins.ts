/**
 * Pins. Each pinned object carries its own position in its space's sidebar
 * (`pin_order`, a sortable text key): pinning, unpinning or moving one object
 * writes only that object, so edits from different devices to different pins
 * all survive. A space's pin list (SpaceJSON.pinnedIds) is its live objects
 * with a position, in key order, id breaking ties - built by whoever lists
 * spaces (this replica, the daemon, the iOS app) the same way as core/pins.odin.
 */
import type { ValueJSON } from "$lib/types";

export const PIN_KEY = "pin_order";

const DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/**
 * A key strictly between `a` and `b` ("" for a = before everything; null b =
 * after everything). Keys never end in "0", so there is always room between
 * two. core/pins.odin pin_key_between is the same algorithm.
 */
export function keyBetween(a: string, b: string | null): string {
	if (b !== null) {
		let n = 0;
		while (n < b.length && (a[n] ?? "0") === b[n]) n++;
		if (n > 0) return b.slice(0, n) + keyBetween(a.slice(n), b.slice(n));
	}
	const lo = a ? DIGITS.indexOf(a[0]) : 0;
	const hi = b !== null && b ? DIGITS.indexOf(b[0]) : DIGITS.length;
	if (hi - lo > 1) return DIGITS[Math.floor((lo + hi + 1) / 2)];
	if (b !== null && b.length > 1) return b.slice(0, 1);
	return DIGITS[lo] + keyBetween(a.slice(1), null);
}

interface PinnableState {
	id: string;
	typeKey: string;
	deleted?: boolean;
	createdAt: number;
	fields: Record<string, ValueJSON>;
}

/** Pinned object ids per space, in sidebar order. An object with no space belongs to the oldest space. */
export function pinsBySpace(states: Iterable<PinnableState>): Map<string, string[]> {
	const all = [...states].filter((s) => !s.deleted);
	const spaces = all.filter((s) => s.typeKey === "channel").sort((a, b) => a.createdAt - b.createdAt || (a.id < b.id ? -1 : 1));
	const fallback = spaces[0]?.id ?? "";
	const bySpace = new Map<string, Array<{ id: string; key: string }>>();
	for (const s of all) {
		const key = s.fields[PIN_KEY]?.stringValue ?? "";
		if (!key) continue;
		const space = s.fields["channel"]?.stringValue || fallback;
		bySpace.set(space, [...(bySpace.get(space) ?? []), { id: s.id, key }]);
	}
	const out = new Map<string, string[]>();
	for (const [space, list] of bySpace) out.set(space, list.sort((x, y) => (x.key !== y.key ? (x.key < y.key ? -1 : 1) : x.id < y.id ? -1 : 1)).map((e) => e.id));
	return out;
}
