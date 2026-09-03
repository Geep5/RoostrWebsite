/**
 * /api/mutate as local Change builders — a faithful port of the Odin
 * server's mutate.odin op construction. Each action builds one Change
 * (parents = current DAG heads of the object), commits it to the store,
 * and hands the bytes to the sync layer for relay publish.
 *
 * Channel management (create/member/invite/rotate) and nostr_* actions are
 * handled by the web backend directly or unsupported in v1.
 */

import type { ChangeJSON, OpJSON, BlockWire } from "./contracts";
import type { ValueJSON, BlockJSON } from "$lib/types";
import { spaceKeyGet, spaceKeyRotate, spaceKeyEnsure } from "./spacekeys";

export interface MutateCtx {
	/** This device's author id: hex(sha256(privkey_hex))[0..16]. */
	author: string;
	/** All known changes for the object (for heads); [] for new objects. */
	changesFor(objectId: string): Promise<ChangeJSON[]>;
	/** Latest computed state (for table shapes / chat meta / field reads). */
	getObject(objectId: string): Promise<{ blocks: BlockJSON[]; fields?: Record<string, ValueJSON> } | null>;
	/** Encode + id + persist + publish. */
	commit(change: ChangeJSON): Promise<string>;
}

function uuid(): string {
	return crypto.randomUUID();
}

/** Head change ids: changes no other change lists as a parent. */
export function headsOf(changes: ChangeJSON[]): string[] {
	const referenced = new Set<string>();
	for (const c of changes) for (const p of c.parentIds) referenced.add(p);
	return changes.filter((c) => !referenced.has(c.id)).map((c) => c.id).sort();
}

const POS_INNER = 5;

async function commitOps(ctx: MutateCtx, objectId: string, ops: OpJSON[]): Promise<void> {
	const changes = await ctx.changesFor(objectId);
	const change: ChangeJSON = {
		id: "",
		objectId,
		parentIds: headsOf(changes),
		ops,
		timestamp: Date.now(),
		author: ctx.author,
	};
	await ctx.commit(change);
}

function blockWire(b: Partial<BlockJSON>): BlockWire {
	return {
		id: b.id ?? uuid(),
		childrenIds: b.childrenIds ?? [],
		content: b.content ?? { text: { text: "", style: 0 } },
		...(b.align !== undefined ? { align: b.align } : {}),
		...(b.backgroundColor !== undefined ? { backgroundColor: b.backgroundColor } : {}),
	};
}

/** Table subtree shape (mutate.odin table_shape). */
function tableShape(blocks: BlockJSON[], tableId: string) {
	const byId = new Map(blocks.map((b) => [b.id, b]));
	const table = byId.get(tableId);
	if (!table?.content || !("table" in (table.content as object))) return null;
	let colsLayout = "";
	let rowsLayout = "";
	const colIds: string[] = [];
	const rowIds: string[] = [];
	for (const cid of table.childrenIds) {
		const c = byId.get(cid);
		const style = (c?.content as { layout?: { style?: number } })?.layout?.style;
		if (style === 4) {
			colsLayout = cid;
			for (const k of c!.childrenIds) colIds.push(k);
		} else if (style === 5) {
			rowsLayout = cid;
			for (const k of c!.childrenIds) rowIds.push(k);
		}
	}
	if (!colsLayout || !rowsLayout) return null;
	return { colsLayout, rowsLayout, colIds, rowIds };
}

function cellOps(rowId: string, colIds: string[]): OpJSON[] {
	return colIds.map((cid) => ({
		blockAdd: {
			block: { id: `${rowId}-${cid}`, childrenIds: [], content: { text: { text: "", style: 0 } } },
			targetId: rowId,
			position: POS_INNER,
		},
	}));
}

/** Run one mutate action; returns the response extra (e.g. {id}). */
/** Root block every message thread hangs under (chat + object discussions). */
const DISCUSSION_ID = "__discussion__";

// ── Space default definitions ────────────────────────────────────
//
// There are no global definitions: every space owns its OWN copies of
// the default relations and types (ids deterministic per key+space so
// devices converge). Mirrors the Odin daemon's catalog exactly.

