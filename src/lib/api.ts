/**
 * Roostr Web client API - the same surface the desktop app gets from the
 * local Odin server, served by the in-browser relay replica instead.
 * Reads hit replayed local state; mutations build Changes locally and
 * publish them to relays (the home daemon imports them like any device).
 */

import type { ObjectJSON, ObjectSummary, SpaceJSON, RelationDefJSON, BlockJSON, ValueJSON } from "$lib/types";
import { backend } from "$lib/engine/backend";
import { loadKey, saveKey, authorIdFor, clearKey } from "$lib/engine/keys";
import { nip19 } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils.js";

/** Desktop-era base URL; unused on web but kept for API-shape parity. */
export const API = "";

export const fetchObject = (id: string): Promise<ObjectJSON> => backend.fetchObject(id);
export const fetchObjects = (): Promise<ObjectSummary[]> => backend.fetchObjects();
export const fetchChannels = (): Promise<SpaceJSON[]> => backend.fetchChannels();
export const fetchRelations = (): Promise<RelationDefJSON[]> => backend.fetchRelations();

export interface QueryResultRow {
	id: string;
	typeKey: string;
	name?: string;
	snippet?: string;
	createdAt: number;
	updatedAt: number;
	fields: Record<string, ValueJSON>;
}

export async function fetchQuery(body: Record<string, unknown>): Promise<{ total: number; records: QueryResultRow[] }> {
	return backend.fetchQuery(body) as unknown as Promise<{ total: number; records: QueryResultRow[] }>;
}

/**
 * Every match, a page at a time.
 *
 * `total` is the unpaged match count, so a full first page is the only
 * thing that costs a second round trip — and a vault that outgrows any
 * single limit stops silently dropping the tail. Use this wherever a
 * partial answer would be wrong (definitions, rosters); a capped
 * `fetchQuery` is right only when the cap IS the intent, like a
 * top-20 search.
 */
export async function fetchAllQuery(body: Record<string, unknown>, page = 500): Promise<QueryResultRow[]> {
	const first = await fetchQuery({ ...body, offset: 0, limit: page });
	if (first.records.length >= first.total) return first.records;
	const out = first.records.slice();
	while (out.length < first.total) {
		const next = await fetchQuery({ ...body, offset: out.length, limit: page });
		if (next.records.length === 0) break; // concurrent delete shrank the set
		out.push(...next.records);
	}
	return out;
}

async function mutate(action: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
	return backend.mutate(action, params);
}

export const note = {
	create: (name: string, typeKey?: string, fields?: Record<string, ValueJSON>) =>
		mutate("create", { name, type_key: typeKey, fields }) as Promise<{ id: string }>,
	blockAdd: (objectId: string, block: Partial<BlockJSON>, targetId = "", position = 0) =>
		mutate("block_add", { object_id: objectId, block, target_id: targetId, position }),
	blockUpdate: (objectId: string, blockId: string, content: BlockJSON["content"]) =>
		mutate("block_update", { object_id: objectId, block_id: blockId, content }),
	blockMove: (objectId: string, blockId: string, targetId: string, position: number) =>
		mutate("block_move", { object_id: objectId, block_id: blockId, target_id: targetId, position }),
	blockSetAttrs: (objectId: string, blockId: string, attrs: { align?: number; background_color?: string }) =>
		mutate("block_set_attrs", { object_id: objectId, block_id: blockId, ...attrs }),
	blockRemove: (objectId: string, blockId: string) =>
		mutate("block_remove", { object_id: objectId, block_id: blockId }),
	setField: (objectId: string, key: string, value: ValueJSON) =>
		mutate("set_field", { object_id: objectId, key, value }),
	deleteField: (objectId: string, key: string) =>
		mutate("delete_field", { object_id: objectId, key }),
	del: (objectId: string) => mutate("delete", { object_id: objectId }),
	/** Retype in place: history, blocks, and fields survive. */
	setType: (objectId: string, typeKey: string) => mutate("set_type", { object_id: objectId, type_key: typeKey }),
	vanish: (objectIds: string | string[]) =>
		mutate("vanish", Array.isArray(objectIds) ? { object_ids: objectIds } : { object_id: objectIds }),
};

export const table = {
	create: (objectId: string, targetId = "", position = 0, rows = 3, cols = 3) =>
		mutate("table_create", { object_id: objectId, target_id: targetId, position, rows, cols }) as Promise<{ id: string }>,
	rowAdd: (objectId: string, tableId: string) =>
		mutate("table_row_add", { object_id: objectId, table_id: tableId }),
	colAdd: (objectId: string, tableId: string) =>
		mutate("table_col_add", { object_id: objectId, table_id: tableId }),
	colRemove: (objectId: string, tableId: string, columnId: string) =>
		mutate("table_col_remove", { object_id: objectId, table_id: tableId, column_id: columnId }),
};

export const space = {
	create: (name: string, icon?: string) =>
		mutate("channel_create", { name, icon }) as Promise<{ id: string; key_id: number }>,
	memberAdd: (channelId: string, npub: string, role?: string) =>
		mutate("channel_member_add", { channel_id: channelId, npub, role }),
	memberRemove: (channelId: string, npub: string) =>
		mutate("channel_member_remove", { channel_id: channelId, npub }),
	keyRotate: (channelId: string) => mutate("channel_key_rotate", { channel_id: channelId }),
};

export interface NostrSettings {
	hasKey: boolean;
	relays: string[];
	authorId: string;
}

export const settings = {
	fetch: async (): Promise<NostrSettings> => {
		const key = loadKey();
		return { hasKey: !!key, relays: backend.relays(), authorId: key ? authorIdFor(key) : "" };
	},
	importKey: async (key: string) => {
		// The old identity's replica must not leak into the new one.
		await backend.logout();
		localStorage.removeItem("roostr-space-keys");
		localStorage.removeItem("roostr-profile");
		saveKey(key.trim());
		location.reload(); // fresh identity: restart the replica from zero
		return {};
	},
	logout: async () => {
		await backend.logout();
		localStorage.removeItem("roostr-space-keys");
		localStorage.removeItem("roostr-profile");
		clearKey();
		location.href = "/app";
	},
	exportKey: async (): Promise<{ nsec: string; hex: string }> => {
		const key = loadKey();
		if (!key) throw new Error("no key");
		return { nsec: nip19.nsecEncode(key.sk), hex: bytesToHex(key.sk) };
	},
	setRelays: async (relays: string[]) => {
		backend.setRelays(relays);
		return { relays };
	},
};

export const chat = {
	post: (objectId: string, text: string, replyTo = "") =>
		mutate("chat_post", { object_id: objectId, text, reply_to: replyTo }) as Promise<{ id: string }>,
	react: (objectId: string, messageId: string, emoji: string) =>
		mutate("chat_react", { object_id: objectId, message_id: messageId, emoji }),
};
