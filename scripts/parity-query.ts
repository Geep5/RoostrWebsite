/**
 * Parity harness: local runQuery (src/lib/engine/query.ts) vs the live
 * Odin server's POST /api/query. Fetches every object state via the
 * changes manifest (includes deleted + hidden-type objects), then runs
 * a battery of query bodies through both engines and diffs the JSON.
 *
 *   bun run scripts/parity-query.ts
 */

import { runQuery } from "../src/lib/engine/query";
import type { ObjectJSON } from "../src/lib/types";

const BASE = "http://127.0.0.1:7333";
const SET_ID = "3b0763f9-6b2c-4933-8b38-6576a07e821f"; // "note query" (setOf: note)

// ── Fetch all states ─────────────────────────────────────────────────

async function fetchStates(): Promise<ObjectJSON[]> {
	const manifest = (await (await fetch(`${BASE}/api/changes`)).json()) as Record<string, unknown>;
	const ids = Object.keys(manifest);
	const out: ObjectJSON[] = [];
	const CONC = 32;
	for (let i = 0; i < ids.length; i += CONC) {
		const batch = ids.slice(i, i + CONC);
		const objs = await Promise.all(
			batch.map(async (id) => {
				const res = await fetch(`${BASE}/api/objects/${id}`);
				if (!res.ok) return null;
				return (await res.json()) as ObjectJSON;
			}),
		);
		for (const o of objs) if (o !== null) out.push(o);
	}
	return out;
}

// ── Canonical JSON (sorted object keys, array order preserved) ───────

function canonical(v: unknown): string {
	if (v === null || typeof v !== "object") return JSON.stringify(v);
	if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
	const obj = v as Record<string, unknown>;
	const keys = Object.keys(obj).sort();
	return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(obj[k])}`).join(",")}}`;
}

interface QueryResult {
	total: number;
	records: { id: string }[];
}

/** unordered: server iterates a hash map — no-sort queries have nondeterministic order. */
function normalize(result: QueryResult, unordered: boolean): string {
	const records = unordered ? [...result.records].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)) : result.records;
	return canonical({ total: result.total, records });
}

// ── Battery ──────────────────────────────────────────────────────────

interface Case {
	name: string;
	body: Record<string, unknown>;
	/** compare as id-sorted multiset (body has no sorts → server order is map order) */
	unordered?: boolean;
}

const sortId = [{ key: "id", type: "asc" }];
const now = Date.now();

