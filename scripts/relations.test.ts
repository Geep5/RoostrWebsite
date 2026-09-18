/**
 * A relation crossing the wire must arrive usable.
 *
 * `RelationDefJSON.options` is declared non-optional and indexed directly by
 * ten call sites (`rel.options.find(...)` in FeaturedProps, KanbanView,
 * SetTable, OptionPicker, …). A daemon build that omitted it therefore did not
 * degrade - it threw inside a `$derived` while rendering, which Svelte cannot
 * recover from, so every object featuring a select or tag property showed a
 * permanent "Loading…" with no error and no chat pane.
 *
 * The daemon was fixed (`src/server.odin` built the array and never assigned
 * it), but the client cannot assume the host is the build it shipped with:
 * an older daemon, a third-party host, or a future field are all normal.
 */

import { expect, test } from "bun:test";
import { normalizeRelations } from "../src/lib/local-backend";
import type { RelationDefJSON } from "../src/lib/types";

const relation = (over: Partial<RelationDefJSON>): RelationDefJSON =>
	({ id: "r", key: "status", name: "Status", format: "status", options: [], ...over }) as RelationDefJSON;

test("a relation with no options list gets an empty one", () => {
	const missing = relation({});
	delete (missing as { options?: unknown }).options;
	const [fixed] = normalizeRelations([missing]);
	expect(Array.isArray(fixed.options)).toBe(true);
	expect(fixed.options).toEqual([]);
	// The shape every call site relies on.
	expect(fixed.options.find((o) => o.text === "Done")).toBeUndefined();
});

test("a wrong type is replaced, not trusted", () => {
	// A host that sends null, or an object, must not reach `.find`.
	const nulled = relation({ options: null as unknown as RelationDefJSON["options"] });
	const objected = relation({ options: {} as unknown as RelationDefJSON["options"] });
	const [a, b] = normalizeRelations([nulled, objected]);
	expect(a.options).toEqual([]);
	expect(b.options).toEqual([]);
});

test("real options are left exactly as sent", () => {
	const options = [
		{ id: "1", text: "Todo", color: "grey", orderId: "000000" },
		{ id: "2", text: "Done", color: "green", orderId: "000001" },
	];
	const [fixed] = normalizeRelations([relation({ options })]);
	expect(fixed.options).toBe(options);
	expect(fixed.options.map((o) => o.text)).toEqual(["Todo", "Done"]);
});
