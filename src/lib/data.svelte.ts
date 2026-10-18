/**
 * Client-side reactive data store — replaces the SvelteKit server loads.
 * Holds channels, object summaries, and relation defs; refreshed from the
 * Odin backend and kept live via the /api/events SSE stream.
 */

import { fetchChannels, fetchObjects, fetchQuery, fetchRelations, fetchAllQuery, userPrefs } from "$lib/api";
import { backend } from "$lib/client-backend";
import { DEFAULT_CHAT_COLOR, normalizeHex } from "$lib/chat-color";
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
	/** Owning space: an agent lives in exactly one. Empty = unassigned. */
	channel: string;
	/** The agent's `chat_color` as #RRGGBB; "" = the default grey bubble. */
	chatColor: string;
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
	/** Built into Roostr (`bundled`) or a Roostr default (`system`). */
	builtin?: boolean;
}

export const store = $state({
	channels: [] as SpaceJSON[],
	summaries: [] as ObjectSummary[],
	relations: [] as RelationDefJSON[],
	types: [] as TypeDef[],
	/** Ids of types and properties that are built in or Roostr defaults - marked in the sidebar, never deletable from the UI. */
	builtinIds: new Set<string>(),
	agents: [] as AgentRef[],
	/** The user's own chat bubble color (Settings; synced on the personal stream). */
	chatColor: DEFAULT_CHAT_COLOR,
	/** This person's order of the space list (space id -> position), from their own preferences. */
	spaceOrder: {} as Record<string, number>,
	loaded: false,
});

/** Type and property definition records, read once for both the type list and the built-in marks. */
async function fetchDefs(): Promise<{ types: TypeDef[]; builtinIds: Set<string> }> {
	const records = await fetchAllQuery({ filters: [{ key: "typeKey", condition: "in", value: ["type", "relation"] }] });
	const s = (f: Record<string, { stringValue?: string }>, k: string) => f[k]?.stringValue ?? "";
	const builtinIds = new Set(records.filter((r) => r.fields["bundled"]?.boolValue || r.fields["system"]?.boolValue).map((r) => r.id));
	const types = records
		.filter((r) => r.typeKey === "type")
		.map((r) => ({
			id: r.id,
			key: s(r.fields, "key"),
			name: s(r.fields, "name"),
			icon: s(r.fields, "iconEmoji"),
			layout: s(r.fields, "layout") || "page",
			defaultTemplateId: s(r.fields, "default_template_id"),
			space: s(r.fields, "channel"),
			builtin: builtinIds.has(r.id),
		}))
		.filter((t) => t.key)
		.sort((a, b) => a.name.localeCompare(b.name));
	return { types, builtinIds };
}

async function fetchAgents(): Promise<AgentRef[]> {
	const records = await fetchAllQuery({ type: "agent" });
	return records.map((r) => ({
		id: r.id,
		name: r.fields["name"]?.stringValue ?? "",
		icon: r.fields["iconEmoji"]?.stringValue ?? "",
		channel: r.fields["channel"]?.stringValue ?? "",
		chatColor: normalizeHex(r.fields["chat_color"]?.stringValue ?? "") ?? "",
	}));
}

export async function refreshAll(): Promise<void> {
	const [channels, summaries, relations, defs, agents, prefs] = await Promise.all([
		fetchChannels(),
		fetchObjects(),
		fetchRelations(),
		fetchDefs(),
		fetchAgents(),
		userPrefs.read(),
	]);
	store.channels = channels;
	store.summaries = summaries;
	store.relations = relations;
	store.types = defs.types;
	store.builtinIds = defs.builtinIds;
	store.agents = agents;
	store.chatColor = prefs.chatColor;
	store.spaceOrder = prefs.spaceOrder;
	store.loaded = true;
}

/** Discussion drawer: the object page publishes availability + count; the
 * header chip (layout) toggles it. Open state survives navigation so you
 * can browse objects with the drawer docked. */
export const discussionUI = $state({ open: true, available: false, count: 0, convCount: 0 });

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
	// Throttle, not debounce: a catch-up commits every few hundred ms for
	// minutes, and a timer reset on each commit never fired - the spaces list
	// stayed as it was at load (nameless, iconless, or empty) until sync went quiet.
	// A commit during a running refresh queues one more after it.
	let timer: number | undefined;
	let running = false;
	let again = false;
	const refresh = async (): Promise<void> => {
		timer = undefined;
		if (running) { again = true; return; }
		running = true;
		try { await refreshAll(); } catch { /* the next commit retries */ }
		running = false;
		if (again) { again = false; timer ??= window.setTimeout(() => void refresh(), 800); }
	};
	const off = backend.onCommit((ids) => {
		timer ??= window.setTimeout(() => void refresh(), 800);
		for (const id of ids) for (const fn of objectListeners) fn(id);
	});
	return () => {
		connected = false;
		window.clearTimeout(timer);
		off();
	};
}