const SPACE_DEFAULT_RELATIONS = [
	{ key: "name", format: "shorttext", name: "Name", emoji: "✏️", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "description", format: "longtext", name: "Description", emoji: "📝", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "iconEmoji", format: "emoji", name: "Icon", emoji: "🖼️", hidden: true, readOnly: false, maxCount: 0 },
	{ key: "createdDate", format: "date", name: "Created date", emoji: "📅", hidden: false, readOnly: true, maxCount: 0 },
	{ key: "modifiedDate", format: "date", name: "Modified date", emoji: "🗓️", hidden: false, readOnly: true, maxCount: 0 },
	{ key: "dueDate", format: "date", name: "Due date", emoji: "⏰", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "tag", format: "tag", name: "Tag", emoji: "🏷️", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "status", format: "status", name: "Status", emoji: "🚦", hidden: false, readOnly: false, maxCount: 1 },
	{ key: "done", format: "checkbox", name: "Done", emoji: "✅", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "url", format: "url", name: "URL", emoji: "🔗", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "email", format: "email", name: "Email", emoji: "✉️", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "phone", format: "phone", name: "Phone", emoji: "📞", hidden: false, readOnly: false, maxCount: 0 },
	{ key: "featuredRelations", format: "relations", name: "Featured relations", emoji: "⭐", hidden: true, readOnly: false, maxCount: 0 },
	{ key: "setOf", format: "object", name: "Set of", emoji: "🗂️", hidden: true, readOnly: false, maxCount: 0 },
] as const;

const SPACE_DEFAULT_TYPES = [
	{ key: "page", name: "Page", emoji: "📄", layout: "page" },
	{ key: "note", name: "Note", emoji: "📝", layout: "page" },
	{ key: "task", name: "Task", emoji: "✅", layout: "task" },
	{ key: "person", name: "Human", emoji: "👤", layout: "page" },
	{ key: "project", name: "Project", emoji: "🔨", layout: "page" },
	{ key: "bookmark", name: "Bookmark", emoji: "🔖", layout: "page" },
	{ key: "chat", name: "Chat", emoji: "💬", layout: "chat" },
] as const;

async function seedSpaceDefs(ctx: MutateCtx, spaceId: string): Promise<void> {
	const prefix = spaceId.slice(0, 8);
	for (const r of SPACE_DEFAULT_RELATIONS) {
		await commitOps(ctx, `bundled-rel-${r.key}-${prefix}`, [
			{ objectCreate: { typeKey: "relation" } },
			{ fieldSet: { key: "channel", value: { stringValue: spaceId } } },
			{ fieldSet: { key: "key", value: { stringValue: r.key } } },
			{ fieldSet: { key: "format", value: { stringValue: r.format } } },
			{ fieldSet: { key: "name", value: { stringValue: r.name } } },
			{ fieldSet: { key: "iconEmoji", value: { stringValue: r.emoji } } },
			{ fieldSet: { key: "hidden", value: { boolValue: r.hidden } } },
			{ fieldSet: { key: "readOnly", value: { boolValue: r.readOnly } } },
			{ fieldSet: { key: "maxCount", value: { intValue: r.maxCount } } },
			{ fieldSet: { key: "bundled", value: { boolValue: true } } },
			{ fieldSet: { key: "options", value: { valuesValue: { items: [] } } } },
		]);
	}
	for (const t of SPACE_DEFAULT_TYPES) {
		await commitOps(ctx, `bundled-type-${t.key}-${prefix}`, [
			{ objectCreate: { typeKey: "type" } },
			{ fieldSet: { key: "channel", value: { stringValue: spaceId } } },
			{ fieldSet: { key: "key", value: { stringValue: t.key } } },
			{ fieldSet: { key: "name", value: { stringValue: t.name } } },
			{ fieldSet: { key: "iconEmoji", value: { stringValue: t.emoji } } },
			{ fieldSet: { key: "layout", value: { stringValue: t.layout } } },
			{ fieldSet: { key: "bundled", value: { boolValue: true } } },
		]);
	}
}

