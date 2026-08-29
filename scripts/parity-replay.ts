/**
 * Replay parity harness: for every object dir in ~/.glon/changes,
 * decode all .pb changes, verify each file's content address
 * (sha256 of encoding with id zeroed == filename), compute the object
 * with the browser engine, and deep-compare against the live Odin
 * server's GET /api/objects/:id.
 *
 * Run: bun run scripts/parity-replay.ts
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import { sha256 } from "@noble/hashes/sha2.js";
import { decodeChange, changeId } from "../src/lib/engine/proto";
import { computeObject } from "../src/lib/engine/replay";

const SERVER = "http://127.0.0.1:7333";
const CHANGES_DIR = join(homedir(), ".glon", "changes");

// ── Canonical form for comparison ───────────────────────────────────
// Sort object keys recursively. No other normalization: the engine is
// expected to produce the server's exact key set and values.

function canon(v: unknown): unknown {
	if (Array.isArray(v)) return v.map(canon);
	if (v !== null && typeof v === "object") {
		const out: Record<string, unknown> = {};
		for (const k of Object.keys(v as Record<string, unknown>).sort()) {
			out[k] = canon((v as Record<string, unknown>)[k]);
		}
		return out;
	}
	return v;
}

function stableJSON(v: unknown): string {
	return JSON.stringify(canon(v));
}

/** First differing path between two canonical values, for diagnostics. */
function firstDiff(a: unknown, b: unknown, path = "$"): string | null {
	if (a === b) return null;
	if (Array.isArray(a) && Array.isArray(b)) {
		if (a.length !== b.length) return `${path}.length ${a.length} != ${b.length}`;
		for (let i = 0; i < a.length; i++) {
			const d = firstDiff(a[i], b[i], `${path}[${i}]`);
			if (d) return d;
		}
		return null;
	}
	if (a !== null && b !== null && typeof a === "object" && typeof b === "object") {
		const ka = Object.keys(a as object).sort();
		const kb = Object.keys(b as object).sort();
		if (ka.join(",") !== kb.join(",")) return `${path} keys [${ka}] != [${kb}]`;
		for (const k of ka) {
			const d = firstDiff(
				(a as Record<string, unknown>)[k],
				(b as Record<string, unknown>)[k],
				`${path}.${k}`,
			);
			if (d) return d;
		}
		return null;
	}
	return `${path}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`;
}

// ── Walk the change store ───────────────────────────────────────────

const entries = readdirSync(CHANGES_DIR);
const objectDirs: string[] = [];
const looseFiles: string[] = [];
for (const e of entries) {
	const full = join(CHANGES_DIR, e);
	if (statSync(full).isDirectory()) objectDirs.push(e);
	else if (e.endsWith(".pb")) looseFiles.push(e);
}
objectDirs.sort();

let decodeFail = 0;
let idFail = 0;
let idOk = 0;
let parityOk = 0;
let parityFail = 0;
let fetchFail = 0;
let deletedCount = 0;
let canonicalIdOk = 0;
let canonicalIdLegacy = 0;
const failures: string[] = [];

/**
 * Content address over RAW WIRE BYTES, exactly as the Odin server's
 * importer verifies it (glonOdin/src/sync.odin): every writer emits the
 * id as the leading field (0x0a 0x20 + 32 bytes); the address is
 * sha256(`0a 00` + rest). Re-encoding through any codec can't reproduce
 * legacy writers' explicit-empty-field bytes — bytes are the truth.
 */
function rawChangeId(data: Uint8Array): string | null {
	if (data.length < 34 || data[0] !== 0x0a || data[1] !== 0x20) return null;
	const preimage = new Uint8Array(data.length - 32);
	preimage[0] = 0x0a;
	preimage[1] = 0x00;
	preimage.set(data.subarray(34), 2);
	return Buffer.from(sha256(preimage)).toString("hex");
}

