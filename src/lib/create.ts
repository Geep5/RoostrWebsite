/**
 * Object creation helpers, channel-aware — shared by the home page, the
 * sidebar create button (Anytype's typeSuggest analog), and the search
 * modal's "Create object" row.
 */

import { goto } from "$app/navigation";
import { fetchAllQuery, fetchObject, note } from "$lib/api";
import { thisMachineId } from "$lib/capability-actions";
import { guestAgents, isBuiltinTool, type ValueJSON } from "$lib/types";
import { agentLinksValue } from "$lib/agent-field";
import { typeIcon } from "$lib/icons";
import { store } from "$lib/data.svelte";
import { activeSpace } from "$lib/space.svelte";
import { addFailureText, pickFile, uploadFile } from "$lib/files";

const channelField = (channelId: string): Record<string, ValueJSON> =>
	channelId ? { channel: { stringValue: channelId } } : {};

/** Legacy fallback until the type objects have loaded. */
export const CREATABLE_TYPES = ["page", "note", "task", "person", "project", "bookmark", "chat"] as const;

/** Creatable types for the sidebar dropdown: bundled types plus the
 *  active space's own — spaces are self-contained. */
export function creatableTypes(): Array<{ key: string; name: string; icon: string }> {
	if (store.types.length === 0) return CREATABLE_TYPES.map((k) => ({ key: k, name: k[0].toUpperCase() + k.slice(1), icon: typeGlyph(k) }));
	const sid = activeSpace.id || store.channels[0]?.id || "";
	const own = store.types.filter((t) => t.space === sid);
	if (own.length === 0) return CREATABLE_TYPES.map((k) => ({ key: k, name: k[0].toUpperCase() + k.slice(1), icon: typeGlyph(k) }));
	return own.map((t) => ({ key: t.key, name: t.name || t.key, icon: t.icon || typeGlyph(t.key) }));
}

export function typeGlyph(typeKey: string): string {
	return typeIcon(typeKey) || "▨";
}

/**
 * Copy a template into a fresh object (Anytype: ObjectCreate with
 * type.defaultTemplateId): its content blocks with ids remapped, and its
 * properties as the new object's DEFAULTS - an Agent template's Served by,
 * System prompt, Model, Skills… The discussion subtree and the fields that
 * describe the template itself (TEMPLATE_OWN) stay behind; the guest list
 * is normalized to the link-list shape however the template stored it.
 *
 * Defaults never overwrite: a property the new object already has wins.
 * Those come from where it was made - a filtered view's Status, the
 * computer a credential is pinned to, an agent's default prompt and tools -
 * and are more specific than the template's guess.
 */
export async function applyTemplate(objectId: string, templateId: string): Promise<void> {
	const [tpl, target] = await Promise.all([fetchObject(templateId), fetchObject(objectId)]);
	// Everything reachable from __discussion__ is conversation, not content.
	const byId = new Map(tpl.blocks.map((b) => [b.id, b]));
	const skip = new Set<string>();
	const markSkip = (id: string) => {
		if (skip.has(id)) return;
		skip.add(id);
		for (const c of byId.get(id)?.childrenIds ?? []) markSkip(c);
	};
	markSkip("__discussion__");

	const idMap = new Map<string, string>();
	for (const b of tpl.blocks) if (!skip.has(b.id)) idMap.set(b.id, crypto.randomUUID());
	for (const b of tpl.blocks) {
		if (skip.has(b.id)) continue;
		await note.blockAdd(objectId, {
			id: idMap.get(b.id)!,
			childrenIds: b.childrenIds.map((c) => idMap.get(c)).filter((c): c is string => !!c),
			content: b.content,
		});
	}
	for (const [key, value] of Object.entries(tpl.fields)) {
		if (TEMPLATE_OWN.has(key) || hasValue(target.fields[key])) continue;
		await note.setField(objectId, key, key === "agent" ? agentLinksValue(guestAgents(tpl.fields)) : value);
	}
}

/** Whether a field holds something: blank text and empty lists count as unset, so a template may fill them. */
function hasValue(v: ValueJSON | undefined): boolean {
	if (!v) return false;
	if (v.stringValue !== undefined) return v.stringValue !== "";
	if (v.valuesValue) return v.valuesValue.items.length > 0;
	if (v.listValue) return v.listValue.values.length > 0;
	return true;
}

/** A template's own identity and bookkeeping (the harness's seed_key/seed_hash included), never a default for what it creates. */
const TEMPLATE_OWN = new Set(["name", "target_type", "channel", "error", "createdDate", "modifiedDate", "type_key", "repeat", "seed_key", "seed_hash"]);

/**
 * A computer object is a machine's self-publication: it exists because a
 * running harness announced itself (stable machine id). Hand-crafting one
 * would mint an inert twin nothing serves, so New →
 * Computer adopts this device's row when its harness already published it,
 * and otherwise goes to setup - the machine creates its own object.
 */
async function createMachine(): Promise<string> {
	const id = await thisMachineId();
	if (id) {
		const mine = (await fetchAllQuery({ type: "machine" })).find((r) => r.fields["machine_id"]?.stringValue === id);
		if (mine) {
			await goto(`/app/object/${mine.id}`);
			return mine.id;
		}
	}
	// No machine registered here yet: there is nothing to link or open.
	return "";
}
/**
 * A new agent is an object configured the same way as any other: its
 * Computer, System prompt, Model, Requires and Credentials are properties.
 * The Agent type's default template supplies them as defaults (a template
 * with a Served by gives every agent it creates that computer). Without a
 * template prompt it links its space's "Assistant" system prompt when one
 * exists, so the prompt it runs on is always visible. It runs nowhere until
 * its Served by names a computer; until then the engine shows that on its
 * Error property.
 */