const CASES: Case[] = [
	{ name: "baseline all, sort id", body: { sorts: sortId } },
	{ name: "includeDeleted", body: { includeDeleted: true, sorts: sortId } },
	{ name: "body.type exact", body: { type: "note", sorts: sortId } },
	{ name: "equal string", body: { filters: [{ key: "name", condition: "equal", value: "Groceries" }] }, unordered: true },
	{ name: "empty condition = equal", body: { filters: [{ key: "name", condition: "", value: "Trip plan" }] }, unordered: true },
	{ name: "equal whole-list", body: { filters: [{ key: "tag", condition: "equal", value: ["errand"] }], sorts: sortId } },
	{ name: "equal scalar-vs-list", body: { filters: [{ key: "name", condition: "equal", value: ["Groceries"] }], sorts: sortId } },
	{ name: "notEqual", body: { filters: [{ key: "typeKey", condition: "notEqual", value: "typescript" }], sorts: sortId } },
	{ name: "greater number", body: { filters: [{ key: "size", condition: "greater", value: 10000 }], sorts: sortId } },
	{ name: "less number", body: { filters: [{ key: "lines", condition: "less", value: 50 }], sorts: sortId } },
	{ name: "greaterOrEqual createdAt", body: { filters: [{ key: "createdAt", condition: "greaterOrEqual", value: now - 30 * 86400000 }], sorts: sortId } },
	{ name: "lessOrEqual updatedAt", body: { filters: [{ key: "updatedAt", condition: "lessOrEqual", value: now }], sorts: sortId } },
	{ name: "greater on string", body: { filters: [{ key: "name", condition: "greater", value: "n" }], sorts: sortId } },
	{ name: "like", body: { filters: [{ key: "name", condition: "like", value: "QUERY" }], sorts: sortId } },
	{ name: "notLike", body: { type: "note", filters: [{ key: "name", condition: "notLike", value: "e" }], sorts: sortId } },
	{ name: "in list", body: { filters: [{ key: "tag", condition: "in", value: ["errand", "nope"] }], sorts: sortId } },
	{ name: "in scalar field", body: { filters: [{ key: "typeKey", condition: "in", value: ["note", "task"] }], sorts: sortId } },
	{ name: "notIn", body: { type: "note", filters: [{ key: "tag", condition: "notIn", value: ["errand"] }], sorts: sortId } },
	{ name: "allIn", body: { filters: [{ key: "tag", condition: "allIn", value: ["errand"] }], sorts: sortId } },
	{ name: "allIn empty filter set", body: { type: "note", filters: [{ key: "tag", condition: "allIn", value: [] }], sorts: sortId } },
	{ name: "notAllIn", body: { type: "note", filters: [{ key: "tag", condition: "notAllIn", value: ["errand"] }], sorts: sortId } },
	{ name: "exactIn", body: { filters: [{ key: "tag", condition: "exactIn", value: ["errand"] }], sorts: sortId } },
	{ name: "notExactIn", body: { type: "note", filters: [{ key: "tag", condition: "notExactIn", value: ["errand"] }], sorts: sortId } },
	{ name: "empty", body: { type: "note", filters: [{ key: "tag", condition: "empty" }], sorts: sortId } },
	{ name: "notEmpty", body: { filters: [{ key: "iconEmoji", condition: "notEmpty" }], sorts: sortId } },
	{ name: "exists field", body: { filters: [{ key: "done", condition: "exists" }], sorts: sortId } },
	{ name: "exists special key", body: { type: "note", filters: [{ key: "updatedAt", condition: "exists" }], sorts: sortId } },
	{ name: "deleted key filter", body: { includeDeleted: true, filters: [{ key: "deleted", condition: "equal", value: true }], sorts: sortId } },
	{
		name: "nested or",
		body: {
			filters: [{ operator: "or", nested: [{ key: "type", condition: "equal", value: "note" }, { key: "type", condition: "equal", value: "task" }] }],
			sorts: sortId,
		},
	},
	{
		name: "nested and",
		body: {
			filters: [{ operator: "and", nested: [{ key: "type", condition: "equal", value: "note" }, { key: "tag", condition: "notEmpty" }] }],
			sorts: sortId,
		},
	},
	{
		name: "nested or-of-and",
		body: {
			filters: [
				{
					operator: "or",
					nested: [
						{ operator: "and", nested: [{ key: "type", condition: "equal", value: "note" }, { key: "tag", condition: "notEmpty" }] },
						{ key: "type", condition: "equal", value: "task" },
					],
				},
			],
			sorts: sortId,
		},
	},
	{ name: "quickOption today equal", body: { filters: [{ key: "updatedAt", condition: "equal", quickOption: "today" }], sorts: sortId } },
	{ name: "quickOption currentWeek", body: { filters: [{ key: "updatedAt", condition: "", quickOption: "currentWeek" }], sorts: sortId } },
	{ name: "quickOption lastWeek notEqual", body: { filters: [{ key: "updatedAt", condition: "notEqual", quickOption: "lastWeek" }], sorts: sortId } },
	{ name: "quickOption exactDate", body: { filters: [{ key: "updatedAt", condition: "equal", quickOption: "exactDate", value: now }], sorts: sortId } },
	{ name: "quickOption numberOfDaysAgo greaterOrEqual", body: { filters: [{ key: "createdAt", condition: "greaterOrEqual", quickOption: "numberOfDaysAgo", value: 60 }], sorts: sortId } },
	{ name: "quickOption on non-number field", body: { filters: [{ key: "name", condition: "notEqual", quickOption: "today" }], sorts: sortId } },
	{ name: "textQuery blocks+fields", body: { textQuery: "groceries", sorts: sortId } },
	{ name: "textQuery type key", body: { textQuery: "pinned_fact", sorts: sortId } },
	{ name: "textQuery snippet trim", body: { textQuery: "the", type: "note", sorts: sortId } },
	{ name: "textQuery + filters", body: { textQuery: "note", filters: [{ key: "name", condition: "notEmpty" }], sorts: sortId } },
	{ name: "setId note query", body: { setId: SET_ID, sorts: sortId } },
	{ name: "setId + filter + sort name", body: { setId: SET_ID, filters: [{ key: "tag", condition: "in", value: ["errand"] }], sorts: [{ key: "name", type: "asc", emptyPlacement: "end" }] } },
	{ name: "setId unknown id", body: { setId: "definitely-not-an-object", sorts: sortId } },
	{ name: "sort name asc empties end", body: { sorts: [{ key: "name", type: "asc", emptyPlacement: "end" }] } },
	{ name: "sort name desc empties start", body: { sorts: [{ key: "name", type: "desc", emptyPlacement: "start" }] } },
	{ name: "sort name default placement", body: { sorts: [{ key: "name", type: "asc" }] } },
	{ name: "sort hierarchical", body: { sorts: [{ key: "typeKey", type: "asc" }, { key: "updatedAt", type: "desc" }] } },
	{ name: "sort numeric empties start", body: { sorts: [{ key: "size", type: "asc", emptyPlacement: "start" }] } },
	{ name: "sort numeric desc", body: { sorts: [{ key: "lines", type: "desc" }] } },
	{ name: "sort list key (id tiebreak)", body: { sorts: [{ key: "tag", type: "asc" }] } },
	{ name: "sort bool", body: { includeDeleted: true, sorts: [{ key: "deleted", type: "desc" }, { key: "id", type: "asc" }] } },
	{ name: "paging limit", body: { sorts: sortId, limit: 10 } },
	{ name: "paging offset+limit", body: { sorts: sortId, offset: 5, limit: 7 } },
	{ name: "paging offset beyond end", body: { sorts: sortId, offset: 100000, limit: 5 } },
	{ name: "paging limit 0", body: { sorts: sortId, limit: 0 } },
	{ name: "paging offset only", body: { sorts: sortId, offset: 400 } },
	{ name: "kitchen sink", body: { type: "note", textQuery: "e", filters: [{ key: "name", condition: "notEmpty" }], sorts: [{ key: "updatedAt", type: "desc" }], offset: 1, limit: 3 } },
];

