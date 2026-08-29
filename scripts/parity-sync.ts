/**
 * parity-sync.ts — READ-ONLY relay backfill parity check.
 *
 * Loads the real key from ~/.glon/nostr.json, runs the engine's backfill
 * path (fake-indexeddb store) against the default relays, and reports
 * decrypt/decode/dedup/cursor numbers. Publishes NOTHING.
 *
 *   bun run scripts/parity-sync.ts
 */

import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import { parseKey, authorIdFor } from "../src/lib/engine/keys";
import { ChangeStore } from "../src/lib/engine/store";
import { RelaySync, DEFAULT_RELAYS } from "../src/lib/engine/sync";
import type { ChangeJSON } from "../src/lib/engine/contracts";

const QUIET_MS = 5_000;
const MAX_EVENTS = 300;

// ── Identity ─────────────────────────────────────────────────────
const nostrPath = `${process.env.HOME}/.glon/nostr.json`;
const identity = (await Bun.file(nostrPath).json()) as { privkey?: string };
if (!identity.privkey) throw new Error(`no privkey in ${nostrPath}`);
const key = parseKey(identity.privkey);
if (!key) throw new Error("privkey did not parse");
console.log(`identity ${key.pk.slice(0, 8)}… (${key.npub.slice(0, 14)}…) authorId=${authorIdFor(key)}`);

// ── Decode: real proto.ts if loadable, else byte-passthrough stub ─
let decodeLabel = "proto.ts";
const firstBytes: number[] = [];
let realDecode: ((bytes: Uint8Array) => ChangeJSON | null) | null = null;
try {
	// Runtime-conditional on purpose: proto.ts imports
	// "$lib/engine-proto.proto?raw", which bun cannot resolve outside Vite;
	// may also not exist yet (written by a sibling agent).
	const mod = (await import("../src/lib/engine/proto")) as { proto: { decodeChange(b: Uint8Array): ChangeJSON | null } };
	realDecode = (b) => mod.proto.decodeChange(b);
} catch {
	decodeLabel = "STUB (byte-passthrough; proto.ts not bun-loadable — verifying decrypt+dedup+cursor only)";
}
const decode = (bytes: Uint8Array): ChangeJSON | null => {
	if (bytes.length > 0) firstBytes.push(bytes[0]);
	if (realDecode) return realDecode(bytes);
	const hex = bytesToHex(sha256(bytes));
	return { id: hex, objectId: `stub:${hex.slice(0, 16)}`, parentIds: [], ops: [], timestamp: 0, author: "" };
};
console.log(`decode: ${decodeLabel}`);

// ── Store + sync (fresh fake-indexeddb each run) ─────────────────
const store = new ChangeStore();
await store.open();
const cursorBefore = await store.getCursor();

let objectNotifyBatches = 0;
let objectNotifyIds = 0;
const sync = new RelaySync(
	key.sk,
	DEFAULT_RELAYS,
	store,
	{
		onObjects: (ids) => {
			objectNotifyBatches++;
			objectNotifyIds += ids.length;
		},
		onStatus: (s) => {
			console.log(`[status] ${s.phase}${s.detail ? `: ${s.detail}` : ""}${s.imported !== undefined ? ` (imported=${s.imported})` : ""}`);
		},
	},
	{ decode },
);

const t0 = Date.now();
await sync.start(); // backfill then live — publish() is never called
const backfillMs = Date.now() - t0;

// Stay live until quiet for QUIET_MS or MAX_EVENTS total.
let lastCount = sync.stats.events;
let quietSince = Date.now();
while (Date.now() - quietSince < QUIET_MS && sync.stats.events < MAX_EVENTS) {
	await Bun.sleep(250);
	if (sync.stats.events !== lastCount) {
		lastCount = sync.stats.events;
		quietSince = Date.now();
	}
}
sync.stop();

// ── Dedup re-check: re-adding an already-stored change must add 0 ─
const objectIds = await store.objectIds();
let dedup = "n/a (no objects)";
if (objectIds.length > 0) {
	const existing = await store.changesFor(objectIds[0]);
	const again = await store.addChanges(existing.map((c) => ({ bytes: new Uint8Array([0x0a]), change: c })));
	dedup = again === 0 ? "ok (re-add of stored change ids → 0 new)" : `FAIL (re-add returned ${again})`;
}

const cursorAfter = await store.getCursor();
const protoLooking = firstBytes.filter((b) => (b & 7) <= 5 && b >> 3 >= 1 && b >> 3 <= 15).length;
const leadingA = firstBytes.filter((b) => b === 0x0a).length;

console.log("\n── parity-sync report ─────────────────────────────");
console.log(`relays:                    ${DEFAULT_RELAYS.join(", ")}`);
console.log(`backfill wall time:        ${(backfillMs / 1000).toFixed(1)}s`);
console.log(`events received:           ${sync.stats.events}`);
console.log(`decrypt failures:          ${sync.stats.decryptFailures}`);
console.log(`decode failures:           ${sync.stats.decodeFailures}`);
console.log(`changes imported (new):    ${sync.stats.imported}`);
console.log(`distinct blinded 'h' tags: ${sync.stats.blindedTags.size}`);
console.log(`distinct object ids:       ${objectIds.length}`);
console.log(`onObjects batches/ids:     ${objectNotifyBatches}/${objectNotifyIds}`);
console.log(`cursor:                    ${cursorBefore} → ${cursorAfter} (${cursorAfter > cursorBefore ? "advanced" : "NOT advanced"})`);
console.log(`protobuf-looking payloads: ${protoLooking}/${firstBytes.length} (first byte 0x0a: ${leadingA})`);
console.log(`dedup:                     ${dedup}`);

let failed = false;
const fail = (msg: string) => {
	failed = true;
	console.error(`FAIL: ${msg}`);
};
if (sync.stats.decryptFailures !== 0) fail("decrypt failures != 0");
if (sync.stats.decodeFailures !== 0) fail("decode failures != 0");
if (!(cursorAfter > cursorBefore)) fail("cursor did not advance");
if (protoLooking < 100) fail(`only ${protoLooking} protobuf-looking payloads (<100)`);
if (dedup.startsWith("FAIL")) fail("dedup");
console.log(failed ? "\nRESULT: FAIL" : "\nRESULT: PASS");
process.exit(failed ? 1 : 0);