async function createAgent(channelId: string, name = ""): Promise<string> {
	const agentId = await newAgent(channelId, name);
	await goto(`/app/object/${agentId}`);
	return agentId;
}

/** The built-in Tools (by callable name) a new agent gets when its template sets none. */
const DEFAULT_TOOLS = ["shell_exec", "web_fetch"];

/**
 * A new agent in the space, from the agent type's default template there
 * (its model, prompt, skills, tools, credentials, computer), else pointed at the
 * space's "Assistant" prompt. Stays where you are - for pickers that link it.
 * `seeds` are fields it starts with (a view's filter values, view-seeds.ts);
 * the template fills in only what they leave empty.
 */
export async function newAgent(channelId: string, name = "", seeds: Record<string, ValueJSON> = {}): Promise<string> {
	const tplId = defaultTemplateOf("agent", channelId);
	const tpl = tplId ? await fetchObject(tplId).catch(() => null) : null;
	const assistant = tpl?.fields["prompt"]
		? undefined
		: (await fetchAllQuery({ type: "system_prompt", filters: [{ key: "channel", condition: "equal", value: channelId }] }))
			.find((r) => r.fields["name"]?.stringValue === "Assistant");
	// A normal agent can run commands and fetch the web: those are its
	// space's built-in shell_exec and web_fetch Tools, linked unless its
	// template sets Tools.
	const tools = tpl?.fields["tools"]
		? []
		: (await fetchAllQuery({ type: "tool", filters: [{ key: "channel", condition: "equal", value: channelId }] }))
			.filter((r) => isBuiltinTool(r.fields) && DEFAULT_TOOLS.includes(r.fields["name"]?.stringValue ?? ""));
	const { id: agentId } = await note.create(name.trim() || "New agent", "agent", {
		// A view's Agent filter seeds its records' guest list; an agent has none of its own.
		...Object.fromEntries(Object.entries(seeds).filter(([key]) => key !== "agent")),
		...channelField(channelId),
		...(assistant ? { prompt: { linkValue: { targetId: assistant.id, relationKey: "prompt" } } } : {}),
		...(tools.length ? { tools: { valuesValue: { items: tools.map((t) => ({ linkValue: { targetId: t.id, relationKey: "tools" } })) } } } : {}),
	});
	if (tpl) await applyTemplate(agentId, tpl.id);
	return agentId;
}
/** The type's default template in this space: every space carries its own copy of a bundled type. */
function defaultTemplateOf(key: string, channelId: string): string {
	const types = store.types.filter((t) => t.key === key);
	return (types.find((t) => t.space === channelId) ?? types[0])?.defaultTemplateId ?? "";
}
/**
 * A credential lives on one computer - its Chrome profile or keys never
 * leave it - so a new one starts pinned to the computer this tab is paired
 * with, when there is one. The type's default template still applies.
 */
async function createCredential(channelId: string, name: string): Promise<string> {
	const machineId = await thisMachineId();
	const { id } = await note.create(name || "New credential", "credential", {
		...channelField(channelId),
		...(machineId ? { served_by: { stringValue: machineId } } : {}),
	});
	const tplId = defaultTemplateOf("credential", channelId);
	if (tplId) await applyTemplate(id, tplId).catch(() => {}); // a deleted default template is a no-op
	await goto(`/app/object/${id}`);
	return id;
}
/**
 * A file is its bytes: the picker comes first, then they go to Blossom (or
 * through the harness of a Roostr computer paired with this tab), which
 * creates the File object. A cancelled pick creates nothing.
 */
async function createFile(channelId: string): Promise<string> {
	const file = await pickFile();
	if (!file) return "";
	try {
		const { id } = await uploadFile(file, channelId);
		await goto(`/app/object/${id}`);
		return id;
	} catch (e) {
		alert(addFailureText(e, file.name));
		return "";
	}
}
export async function createTyped(typeKey: string, channelId: string, name = ""): Promise<string> {
	const key = typeKey.trim().toLowerCase();
	if (key === "agent") return createAgent(channelId, name);
	if (key === "machine") return createMachine();
	if (key === "credential") return createCredential(channelId, name);
	if (key === "file") return createFile(channelId);
	const { id } = await note.create(name, key, channelField(channelId));
	const tplId = defaultTemplateOf(key, channelId);
	if (tplId) await applyTemplate(id, tplId).catch(() => {}); // a deleted default template is a no-op
	await goto(`/app/object/${id}`);
	return id;
}

export async function createCollection(channelId: string): Promise<string> {
	const { id } = await note.create("New collection", "collection", {
		...channelField(channelId),
		collectionIds: { valuesValue: { items: [] } },
	});
	await goto(`/app/object/${id}`);
	return id;
}

/** Create a sourceless query; the query page opens the type suggest (Anytype: a new set asks for its source). */
/**
 * Default content every fresh space starts with: a "Chats" query over
 * the chat type, so conversations are reachable the moment the space
 * exists. No navigation - the caller decides where to land.
 */
export async function seedSpaceDefaults(channelId: string): Promise<void> {
	await note.create("Chats", "query", {
		...channelField(channelId),
		setOf: { valuesValue: { items: [{ stringValue: "chat" }] } },
	});
}

export async function createQuery(channelId: string): Promise<string> {
	const { id } = await note.create("New query", "query", {
		...channelField(channelId),
		setOf: { valuesValue: { items: [] } },
	});
	await goto(`/app/object/${id}`);
	return id;
}
