/**
 * Types and properties the UI refuses to delete: Roostr's own (core
 * built-ins carry `bundled`, harness seeds carry `system`) and anything a
 * query, template or property in the same space depends on. UI-level only:
 * the engine still deletes whatever it is told to.
 */
import { fetchAllQuery, fetchObject, type QueryResultRow } from "$lib/api";
import { store } from "$lib/data.svelte";
import { fieldStr, type ValueJSON } from "$lib/types";

type Fields = Record<string, ValueJSON>;
interface Def {
	id: string;
	typeKey: string;
	fields: Fields;
}

const strItems = (v: ValueJSON | undefined): string[] => (v?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean);
const mapKeys = (v: ValueJSON | undefined): string[] => (v?.valuesValue?.items ?? []).map((i) => i.mapValue?.entries?.["key"]?.stringValue ?? "").filter(Boolean);
const name = (r: QueryResultRow): string => fieldStr(r.fields, "name") || "Untitled";

/** Why `def` (a type or relation object) can't be deleted, or "" when it can. */
export async function deletionBlocker(def: Def): Promise<string> {
	if (def.typeKey !== "type" && def.typeKey !== "relation") return "";
	if (def.fields["bundled"]?.boolValue) return "Built into Roostr";
	if (def.fields["system"]?.boolValue) return "A Roostr default";
	const key = fieldStr(def.fields, "key");
	const space = fieldStr(def.fields, "channel");
	if (def.typeKey === "relation" && key) {
		// A Skill names the property its result is written to (Jev "Writes to").
		const writers = await fetchAllQuery({ filters: [{ key: "jev_writes", condition: "equal", value: key }, { key: "channel", condition: "equal", value: space }] });
		if (writers[0]) return `Skill “${name(writers[0])}” writes to it`;
	}
	const rows = await fetchAllQuery({
		filters: [
			{ key: "typeKey", condition: "in", value: def.typeKey === "type" ? ["query", "collection", "set", "template", "relation"] : ["query", "collection", "set"] },
			{ key: "channel", condition: "equal", value: space },
		],
	});
	for (const r of rows) {
		if (r.id === def.id) continue;
		if (def.typeKey === "type") {
			if (r.typeKey === "template" && fieldStr(r.fields, "target_type") === def.id) return `Used by template “${name(r)}”`;
			if (r.typeKey === "relation" && strItems(r.fields["object_types"]).includes(def.id)) return `Property “${name(r)}” is limited to it`;
			if (key && strItems(r.fields["setOf"]).includes(key)) return `Used by query “${name(r)}”`;
		} else if (
			key &&
			(mapKeys(r.fields["viewFilters"]).includes(key) ||
				mapKeys(r.fields["viewSorts"]).includes(key) ||
				fieldStr(r.fields, "viewGroupKey") === key ||
				fieldStr(r.fields, "viewDateKey") === key)
		) {
			return `Used by query “${name(r)}”`;
		}
	}
	return "";
}

/** Blockers for a selection of object ids, keyed by id; ids absent from the map are deletable. */
export async function blockersFor(ids: string[]): Promise<Map<string, string>> {
	const out = new Map<string, string>();
	for (const id of ids) {
		const isDef = store.relations.some((r) => r.id === id) || store.summaries.some((s) => s.id === id && s.typeKey === "type");
		if (!isDef) continue;
		const why = await deletionBlocker(await fetchObject(id));
		if (why) out.set(id, why);
	}
	return out;
}