for (const objectId of objectDirs) {
	const dir = join(CHANGES_DIR, objectId);
	const files = readdirSync(dir).filter((f) => f.endsWith(".pb")).sort();

	const changes = [];
	let bad = false;
	for (const f of files) {
		const bytes = new Uint8Array(readFileSync(join(dir, f)));
		const change = decodeChange(bytes);
		if (!change) {
			decodeFail++;
			failures.push(`${objectId}/${f}: DECODE FAIL`);
			bad = true;
			continue;
		}
		const expected = f.slice(0, -3);
		const raw = rawChangeId(bytes);
		if (raw !== expected) {
			idFail++;
			failures.push(`${objectId}/${f}: rawChangeId ${raw} != filename`);
		} else {
			idOk++;
		}
		// Canonical re-encode (mint path). Legacy TS-writer files emitted
		// explicit empty proto3 fields protobufjs 8 rightly skips, so this
		// is informational, not a failure.
		if (changeId(change) === expected) canonicalIdOk++;
		else canonicalIdLegacy++;
		if (change.id !== expected) {
			failures.push(`${objectId}/${f}: embedded id ${change.id} != filename`);
		}
		changes.push(change);
	}
	if (bad || changes.length === 0) {
		parityFail++;
		failures.push(`${objectId}: no decodable changes`);
		continue;
	}

	const mine = computeObject(changes);
	if (!mine) {
		parityFail++;
		failures.push(`${objectId}: computeObject returned null`);
		continue;
	}
	if (mine.deleted) deletedCount++;

	let server: unknown;
	try {
		const res = await fetch(`${SERVER}/api/objects/${objectId}`);
		if (!res.ok) {
			fetchFail++;
			failures.push(`${objectId}: server ${res.status}`);
			continue;
		}
		server = await res.json();
	} catch (e) {
		fetchFail++;
		failures.push(`${objectId}: fetch error ${e}`);
		continue;
	}

	if (stableJSON(mine) === stableJSON(server)) {
		parityOk++;
	} else {
		parityFail++;
		failures.push(`${objectId}: MISMATCH ${firstDiff(canon(mine), canon(server))}`);
	}
}

// Loose top-level .pb files (not object dirs): verify content address only.
let looseOk = 0;
let looseFail = 0;
for (const f of looseFiles) {
	const bytes = new Uint8Array(readFileSync(join(CHANGES_DIR, f)));
	const change = decodeChange(bytes);
	if (!change) {
		looseFail++;
		failures.push(`(loose) ${f}: DECODE FAIL`);
		continue;
	}
	if (rawChangeId(bytes) === f.slice(0, -3)) looseOk++;
	else {
		looseFail++;
		failures.push(`(loose) ${f}: rawChangeId mismatch`);
	}
}

console.log("── parity-replay ─────────────────────────────────────");
console.log(`object dirs:        ${objectDirs.length}`);
console.log(`loose .pb files:    ${looseFiles.length} (raw changeId ok: ${looseOk}, fail: ${looseFail})`);
console.log(`change files in dirs: ${idOk + idFail + decodeFail}`);
console.log(`  decode failures:  ${decodeFail}`);
console.log(`  raw changeId ok:  ${idOk}`);
console.log(`  raw changeId fail: ${idFail}`);
console.log(`  canonical re-encode id ok: ${canonicalIdOk} (legacy-writer bytes: ${canonicalIdLegacy})`);
console.log(`objects parity ok:  ${parityOk} (${deletedCount} deleted)`);
console.log(`objects mismatch:   ${parityFail}`);
console.log(`objects fetch fail: ${fetchFail}`);
if (failures.length > 0) {
	console.log(`\nfailures (${failures.length}):`);
	for (const f of failures.slice(0, 60)) console.log("  " + f);
	if (failures.length > 60) console.log(`  … ${failures.length - 60} more`);
}
process.exit(failures.length === 0 ? 0 : 1);
