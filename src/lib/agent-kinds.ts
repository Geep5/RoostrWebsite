/**
 * An agent is an object configured through its properties. This file is
 * only the roster/machine helpers creation still needs: which computer this
 * tab is paired with, and claiming a new agent on it so it answers once it
 * has a Computer set.
 */

import { isLocalBackend } from "$lib/client-backend";
import { harnessFetch, pairedSession } from "$lib/local-transport";
import type { ValueJSON } from "$lib/types";

/** machine_id of the harness this tab is paired with; "" when hosted, unpaired, or unreachable. */
export async function localMachineId(): Promise<string> {
	if (!isLocalBackend || !pairedSession()) return "";
	try {
		const res = await harnessFetch("/machine");
		return res.ok ? ((await res.json()) as { id: string }).id : "";
	} catch {
		return "";
	}
}

/**
 * Claim an agent on the paired harness's roster now, the way SpaceAgents
 * does, so it answers before the next converge. A failure here is not an error.
 */
export async function adoptLocally(id: string): Promise<boolean> {
	try {
		const res = await harnessFetch("/agents/toggle", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id, enabled: true }) });
		return res.ok;
	} catch {
		return false;
	}
}

export const sv = (s: string): ValueJSON => ({ stringValue: s });
