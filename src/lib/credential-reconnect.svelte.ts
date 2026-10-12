/**
 * Reconnects started from views, per credential: a table cell, a card and
 * the popover over them all read the same progress, and it outlives the
 * popover (closing it leaves the sign-in running and the cell following it).
 */

import { SvelteMap } from "svelte/reactivity";
import { startReconnect, type ReconnectState } from "$lib/credential-actions";

const states = new SvelteMap<string, ReconnectState>();
const stops = new Map<string, () => void>();

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
