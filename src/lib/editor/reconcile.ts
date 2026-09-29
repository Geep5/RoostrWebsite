/**
 * Fold a fresh copy of an object into the one on screen, in place.
 *
 * Replacing the whole object on every sync event swapped every block
 * reference at once: each block re-rendered, the focused one lost its
 * caret, and a keystroke landing mid-swap read a map that was about to
 * change. Here only what differs is written: a block whose content is
 * unchanged keeps its very object (so nothing re-renders for it), changed
 * blocks are replaced one by one, and fields/metadata are patched key by
 * key. Works on Svelte `$state` proxies: reassigning an existing proxy keeps it.
 */
import type { BlockJSON, ObjectJSON } from "$lib/types";

const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);

export function reconcileObject(target: ObjectJSON, fresh: ObjectJSON): void {
	// Top-level metadata (typeKey, deleted, updatedAt, mailbox, ...), except the two handled below.
	const t = target as unknown as Record<string, unknown>;
	const f = fresh as unknown as Record<string, unknown>;
	for (const key of Object.keys(f)) {
		if (key === "fields" || key === "blocks") continue;
		if (!same(t[key], f[key])) t[key] = f[key];
	}
	for (const key of Object.keys(t)) if (!(key in f) && key !== "fields" && key !== "blocks") delete t[key];

	for (const [key, value] of Object.entries(fresh.fields)) {
		if (!same(target.fields[key], value)) target.fields[key] = value;
	}
	for (const key of Object.keys(target.fields)) if (!(key in fresh.fields)) delete target.fields[key];

	const current = new Map(target.blocks.map((b) => [b.id, b]));
	const next: BlockJSON[] = fresh.blocks.map((b) => {
		const kept = current.get(b.id);
		return kept && same(kept, b) ? kept : b;
	});
	const changed = next.length !== target.blocks.length || next.some((b, i) => b !== target.blocks[i]);
	if (changed) target.blocks = next;
}