export async function runMutation(
	ctx: MutateCtx,
	action: string,
	params: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	const str = (k: string) => (typeof params[k] === "string" ? (params[k] as string) : "");
	const num = (k: string) => (typeof params[k] === "number" ? (params[k] as number) : undefined);

	switch (action) {
		case "create": {
			const typeKey = str("type_key") || "note";
			const id = uuid();
			const ops: OpJSON[] = [
				{ objectCreate: { typeKey } },
				{ fieldSet: { key: "name", value: { stringValue: str("name") } } },
			];
			const fields = params["fields"] as Record<string, ValueJSON> | undefined;
			if (fields) for (const [k, v] of Object.entries(fields)) ops.push({ fieldSet: { key: k, value: v } });
			await commitOps(ctx, id, ops);
			return { id };
		}

		case "block_add": {
			const objectId = str("object_id");
			const block = blockWire((params["block"] ?? {}) as Partial<BlockJSON>);
			const targetId = str("target_id");
			// An unknown insert target degrades to a root block rather than
			// being lost, so adding into a discussion that does not exist yet
			// silently orphans the block: present in the object, absent from
			// the thread. chat_post has always guarded this with an idempotent
			// root add (replay skips it once the id exists); do the same here.
			const ops: OpJSON[] = targetId === DISCUSSION_ID
				? [
						{
							blockAdd: {
								block: { id: DISCUSSION_ID, childrenIds: [], content: { custom: { contentType: "discussion", meta: {} } } },
								targetId: "",
								position: 0,
							},
						},
					]
				: [];
			ops.push({ blockAdd: { block, targetId, position: num("position") ?? 0 } });
			await commitOps(ctx, objectId, ops);
			return {};
		}

		case "block_update": {
			await commitOps(ctx, str("object_id"), [
				{ blockUpdate: { blockId: str("block_id"), content: params["content"] } },
			]);
			return {};
		}

		case "block_move": {
			await commitOps(ctx, str("object_id"), [
				{ blockMove: { blockId: str("block_id"), targetId: str("target_id"), position: num("position") ?? 0 } },
			]);
			return {};
		}

		case "block_remove": {
			await commitOps(ctx, str("object_id"), [{ blockRemove: { blockId: str("block_id") } }]);
			return {};
		}

		case "block_set_attrs": {
			const ops: OpJSON[] = [];
			const blockId = str("block_id");
			if (num("align") !== undefined) ops.push({ blockSetAlign: { blockId, align: num("align")! } });
			if (typeof params["background_color"] === "string")
				ops.push({ blockSetBackground: { blockId, color: params["background_color"] as string } });
			if (ops.length === 0) throw new Error("at least one attr required");
			await commitOps(ctx, str("object_id"), ops);
			return {};
		}

		case "set_field": {
			await commitOps(ctx, str("object_id"), [
				{ fieldSet: { key: str("key"), value: params["value"] as ValueJSON } },
			]);
			return {};
		}

		case "delete_field": {
			await commitOps(ctx, str("object_id"), [{ fieldDelete: { key: str("key") } }]);
			return {};
		}

		case "delete": {
			await commitOps(ctx, str("object_id"), [{ objectDelete: {} }]);
			return {};
		}

		// Real deletion: record object ids in the synced vanish ledger
		// (object __vanished__, fields vanished:<id>) and tombstone locally.
		// Devices enforce the ledger on load; the desktop purges the files.
		case "vanish": {
			const ids: string[] = [];
			const single = str("object_id");
			if (single) ids.push(single);
			const arr = params["object_ids"];
			if (Array.isArray(arr)) for (const v of arr) if (typeof v === "string") ids.push(v);
			if (ids.length === 0) throw new Error("object_id or object_ids required");
			if (ids.includes("__vanished__")) throw new Error("the vanish ledger cannot be vanished");
			for (const id of ids) {
				await commitOps(ctx, id, [{ objectDelete: {} }]);
				await commitOps(ctx, "__vanished__", [
					{ fieldSet: { key: `vanished:${id}`, value: { boolValue: true } } },
				]);
			}
			return { vanished: ids.length };
		}

		case "table_create": {
			const objectId = str("object_id");
			const rows = Math.max(1, num("rows") ?? 3);
			const cols = Math.max(1, num("cols") ?? 3);
			const tid = uuid();
			const ops: OpJSON[] = [
				{ blockAdd: { block: { id: tid, childrenIds: [], content: { table: {} } }, targetId: str("target_id"), position: num("position") ?? 0 } },
			];
			const colsLayout = uuid();
			ops.push({ blockAdd: { block: { id: colsLayout, childrenIds: [], content: { layout: { style: 4 } } }, targetId: tid, position: POS_INNER } });
			const colIds: string[] = [];
			for (let i = 0; i < cols; i++) {
				const cid = uuid();
				colIds.push(cid);
				ops.push({ blockAdd: { block: { id: cid, childrenIds: [], content: { tableColumn: {} } }, targetId: colsLayout, position: POS_INNER } });
			}
			const rowsLayout = uuid();
			ops.push({ blockAdd: { block: { id: rowsLayout, childrenIds: [], content: { layout: { style: 5 } } }, targetId: tid, position: POS_INNER } });
			for (let r = 0; r < rows; r++) {
				const rid = uuid();
				ops.push({ blockAdd: { block: { id: rid, childrenIds: [], content: { tableRow: {} } }, targetId: rowsLayout, position: POS_INNER } });
				ops.push(...cellOps(rid, colIds));
			}
			await commitOps(ctx, objectId, ops);
			return { id: tid };
		}

		case "table_row_add": {
			const objectId = str("object_id");
			const obj = await ctx.getObject(objectId);
			const shape = obj && tableShape(obj.blocks, str("table_id"));
			if (!shape) throw new Error("table not found");
			const rid = uuid();
			const ops: OpJSON[] = [
				{ blockAdd: { block: { id: rid, childrenIds: [], content: { tableRow: {} } }, targetId: shape.rowsLayout, position: POS_INNER } },
				...cellOps(rid, shape.colIds),
			];
			await commitOps(ctx, objectId, ops);
			return {};
		}

		case "table_col_add": {
			const objectId = str("object_id");
			const obj = await ctx.getObject(objectId);
			const shape = obj && tableShape(obj.blocks, str("table_id"));
			if (!shape) throw new Error("table not found");
			const cid = uuid();
			const ops: OpJSON[] = [
				{ blockAdd: { block: { id: cid, childrenIds: [], content: { tableColumn: {} } }, targetId: shape.colsLayout, position: POS_INNER } },
				...shape.rowIds.map((rid) => ({
					blockAdd: { block: { id: `${rid}-${cid}`, childrenIds: [], content: { text: { text: "", style: 0 } } }, targetId: rid, position: POS_INNER },
				})),
			];
			await commitOps(ctx, objectId, ops);
			return {};
		}

		case "table_col_remove": {
			const objectId = str("object_id");
			const columnId = str("column_id");
			const obj = await ctx.getObject(objectId);
			const shape = obj && tableShape(obj.blocks, str("table_id"));
			if (!shape || !columnId) throw new Error("table not found");
			const ops: OpJSON[] = [
				{ blockRemove: { blockId: columnId } },
				...shape.rowIds.map((rid) => ({ blockRemove: { blockId: `${rid}-${columnId}` } })),
			];
			await commitOps(ctx, objectId, ops);
			return {};
		}

		case "chat_post": {
			const objectId = str("object_id");
			const text = str("text");
			if (!objectId || !text) throw new Error("object_id and text required");
			const meta: Record<string, string> = {
				author: str("as_author") || ctx.author,
				ts: String(Date.now()),
				text,
			};
			if (str("reply_to")) meta["replyTo"] = str("reply_to");
			const mid = uuid();
			await commitOps(ctx, objectId, [
				{
					blockAdd: {
						block: { id: DISCUSSION_ID, childrenIds: [], content: { custom: { contentType: "discussion", meta: {} } } },
						targetId: "",
						position: 0,
					},
				},
				{
					blockAdd: {
						block: { id: mid, childrenIds: [], content: { custom: { contentType: "chat", meta } } },
						targetId: DISCUSSION_ID,
						position: POS_INNER,
					},
				},
			]);
			return { id: mid };
		}

		case "chat_react": {
			const objectId = str("object_id");
			const messageId = str("message_id");
			const emoji = str("emoji");
			const obj = await ctx.getObject(objectId);
			const msg = obj?.blocks.find((b) => b.id === messageId);
			const meta = (msg?.content as { custom?: { meta?: Record<string, string> } })?.custom?.meta;
			if (!meta) throw new Error("message not found");
			// "emoji|a1,a2;emoji|a1" toggle - port of mutate.odin chat_react.
			const entries: Array<{ emoji: string; authors: string[] }> = [];
			for (const chunk of (meta["reactions"] ?? "").split(";")) {
				const bar = chunk.indexOf("|");
				if (bar <= 0) continue;
				entries.push({ emoji: chunk.slice(0, bar), authors: chunk.slice(bar + 1).split(",").filter(Boolean) });
			}
			const me = ctx.author;
			let found = false;
			for (const e of entries) {
				if (e.emoji !== emoji) continue;
				found = true;
				const had = e.authors.includes(me);
				e.authors = had ? e.authors.filter((a) => a !== me) : [...e.authors, me];
			}
			if (!found) entries.push({ emoji, authors: [me] });
			const newMeta: Record<string, string> = {};
			for (const [k, v] of Object.entries(meta)) if (k !== "reactions") newMeta[k] = v;
			const chunks = entries.filter((e) => e.authors.length > 0).map((e) => `${e.emoji}|${e.authors.join(",")}`);
			if (chunks.length > 0) newMeta["reactions"] = chunks.join(";");
			await commitOps(ctx, objectId, [
				{ blockUpdate: { blockId: messageId, content: { custom: { contentType: "chat", meta: newMeta } } } },
			]);
			return {};
		}

		case "channel_create": {
			const name = str("name");
			if (!name) throw new Error("name required");
			const id = uuid();
			await commitOps(ctx, id, [
				{ objectCreate: { typeKey: "channel" } },
				{ fieldSet: { key: "name", value: { stringValue: name } } },
				{ fieldSet: { key: "iconEmoji", value: { stringValue: str("icon") } } },
				{ fieldSet: { key: "pinnedIds", value: { valuesValue: { items: [] } } } },
				{ fieldSet: { key: "members", value: { valuesValue: { items: [] } } } },
				{ fieldSet: { key: "keyId", value: { intValue: 1 } } },
			]);
			spaceKeyEnsure(id);
			await seedSpaceDefs(ctx, id);
			return { id, key_id: 1 };
		}

		// Ports of the Odin daemon's channel member ops (mutate.odin):
		// members is a list of {npub, role} maps on the channel object.
		case "channel_member_add": {
			const channelId = str("channel_id");
			const npub = str("npub");
			if (!channelId || !npub) throw new Error("channel_id and npub required");
			const role = str("role") || "writer";
			const obj = await ctx.getObject(channelId);
			const items = obj?.fields?.["members"]?.valuesValue?.items ?? [];
			if (!items.some((i) => i.mapValue?.entries?.["npub"]?.stringValue === npub)) {
				const entry: ValueJSON = { mapValue: { entries: { npub: { stringValue: npub }, role: { stringValue: role } } } };
				await commitOps(ctx, channelId, [
					{ fieldSet: { key: "members", value: { valuesValue: { items: [...items, entry] } } } },
				]);
			}
			return {};
		}

		// Removal rotates the space key so the removed member loses future access.
		case "channel_member_remove": {
			const channelId = str("channel_id");
			const npub = str("npub");
			if (!channelId || !npub) throw new Error("channel_id and npub required");
			const obj = await ctx.getObject(channelId);
			const items = obj?.fields?.["members"]?.valuesValue?.items ?? [];
			const kept = items.filter((i) => i.mapValue?.entries?.["npub"]?.stringValue !== npub);
			const { keyId } = spaceKeyRotate(channelId);
			await commitOps(ctx, channelId, [
				{ fieldSet: { key: "members", value: { valuesValue: { items: kept } } } },
				{ fieldSet: { key: "keyId", value: { intValue: keyId } } },
			]);
			return { key_id: keyId };
		}

		case "channel_invite_payload": {
			const channelId = str("channel_id");
			if (!channelId) throw new Error("channel_id required");
			const entry = spaceKeyGet(channelId);
			if (!entry) throw new Error("no local key for channel");
			const obj = await ctx.getObject(channelId);
			const name = obj?.fields?.["name"]?.stringValue ?? "";
			return { payload: { channel_id: channelId, name, key: entry.key, key_id: entry.keyId } };
		}

		default:
			throw new Error(`unsupported action on Roostr Web: ${action}`);
	}
}
