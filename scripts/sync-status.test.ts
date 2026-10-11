import { describe, expect, test } from "bun:test";
import { syncProgress, syncTip } from "../src/lib/sync-status";

describe("sync tip", () => {
	test("a known total shows received of expected with a percentage capped at 99 until live", () => {
		expect(syncTip({ phase: "backfill", imported: 0, received: 1234, expected: 5000, bootstrapped: false })).toBe("Syncing · 1,234 of 5,000 (24%)");
		expect(syncTip({ phase: "backfill", imported: 0, received: 5000, expected: 5000, detail: "catching up", bootstrapped: true })).toBe("Syncing · 5,000 of 5,000 (99%)");
		expect(syncProgress({ phase: "live", imported: 0, received: 5000, expected: 5000, bootstrapped: false })?.pct).toBe(100);
	});

	test("an unknown total counts what arrived", () => {
		expect(syncTip({ phase: "backfill", imported: 0, received: 12345, bootstrapped: false })).toBe("Syncing · 12,345 received");
		expect(syncTip({ phase: "backfill", imported: 0, bootstrapped: false })).toBe("Syncing · 0 received");
	});

	test("errors, reconnects and pending keep their wording", () => {
		expect(syncTip({ phase: "error", imported: 0, detail: "boom", bootstrapped: true })).toBe("Error: boom");
		expect(syncTip({ phase: "backfill", imported: 0, received: 3, expected: 9, detail: "relay unresponsive - reconnecting", bootstrapped: true })).toBe("relay unresponsive - reconnecting");
		expect(syncTip({ phase: "backfill", imported: 0, received: 3, pending: 1200, bootstrapped: true })).toBe("1,200 change(s) not yet published · Syncing · 3 received");
	});

	test("live: background verification shows its progress, a finished pass shows none", () => {
		expect(syncTip({ phase: "live", imported: 9, detail: "loading spaces · 9 changes so far", received: 9, expected: 40, bootstrapped: false })).toBe("loading spaces · 9 changes so far · Syncing · 9 of 40 (22%)");
		expect(syncTip({ phase: "live", imported: 2500, bootstrapped: true })).toBe("Synced · 2,500 changes");
		expect(syncProgress({ phase: "live", imported: 2500, bootstrapped: true })).toBeNull();
	});
});
