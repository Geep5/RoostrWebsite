/**
 * The space's schema graph: types on an outer ring (each a cluster of its
 * objects, sized by count), the properties those objects use on an inner
 * ring, and a curve from each type to each property its objects hold,
 * weighted by how many of them do. Object-format properties limited to a
 * type also point at that type.
 */

import { fetchAllQuery, type QueryResultRow } from "$lib/api";
import type { RelationDefJSON, ValueJSON } from "$lib/types";
import type { TypeDef } from "$lib/data.svelte";

export interface GraphObject {
	id: string;
	name: string;
	x: number;
	y: number;
	/** Keys of the properties this object holds a value for. */
	held: Set<string>;
	fields: Record<string, ValueJSON>;
}

export interface GraphType {
	index: number;
	key: string;
	name: string;
	icon: string;
	color: string;
	/** Cluster radius (world units). */
	rad: number;
	/** Angle on the outer ring; x/y is the cluster centre. */
	a: number;
	x: number;
	y: number;
	objects: GraphObject[];
	/** Property key → share of this type's objects holding it (0..1]. */
	usage: Map<string, number>;
}

export interface GraphProperty {
	key: string;
	name: string;
	format: string;
	icon: string;
	a: number;
	x: number;
	y: number;
	/** Types whose objects use it. */
	users: GraphType[];
	/** Object-format property limited to types present here. */
	targets: GraphType[];
}

export interface SchemaGraph {
	types: GraphType[];
	props: GraphProperty[];
	/** Space properties no object here uses (the type panel's "available, unused"). */
	idle: RelationDefJSON[];
	objectCount: number;
	/** Outer (types) and inner (properties) ring radii. */
	rt: number;
	rp: number;
	/** Radius that contains everything, labels included: what "fit" frames. */
	extent: number;
}

/** Infrastructure kinds: their objects are configuration, not content. */
const HIDDEN_KINDS: Record<string, true> = {
	program: true,
	typescript: true,
	json: true,
	proto: true,
	relation: true,
	type: true,
	template: true,
	agent: true,
	skill: true,
	channel: true,
	vanish_log: true,
	user_prefs: true,
};

/** Spacing between neighbouring object dots inside a cluster (world units). */
export const DOT_SPACING = 13;

function hasValue(v: ValueJSON | undefined): boolean {
	if (!v) return false;
	if (v.stringValue !== undefined) return v.stringValue !== "";
	if (v.boolValue !== undefined) return v.boolValue;
	if (v.intValue !== undefined || v.floatValue !== undefined || v.linkValue) return true;
	if (v.valuesValue) return v.valuesValue.items.some(hasValue);
	if (v.listValue) return v.listValue.values.length > 0;
	if (v.mapValue) return Object.keys(v.mapValue.entries).length > 0;
	return false;
}

/**
 * Builds the schema graph of one space from its objects. `relations` and
 * `types` are the space's own definitions (spaceRelations / the store).
 */
