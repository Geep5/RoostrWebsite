/**
 * Client-side reactive data store — replaces the SvelteKit server loads.
 * Holds channels, object summaries, and relation defs; refreshed from the
 * Odin backend and kept live via the /api/events SSE stream.
 */

import { fetchChannels, fetchObjects, fetchQuery, fetchRelations, fetchAllQuery } from "$lib/api";
import { backend } from "$lib/engine/backend";
import type { SpaceJSON, ObjectSummary, RelationDefJSON } from "$lib/types";

/** A type object (Anytype ObjectType analog). */
/**
 * Agents are space infrastructure, so `agent` sits in the server's
 * HIDDEN_LIST_TYPES and never appears in `summaries` — but a discussion still
 * has to name the one that replied. Carried separately for that.
 */
export interface AgentRef {
	id: string;
	name: string;
	icon: string;
}

export interface TypeDef {
	id: string;
	key: string;
	name: string;
	icon: string;
	layout: string; // "page" | "task"
	defaultTemplateId: string;
	/** Owning space id; "" = bundled/system type, present in every space. */
	space: string;
}

export const store = $state({
	channels: [] as SpaceJSON[],
	summaries: [] as ObjectSummary[],
	relations: [] as RelationDefJSON[],
	types: [] as TypeDef[],
	agents: [] as AgentRef[],
	loaded: false,
});

async function fetchTypes(): Promise<TypeDef[]> {
	const records = await fetchAllQuery({ type: "type" });
	const s = (f: Record<string, { stringValue?: string }>, k: string) => f[k]?.stringValue ?? "";
	return records
		.map((r) => ({
			id: r.id,
			key: s(r.fields, "key"),
			name: s(r.fields, "name"),
			icon: s(r.fields, "iconEmoji"),
			layout: s(r.fields, "layout") || "page",
			defaultTemplateId: s(r.fields, "default_template_id"),
			space: s(r.fields, "channel"),
		}))
		.filter((t) => t.key)
		.sort((a, b) => a.name.localeCompare(b.name));
}

async function fetchAgents(): Promise<AgentRef[]> {
	const records = await fetchAllQuery({ type: "agent" });
	return records.map((r) => ({
		id: r.id,
		name: r.fields["name"]?.stringValue ?? "",
		icon: r.fields["iconEmoji"]?.stringValue ?? "",
	}));
}

export async function refreshAll(): Promise<void> {
	const [channels, summaries, relations, types, agents] = await Promise.all([
		fetchChannels(),
		fetchObjects(),
		fetchRelations(),
		fetchTypes(),
		fetchAgents(),
	]);
	store.channels = channels;
	store.summaries = summaries;
	store.relations = relations;
	store.types = types;
	store.agents = agents;
	store.loaded = true;
}

/** Layout for a typeKey: the type object's layout, else legacy fallback. */
export function layoutOf(typeKey: string): string {
	return store.types.find((t) => t.key === typeKey)?.layout ?? (typeKey === "task" ? "task" : "page");
}

type ObjectListener = (objectId: string) => void;
const objectListeners = new Set<ObjectListener>();

/** Register a per-object SSE listener; returns an unsubscribe function. */
export function onObjectEvent(fn: ObjectListener): () => void {
	objectListeners.add(fn);
	return () => objectListeners.delete(fn);
}

/** Connect the commit stream (idempotent) - backend events replace SSE. */
let connected = false;

export function connectEvents(): () => void {
	if (connected) return () => {};
	connected = true;
	let timer: ReturnType<typeof setTimeout> | undefined;
	const off = backend.onCommit((ids) => {
		clearTimeout(timer);
		timer = setTimeout(() => void refreshAll(), 800);
		for (const id of ids) for (const fn of objectListeners) fn(id);
	});
	return () => {
		connected = false;
		off();
	};
}
