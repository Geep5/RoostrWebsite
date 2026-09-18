/**
 * Which computer serves a space.
 *
 * Its own module, with a type-only import, so it can be tested without
 * dragging in the reactive store: `$lib/serving` reaches the store through
 * `$lib/api`, and runes do not exist outside the Svelte compiler.
 */

import type { MachineRow } from "$lib/serving";

export type SpaceComputer =
	| { state: "served"; machine: MachineRow }
	| { state: "choose" }
	| { state: "install" };

/**
 * A space with no computer is inert - agents are minted and answered by a
 * machine's harness, recurring work fires there, and the logins live there -
 * so the front page has to tell three cases apart:
 *
 *   served   a machine is chosen and present; say nothing
 *   choose   machines exist, none is chosen for this space
 *   install  there is no machine at all
 *
 * A `served_by` naming a machine this replica has never seen counts as a
 * choice still to make: something has to answer here.
 */
export function spaceComputer(machines: MachineRow[], servedBy: string): SpaceComputer {
	const machine = machines.find((m) => m.machineId === servedBy);
	if (machine) return { state: "served", machine };
	return { state: machines.length > 0 ? "choose" : "install" };
}
