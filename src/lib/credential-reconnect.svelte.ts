/**
 * Reconnects and Checks started from this tab, per credential: a table
 * cell, a card, the popover over them and the Properties pane's Status row
 * all read the same progress, and it outlives the popover (closing it leaves
 * the sign-in running or the Check waiting, and the cell following it).
 */

import { SvelteMap } from "svelte/reactivity";
import { pendingCheck, startCheck, startReconnect, type CheckState, type ReconnectState } from "$lib/credential-actions";
import type { ValueJSON } from "$lib/types";

const states = new SvelteMap<string, ReconnectState>();
const stops = new Map<string, () => void>();
const checks = new SvelteMap<string, CheckState>();
const checkStops = new Map<string, () => void>();

/** The latest reconnect step for this credential, if one ran in this tab. */
export function reconnectState(id: string): ReconnectState | undefined {
	return states.get(id);
}

/**
 * Connect this credential (or, `alreadyConnecting`, follow a sign-in started
 * elsewhere), unless this tab is already on it. `onchanged` re-reads the view.
 */
export function reconnect(id: string, onchanged: () => Promise<void>, alreadyConnecting = false): void {
	const now = states.get(id)?.phase;
	if (now === "starting" || now === "waiting") return;
	stops.get(id)?.();
	stops.set(
		id,
		startReconnect(
			id,
			(s) => {
				states.set(id, s);
				if (s.phase === "connected" || s.phase === "failed") stops.delete(id);
			},
			onchanged,
			{ alreadyConnecting },
		),
	);
}

/** The latest Check step for this credential, if one ran (or was followed) in this tab. */
export function checkState(id: string): CheckState | undefined {
	return checks.get(id);
}

/** A Check is waiting for its answer: one this tab follows, or one another device asked for. */
export function checking(id: string, fields: Record<string, ValueJSON>): boolean {
	const s = checks.get(id);
	const pending = pendingCheck(fields);
	return s?.phase === "checking" || (pending > 0 && pending !== s?.requestedAt);
}

/**
 * Check this credential (`here`: this tab is paired with the computer
 * serving it; `pending`: follow a request already written instead of
 * asking), unless this tab already waits for an answer. `fields` are the
 * credential's as read now; `onchanged` re-reads the view.
 */
export function check(id: string, fields: Record<string, ValueJSON>, onchanged: () => Promise<void>, opts: { here?: boolean; pending?: number }): void {
	if (checks.get(id)?.phase === "checking") return;
	checkStops.get(id)?.();
	checkStops.set(
		id,
		startCheck(
			id,
			fields,
			(s) => {
				checks.set(id, s);
				if (s.phase !== "checking") checkStops.delete(id);
			},
			onchanged,
			opts,
		),
	);
}

/** Follow a Check another device asked for (its `check_requested` still unanswered), unless this tab already followed that one. */
export function followCheck(id: string, fields: Record<string, ValueJSON>, onchanged: () => Promise<void>): void {
	const pending = pendingCheck(fields);
	if (pending && checks.get(id)?.requestedAt !== pending) check(id, fields, onchanged, { pending });
}
