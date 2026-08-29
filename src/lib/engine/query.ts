/**
 * Query engine — faithful TS port of the Odin server's query.odin +
 * the /api/query handler in server.odin (setId resolution, row shape).
 *
 * Standalone over ObjectJSON; no imports from replay.ts.
 */

import type { ObjectJSON, ValueJSON } from "$lib/types";
import type { QueryBody, QueryResultRowJSON } from "./contracts";

// ── Plain values ─────────────────────────────────────────────────────

type Plain =
	| { kind: "null" }
	| { kind: "string"; str: string }
	| { kind: "number"; num: number }
	| { kind: "bool"; b: boolean }
	| { kind: "list"; list: Plain[] };

const NULL: Plain = { kind: "null" };

function isPlainObject(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** value_to_plain ∘ value_from_json over ValueJSON (oneof key order matters). */
function valueToPlain(v: ValueJSON | undefined | null): Plain {
	if (!isPlainObject(v)) return NULL;
	if ("stringValue" in v) {
		return { kind: "string", str: typeof v.stringValue === "string" ? v.stringValue : "" };
	}
	if ("intValue" in v) {
		const n = v.intValue;
		return { kind: "number", num: typeof n === "number" ? Math.trunc(n) : 0 };
	}
	if ("floatValue" in v) {
		const n = v.floatValue;
		return { kind: "number", num: typeof n === "number" ? n : 0 };
	}
	if ("boolValue" in v) {
		return { kind: "bool", b: v.boolValue === true };
	}
	if ("listValue" in v) {
		const list: Plain[] = [];
		const values = isPlainObject(v.listValue) ? v.listValue.values : undefined;
		if (Array.isArray(values)) {
			for (const s of values) if (typeof s === "string") list.push({ kind: "string", str: s });
		}
		return { kind: "list", list };
	}
	if ("mapValue" in v) return NULL; // maps aren't comparable
	if ("valuesValue" in v) {
		const list: Plain[] = [];
		const items = isPlainObject(v.valuesValue) ? v.valuesValue.items : undefined;
		if (Array.isArray(items)) for (const item of items) list.push(valueToPlain(item));
		return { kind: "list", list };
	}
	if ("linkValue" in v) {
		const target = isPlainObject(v.linkValue) ? v.linkValue.targetId : undefined;
		return { kind: "string", str: typeof target === "string" ? target : "" };
	}
	return NULL; // empty Value / bytesValue
}

/** json_to_plain: raw JSON filter values; objects are ValueJSON-shaped. */
function jsonToPlain(v: unknown): Plain {
	if (typeof v === "string") return { kind: "string", str: v };
	if (typeof v === "number") return { kind: "number", num: v };
	if (typeof v === "boolean") return { kind: "bool", b: v };
	if (Array.isArray(v)) return { kind: "list", list: v.map(jsonToPlain) };
	if (isPlainObject(v)) return valueToPlain(v as ValueJSON);
	return NULL;
}

function readRecordValue(s: ObjectJSON, key: string): Plain {
	switch (key) {
		case "id":
			return { kind: "string", str: s.id };
		case "type":
		case "typeKey":
			return { kind: "string", str: s.typeKey };
		case "createdAt":
			return { kind: "number", num: s.createdAt };
		case "updatedAt":
			return { kind: "number", num: s.updatedAt };
		case "deleted":
			return { kind: "bool", b: s.deleted };
	}
	if (Object.prototype.hasOwnProperty.call(s.fields, key)) return valueToPlain(s.fields[key]);
	return NULL;
}

function isEmptyPlain(p: Plain): boolean {
	switch (p.kind) {
		case "null":
			return true;
		case "string":
			return p.str === "";
		case "number":
			return p.num === 0;
		case "bool":
			return !p.b;
		case "list":
			return p.list.length === 0;
	}
}

function plainEqual(a: Plain, b: Plain): boolean {
	if (a.kind === "null" && b.kind === "null") return true;
	if (a.kind === "string" && b.kind === "string") return a.str === b.str;
	if (a.kind === "number" && b.kind === "number") return a.num === b.num;
	if (a.kind === "bool" && b.kind === "bool") return a.b === b.b;
	if (a.kind === "list" && b.kind === "list") {
		if (a.list.length !== b.list.length) return false;
		for (let i = 0; i < a.list.length; i++) if (!plainEqual(a.list[i], b.list[i])) return false;
		return true;
	}
	return false;
}

/** strings.compare — bytewise UTF-8 order == code-point order. */
function compareStrings(a: string, b: string): number {
	if (a === b) return 0;
	const ia = a[Symbol.iterator]();
	const ib = b[Symbol.iterator]();
	for (;;) {
		const x = ia.next();
		const y = ib.next();
		if (x.done && y.done) return 0;
		if (x.done) return -1;
		if (y.done) return 1;
		const cx = x.value.codePointAt(0)!;
		const cy = y.value.codePointAt(0)!;
		if (cx !== cy) return cx < cy ? -1 : 1;
	}
}

function plainString(p: Plain): string {
	switch (p.kind) {
		case "string":
			return p.str;
		case "number":
			return String(p.num);
		case "bool":
			return p.b ? "true" : "false";
		default:
			return "";
	}
}

function comparePlain(a: Plain, b: Plain): number {
	if (a.kind === "number" && b.kind === "number") {
		if (a.num < b.num) return -1;
		if (a.num > b.num) return 1;
		return 0;
	}
	if (a.kind === "bool" && b.kind === "bool") {
		return (a.b ? 1 : 0) - (b.b ? 1 : 0);
	}
	return compareStrings(plainString(a), plainString(b));
}

function asList(p: Plain): Plain[] {
	if (p.kind === "null") return [];
	if (p.kind === "list") return p.list;
	return [p];
}

function listContains(haystack: Plain[], needle: Plain): boolean {
	for (const h of haystack) if (plainEqual(h, needle)) return true;
	return false;
}

// ── JSON helpers (json_str / json_int / json_bool semantics) ─────────

function jsonStr(v: unknown, key: string): string {
	if (!isPlainObject(v)) return "";
	const field = v[key];
	return typeof field === "string" ? field : "";
}

function jsonInt(v: unknown, key: string): [number, boolean] {
	if (!isPlainObject(v)) return [0, false];
	if (!(key in v)) return [0, false];
	const field = v[key];
	if (typeof field === "number") return [Math.trunc(field), true];
	return [0, false];
}

// ── Filters ──────────────────────────────────────────────────────────

const DAY_MS = 86_400_000;

interface QuickRange {
	start: number;
	end: number;
}

function modF64(a: number, b: number): number {
	let m = a - b * Math.trunc(a / b);
	if (m < 0) m += b;
	return m;
}

function quickOptionRange(option: string, value: Plain, nowUnixMs: number): QuickRange | null {
	// Local-time day boundaries approximated as UTC days (matches server).
	const dayStart = nowUnixMs - modF64(nowUnixMs, DAY_MS);
	// Week starts Monday: unix epoch (1970-01-01) was a Thursday (weekday 3).
	const daysSinceEpoch = dayStart / DAY_MS;
	const weekday = modF64(daysSinceEpoch + 3, 7);
	const weekStart = dayStart - weekday * DAY_MS;
	const days = value.kind === "number" ? value.num : 0;

	switch (option) {
		case "today":
			return { start: dayStart, end: dayStart + DAY_MS - 1 };
		case "yesterday":
			return { start: dayStart - DAY_MS, end: dayStart - 1 };
		case "tomorrow":
			return { start: dayStart + DAY_MS, end: dayStart + 2 * DAY_MS - 1 };
		case "currentWeek":
			return { start: weekStart, end: weekStart + 7 * DAY_MS - 1 };
		case "lastWeek":
			return { start: weekStart - 7 * DAY_MS, end: weekStart - 1 };
		case "nextWeek":
			return { start: weekStart + 7 * DAY_MS, end: weekStart + 14 * DAY_MS - 1 };
		case "numberOfDaysAgo":
			return { start: dayStart - days * DAY_MS, end: dayStart - 1 };
		case "numberOfDaysNow":
			return { start: dayStart, end: dayStart + days * DAY_MS - 1 };
		case "exactDate": {
			const ts = value.kind === "number" ? value.num : 0;
			const s = ts - modF64(ts, DAY_MS);
			return { start: s, end: s + DAY_MS - 1 };
		}
	}
	return null;
}

function evalCondition(v: Plain, condition: string, filterValue: Plain): boolean {
	switch (condition) {
		case "equal":
		case "": {
			if (v.kind === "list" || filterValue.kind === "list") {
				const a = asList(v);
				const b = asList(filterValue);
				if (a.length !== b.length) return false;
				for (let i = 0; i < a.length; i++) if (!plainEqual(a[i], b[i])) return false;
				return true;
			}
			return plainEqual(v, filterValue);
		}
		case "notEqual":
			return !evalCondition(v, "equal", filterValue);
		case "greater":
		case "less":
		case "greaterOrEqual":
		case "lessOrEqual": {
			if (v.kind === "null" || filterValue.kind === "null" || v.kind === "list" || filterValue.kind === "list") return false;
			const c = comparePlain(v, filterValue);
			switch (condition) {
				case "greater":
					return c > 0;
				case "less":
					return c < 0;
				case "greaterOrEqual":
					return c >= 0;
			}
			return c <= 0;
		}
		case "like":
		case "notLike": {
			let hit = false;
			if (v.kind !== "null") {
				hit = plainString(v).toLowerCase().includes(plainString(filterValue).toLowerCase());
			}
			return condition === "like" ? hit : !hit;
		}
		case "in":
		case "notIn": {
			const fset = asList(filterValue);
			const vset = asList(v);
			let hit = false;
			for (const x of vset) {
				if (listContains(fset, x)) {
					hit = true;
					break;
				}
			}
			return condition === "in" ? hit : !hit;
		}
		case "allIn":
		case "notAllIn": {
			const fset = asList(filterValue);
			const vset = asList(v);
			let hit = fset.length > 0;
			for (const x of fset) {
				if (!listContains(vset, x)) {
					hit = false;
					break;
				}
			}
			return condition === "allIn" ? hit : !hit;
		}
		case "exactIn":
		case "notExactIn": {
			const fset = asList(filterValue);
			const vset = asList(v);
			let hit = fset.length === vset.length;
			if (hit) {
				for (const x of fset) {
					if (!listContains(vset, x)) {
						hit = false;
						break;
					}
				}
			}
			return condition === "exactIn" ? hit : !hit;
		}
		case "empty":
			return isEmptyPlain(v);
		case "notEmpty":
			return !isEmptyPlain(v);
		case "exists":
			return v.kind !== "null";
	}
	return false;
}

function evalDateWindow(v: Plain, condition: string, window: QuickRange): boolean {
	if (v.kind !== "number") return condition === "notEqual";
	const x = v.num;
	switch (condition) {
		case "equal":
		case "":
			return x >= window.start && x <= window.end;
		case "notEqual":
			return x < window.start || x > window.end;
		case "greater":
			return x > window.end;
		case "greaterOrEqual":
			return x >= window.start;
		case "less":
			return x < window.start;
		case "lessOrEqual":
			return x <= window.end;
	}
	return evalCondition(v, condition, NULL);
}

/** One filter (possibly a nested and/or group) against a state. */
function matchesFilter(s: ObjectJSON, filter: unknown, nowMs: number): boolean {
	if (isPlainObject(filter) && "nested" in filter) {
		const nested = filter.nested;
		if (Array.isArray(nested) && nested.length > 0) {
			const op = jsonStr(filter, "operator");
			if (op === "or") {
				for (const f of nested) if (matchesFilter(s, f, nowMs)) return true;
				return false;
			}
			for (const f of nested) if (!matchesFilter(s, f, nowMs)) return false;
			return true;
		}
	}
	const key = jsonStr(filter, "key");
	const condition = jsonStr(filter, "condition");
	if (condition === "exists") {
		switch (key) {
			case "id":
			case "type":
			case "typeKey":
			case "deleted":
			case "createdAt":
			case "updatedAt":
				return true;
		}
		return Object.prototype.hasOwnProperty.call(s.fields, key);
	}
	const v = readRecordValue(s, key);
	const rawValue = isPlainObject(filter) ? filter.value : undefined;
	const fv = jsonToPlain(rawValue);
	const quick = jsonStr(filter, "quickOption");
	if (quick !== "") {
		const window = quickOptionRange(quick, fv, nowMs);
		if (window !== null) return evalDateWindow(v, condition, window);
	}
	return evalCondition(v, condition, fv);
}

// ── Text search ──────────────────────────────────────────────────────

function textMatches(s: ObjectJSON, needle: string): boolean {
	const n = needle.toLowerCase();
	if (s.id.toLowerCase().includes(n)) return true;
	if (s.typeKey.toLowerCase().includes(n)) return true;
	for (const key of Object.keys(s.fields)) {
		const flat = asList(valueToPlain(s.fields[key]));
		for (const x of flat) {
			if (x.kind === "string" && x.str.toLowerCase().includes(n)) return true;
		}
	}
	// Fulltext over block content (Anytype's ObjectSearchWithMeta scope).
	for (const b of s.blocks) {
		const text = b.content?.text?.text;
		if (typeof text !== "string") continue;
		if (text.toLowerCase().includes(n)) return true;
	}
	return false;
}

/** First block whose text matches, trimmed around the hit — the result
 * row's snippet line (Anytype's search meta). */
function textSnippet(s: ObjectJSON, needle: string): string {
	const n = needle.toLowerCase();
	for (const b of s.blocks) {
		const text = b.content?.text?.text;
		if (typeof text !== "string") continue;
		const idx = text.toLowerCase().indexOf(n);
		if (idx < 0) continue;
		const start = Math.max(idx - 40, 0);
		const end = Math.min(idx + n.length + 60, text.length);
		let out = text.slice(start, end);
		if (start > 0) out = "…" + out;
		if (end < text.length) out = out + "…";
		return out;
	}
	return "";
}

// ── setId → extra filter (server.odin resolve_set_filter) ────────────

/** Anytype resolveSources: type keys → type-in; relation keys → exists; OR. */
function resolveSetFilter(states: Map<string, ObjectJSON>, setObj: ObjectJSON): unknown {
	const sources: string[] = [];
	const setOf = setObj.fields["setOf"];
	if (isPlainObject(setOf) && "valuesValue" in setOf) {
		const items = isPlainObject(setOf.valuesValue) ? setOf.valuesValue.items : undefined;
		if (Array.isArray(items)) {
			for (const item of items) {
				if (isPlainObject(item) && typeof item.stringValue === "string") sources.push(item.stringValue);
			}
		}
	}
	if (sources.length === 0) return null;

	const relationKeys = new Set<string>();
	for (const s of states.values()) {
		if (s.typeKey !== "relation") continue;
		const k = s.fields["key"];
		if (isPlainObject(k) && typeof k.stringValue === "string") relationKeys.add(k.stringValue);
	}

	const parts: unknown[] = [];
	const typeValues: string[] = [];
	for (const src of sources) {
		if (relationKeys.has(src)) {
			parts.push({ key: src, condition: "exists" });
		} else {
			typeValues.push(src);
		}
	}
	if (typeValues.length > 0) {
		parts.push({ key: "type", condition: "in", value: typeValues });
	}
	if (parts.length === 1) return parts[0];
	return { operator: "or", nested: parts };
}

// ── Entry point ──────────────────────────────────────────────────────

interface SortSpec {
	key: string;
	desc: boolean;
	emptyPlacement: string;
}

export type QueryRow = QueryResultRowJSON & { name: string; snippet?: string };

export function runQuery(
	states: Iterable<ObjectJSON>,
	body: QueryBody,
): { total: number; records: QueryRow[] } {
	const byId = new Map<string, ObjectJSON>();
	for (const s of states) byId.set(s.id, s);

	const now = Date.now();
	const includeDeleted = body.includeDeleted === true;
	const text = jsonStr(body, "textQuery");
	const typeEq = jsonStr(body, "type");

	const filters: unknown[] = Array.isArray(body.filters) ? body.filters : [];

	// setId: resolve the set's sources into an extra filter.
	let extra: unknown = null;
	const setId = jsonStr(body, "setId");
	if (setId !== "") {
		const setObj = byId.get(setId);
		if (setObj !== undefined) extra = resolveSetFilter(byId, setObj);
	}

	const out: ObjectJSON[] = [];
	outer: for (const s of byId.values()) {
		if (s.deleted && !includeDeleted) continue;
		if (typeEq !== "" && s.typeKey !== typeEq) continue;
		for (const f of filters) {
			if (!matchesFilter(s, f, now)) continue outer;
		}
		if (extra !== null && !matchesFilter(s, extra, now)) continue;
		if (text !== "" && !textMatches(s, text)) continue;
		out.push(s);
	}

	// Sorts (hierarchical, empties per placement, id tiebreak).
	const sorts: SortSpec[] = [];
	if (Array.isArray(body.sorts)) {
		for (const spec of body.sorts) {
			sorts.push({
				key: jsonStr(spec, "key"),
				desc: jsonStr(spec, "type") === "desc",
				emptyPlacement: jsonStr(spec, "emptyPlacement"),
			});
		}
	}
	if (sorts.length > 0) {
		out.sort((a, b) => {
			for (const s of sorts) {
				const va = readRecordValue(a, s.key);
				const vb = readRecordValue(b, s.key);
				const ea = isEmptyPlain(va);
				const eb = isEmptyPlain(vb);
				if (ea || eb) {
					if (ea && eb) continue;
					const emptyFirst = s.emptyPlacement === "start";
					return ea ? (emptyFirst ? -1 : 1) : emptyFirst ? 1 : -1;
				}
				const c = comparePlain(va, vb);
				if (c !== 0) return s.desc ? -c : c;
			}
			return compareStrings(a.id, b.id);
		});
	}

	// Paging.
	const [offset, hasOffset] = jsonInt(body, "offset");
	const [limit, hasLimit] = jsonInt(body, "limit");
	let start = hasOffset ? offset : 0;
	if (start > out.length) start = out.length;
	let end = hasLimit ? start + limit : out.length;
	if (end > out.length) end = out.length;
	const paged = out.slice(start, end);

	// Rows (server.odin handle_query shape). Note: the server's `total`
	// is the POST-paging record count, not the pre-paging match count.
	const records: QueryRow[] = paged.map((s) => {
		let name = "";
		const nv = s.fields["name"];
		if (isPlainObject(nv) && "stringValue" in nv) {
			name = typeof nv.stringValue === "string" ? nv.stringValue : "";
		}
		const row: QueryRow = {
			id: s.id,
			typeKey: s.typeKey,
			name,
			fields: s.fields,
			createdAt: s.createdAt,
			updatedAt: s.updatedAt,
		};
		if (text !== "") {
			const snippet = textSnippet(s, text);
			if (snippet !== "") row.snippet = snippet;
		}
		return row;
	});

	return { total: records.length, records };
}
