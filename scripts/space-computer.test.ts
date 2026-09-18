/**
 * A space with no computer is inert: agents are minted and answered by a
 * machine's harness, recurring work fires there, and the logins live there.
 * The front page has to tell the three cases apart, or a fresh space just
 * never answers and reads as broken.
 */
import { expect, test } from "bun:test";
import { spaceComputer } from "../src/lib/space-computer";
import type { MachineRow } from "../src/lib/serving";

const mac: MachineRow = { id: "m1", machineId: "mac-id", name: "Mac", capabilities: ["browserless"] };
const dev: MachineRow = { id: "m2", machineId: "dev-id", name: "geepOmenComp", capabilities: [] };

test("no machines at all asks for an install", () => {
	expect(spaceComputer([], "")).toEqual({ state: "install" });
	// A stale served_by pointing at a machine that is gone is still "install":
	// there is nothing here to serve the space.
	expect(spaceComputer([], "vanished-id")).toEqual({ state: "install" });
});

test("machines present but none chosen asks which one", () => {
	expect(spaceComputer([mac, dev], "")).toEqual({ state: "choose" });
	// Pointing at a machine this replica has never seen is also a choice to make.
	expect(spaceComputer([mac, dev], "someone-elses-id")).toEqual({ state: "choose" });
});

test("a machine that serves the space silences the card", () => {
	expect(spaceComputer([mac, dev], "mac-id")).toEqual({ state: "served", machine: mac });
	expect(spaceComputer([mac, dev], "dev-id")).toEqual({ state: "served", machine: dev });
});

test("a machine with no capabilities can still serve", () => {
	// Capabilities gate which work moves where; serving at all does not
	// require any - a bare machine still answers discussions.
	expect(spaceComputer([dev], "dev-id").state).toBe("served");
});