// ── Run ──────────────────────────────────────────────────────────────

const states = await fetchStates();
console.log(`fetched ${states.length} object states`);

let pass = 0;
let fail = 0;
for (const c of CASES) {
	const res = await fetch(`${BASE}/api/query`, { method: "POST", body: JSON.stringify(c.body) });
	const server = (await res.json()) as QueryResult;
	const local = runQuery(states, c.body) as unknown as QueryResult;
	const sNorm = normalize(server, c.unordered === true);
	const lNorm = normalize(local, c.unordered === true);
	if (sNorm === lNorm) {
		pass++;
		console.log(`PASS  ${c.name}  (total=${server.total})`);
	} else {
		fail++;
		console.log(`FAIL  ${c.name}`);
		console.log(`  server total=${server.total} local total=${local.total}`);
		const sIds = server.records.map((r) => r.id);
		const lIds = local.records.map((r) => r.id);
		const onlyS = sIds.filter((id) => !lIds.includes(id));
		const onlyL = lIds.filter((id) => !sIds.includes(id));
		if (onlyS.length > 0) console.log(`  only server: ${onlyS.slice(0, 8).join(", ")}`);
		if (onlyL.length > 0) console.log(`  only local:  ${onlyL.slice(0, 8).join(", ")}`);
		if (onlyS.length === 0 && onlyL.length === 0) {
			for (let i = 0; i < sIds.length; i++) {
				if (sIds[i] !== lIds[i]) {
					console.log(`  order differs at [${i}]: server=${sIds[i]} local=${lIds[i]}`);
					break;
				}
			}
			// same ids, same order → row content differs
			for (let i = 0; i < server.records.length; i++) {
				const a = canonical(server.records[i]);
				const b = canonical(local.records[i]);
				if (a !== b) {
					console.log(`  row [${i}] (${sIds[i]}) differs:`);
					console.log(`    server: ${a.slice(0, 400)}`);
					console.log(`    local:  ${b.slice(0, 400)}`);
					break;
				}
			}
		}
	}
}
console.log(`\n${pass}/${pass + fail} passed`);
if (fail > 0) process.exit(1);
