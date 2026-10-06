/**
 * What a saved view's filters say about anything made inside it: every
 * equal / in / allIn filter on a property becomes that property's value, in
 * the property's own shape - so a record born in a filtered view satisfies
 * the view (Anytype getDetails), and an agent made from a row of the view
 * carries the same values (an agent made in "Team Tasks" is Customer: Team).
 */
import type { RelationDefJSON, ValueJSON } from "$lib/types";

/** The view object's stored filters (`viewFilters`): key, condition, values. */
function viewFilters(fields: Record<string, ValueJSON>): Array<{ key: string; condition: string; value: string[] }> {
	return (fields["viewFilters"]?.valuesValue?.items ?? []).flatMap((item) => {
		const e = item.mapValue?.entries;
		if (!e) return [];
		const value = (e["value"]?.valuesValue?.items ?? []).map((i) => i.stringValue).filter((s): s is string => typeof s === "string");
		return [{ key: e["key"]?.stringValue ?? "", condition: e["condition"]?.stringValue ?? "equal", value }];
	});
}

/** The fields the view's filters give a new object. */
export function seedsFromView(viewFields: Record<string, ValueJSON>, relations: RelationDefJSON[]): Record<string, ValueJSON> {
	const fields: Record<string, ValueJSON> = {};
	for (const f of viewFilters(viewFields)) {
		if (!["equal", "in", "allIn"].includes(f.condition)) continue;
		const rel = relations.find((r) => r.key === f.key);
		if (!rel) continue;
		if (rel.format === "checkbox") {
			// Checkbox equal-filters carry no value list: equal means
			// "checked" (an explicit first value overrides).
			fields[f.key] = { boolValue: f.value[0] === undefined ? true : f.value[0] !== "false" };
			continue;
		}
		if (f.value.length === 0) continue;
		if (rel.format === "tag" || rel.format === "status") fields[f.key] = { valuesValue: { items: f.value.map((v) => ({ stringValue: v })) } };
		else if (rel.format === "number") fields[f.key] = { floatValue: Number(f.value[0]) || 0 };
		else if (rel.format === "date") fields[f.key] = { intValue: Number(f.value[0]) || Date.now() };
		else fields[f.key] = { stringValue: f.value[0] };
	}
	return fields;
}