export async function buildSchemaGraph(channelId: string, isDefaultChannel: boolean, relations: RelationDefJSON[], types: TypeDef[]): Promise<SchemaGraph> {
	const rows: QueryResultRow[] = (await fetchAllQuery({})).filter((r) => {
		if (HIDDEN_KINDS[r.typeKey]) return false;
		const ch = r.fields["channel"]?.stringValue ?? "";
		return ch === channelId || (ch === "" && isDefaultChannel);
	});
	// The space's properties, minus hidden plumbing; only these count as "held".
	const relByKey = new Map(relations.filter((r) => !r.hidden).map((r) => [r.key, r]));
	const typeDef = (key: string) => types.find((t) => t.key === key && t.space === channelId) ?? types.find((t) => t.key === key);

	const byKind = new Map<string, GraphObject[]>();
	for (const r of rows) {
		const held = new Set<string>();
		for (const [k, v] of Object.entries(r.fields)) if (relByKey.has(k) && hasValue(v)) held.add(k);
		const obj: GraphObject = {
			id: r.id,
			name: [r.fields["iconEmoji"]?.stringValue, r.fields["name"]?.stringValue || "Untitled"].filter(Boolean).join(" "),
			x: 0,
			y: 0,
			held,
			fields: r.fields,
		};
		const list = byKind.get(r.typeKey);
		if (list) list.push(obj);
		else byKind.set(r.typeKey, [obj]);
	}

	// ── Types on the outer ring, each an arc as wide as its cluster ──
	const kinds = [...byKind.entries()].sort((a, b) => (typeDef(a[0])?.name ?? a[0]).localeCompare(typeDef(b[0])?.name ?? b[0]));
	const n = kinds.length;
	const gtypes: GraphType[] = kinds.map(([key, objects], index) => {
		const usage = new Map<string, number>();
		for (const o of objects) for (const k of o.held) usage.set(k, (usage.get(k) ?? 0) + 1);
		for (const [k, c] of usage) usage.set(k, c / objects.length);
		const hue = ((index / Math.max(1, n)) * 330 + 15) % 360;
		const def = typeDef(key);
		return { index, key, name: def?.name || key || "Untyped", icon: def?.icon ?? "", color: `oklch(0.76 0.13 ${hue})`, rad: Math.sqrt(objects.length) * DOT_SPACING + 18, a: 0, x: 0, y: 0, objects, usage };
	});
	const sumD = gtypes.reduce((s, t) => s + 2 * t.rad, 0);
	// Room for every cluster plus a gap; never tighter than the design's ring.
	const rt = Math.max(1150, (sumD * 1.35) / (2 * Math.PI));
	const gap = (2 * Math.PI * rt - sumD) / Math.max(1, n);
	let acc = -(gtypes[0]?.rad ?? 0);
	for (const t of gtypes) {
		const s = acc + t.rad;
		acc += 2 * t.rad + gap;
		t.a = -Math.PI / 2 + s / rt;
		t.x = Math.cos(t.a) * rt;
		t.y = Math.sin(t.a) * rt;
		// Phyllotaxis: evenly packed, densest at the centre.
		for (const [j, o] of t.objects.entries()) {
			const rr = DOT_SPACING * Math.sqrt(j + 0.5);
			const th = j * 2.39996;
			o.x = t.x + rr * Math.cos(th);
			o.y = t.y + rr * Math.sin(th);
		}
	}

	// ── Properties in use, on the inner ring near the types that use them ──
	const typeById = new Map<string, GraphType>();
	for (const t of gtypes) {
		const def = typeDef(t.key);
		if (def) typeById.set(def.id, t);
		for (const d of types) if (d.key === t.key) typeById.set(d.id, t);
	}
	const props: (GraphProperty & { mean: number })[] = [];
	for (const [key, rel] of relByKey) {
		const users = gtypes.filter((t) => t.usage.has(key));
		if (users.length === 0) continue;
		let cx = 0;
		let cy = 0;
		for (const t of users) {
			const w = t.usage.get(key)! * Math.sqrt(t.objects.length);
			cx += Math.cos(t.a) * w;
			cy += Math.sin(t.a) * w;
		}
		// A property nearly every type uses sits at the top, not at a meaningless mean.
		const mean = users.length >= n * 0.9 ? -Math.PI / 2 + 0.001 : Math.atan2(cy, cx);
		const targets = rel.format === "object" ? [...new Set((rel.objectTypes ?? []).map((id) => typeById.get(id)).filter((t): t is GraphType => !!t))] : [];
		props.push({ key, name: rel.name || key, format: rel.format, icon: rel.iconEmoji ?? "", a: 0, x: 0, y: 0, users, targets, mean });
	}
	props.sort((a, b) => a.mean - b.mean);
	// Label room: ~26 world units of arc per property.
	const rp = Math.max(440, (props.length * 26) / (2 * Math.PI));
	for (const [k, p] of props.entries()) {
		p.a = -Math.PI / 2 + ((k + 0.5) * 2 * Math.PI) / props.length;
		p.x = Math.cos(p.a) * rp;
		p.y = Math.sin(p.a) * rp;
	}
	const used = new Set(props.map((p) => p.key));
	const maxRad = gtypes.reduce((m, t) => Math.max(m, t.rad), 0);
	return {
		types: gtypes,
		props: props.map(({ mean: _mean, ...p }) => p),
		idle: [...relByKey.values()].filter((r) => !used.has(r.key)),
		objectCount: rows.length,
		rt,
		rp,
		extent: rt + maxRad + 420,
	};
}
