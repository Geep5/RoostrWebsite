<script lang="ts">
	/**
	 * Right-click menu for a record in a set/collection view: open it,
	 * retype it, add it to a collection, sever its collection membership,
	 * or bin it. Bulk by construction: a click inside the selection acts
	 * on the whole selection.
	 *
	 * Shared by every view that lists records — table, gallery, kanban —
	 * because the menu is a property of "a record in a view", not of the
	 * table that first grew one. Removing is offered only when the host
	 * passes `onremove`, i.e. only inside a collection, since a query has
	 * no membership to sever.
	 */
	import { note, fetchObject } from "$lib/api";
	import { tabs } from "$lib/tabs.svelte";
	import { store } from "$lib/data.svelte";
	import { objectIcon } from "$lib/icons";
	import { typeGlyph } from "$lib/create";
	import PropertyValue from "./PropertyValue.svelte";
	import { spaceRelations, RESERVED_KEYS } from "$lib/relations";
	import type { RelationDefJSON, ValueJSON } from "$lib/types";

	let {
		x,
		y,
		ids,
		spaceId = "",
		onremove,
		relationKeys,
		fieldsOf,
		onchanged,
		onclose,
	}: {
		x: number;
		y: number;
		/** The records this menu acts on — the selection when the click landed inside it. */
		ids: string[];
		/** Scopes the type / collection submenus to the view's space. */
		spaceId?: string;
		/** Collections only: drop these from the collection, never touch the objects. */
		onremove?: (ids: string[]) => Promise<void>;
		/** Anytype objectContext passes the view's visible relation keys; they head the
		 * Edit-properties list, the rest of the space's relations follow. */
		relationKeys?: string[];
		/** Row fields the host already holds (a table's rows). Without it, values are
		 * fetched per object when a bulk merge needs them. */
		fieldsOf?: (id: string) => Record<string, ValueJSON> | undefined;
		onchanged: () => Promise<void>;
		onclose: () => void;
	} = $props();

	const n = $derived(ids.length);
	const suffix = $derived(n > 1 ? ` (${n})` : "");

	let showTypes = $state(false);
	let showCols = $state(false);

	/** Space types plus bundled ones — the same set every type picker offers. */
	const types = $derived(store.types.filter((t) => !t.space || t.space === spaceId));
	const collections = $derived(
		store.summaries.filter((s) => s.typeKey === "collection" && s.channelId === spaceId && !ids.includes(s.id)),
	);

	/** Snapshot before closing: `ids` is a live prop, and closing clears
	 * the host's selection, so reading it afterwards yields nothing. */
	async function remove() {
		const targets = [...ids];
		onclose();
		await onremove?.(targets);
	}

	async function bin() {
		const targets = [...ids];
		onclose();
		for (const id of targets) await note.del(id);
		await onchanged();
	}

	/** Retype in place — blocks and fields survive, same as the object page. */
	async function retype(typeKey: string) {
		const targets = [...ids];
		onclose();
		for (const id of targets) await note.setType(id, typeKey);
		await onchanged();
	}

	// ── Edit properties (Anytype popup 'relation') ─────────────────
	// One property at a time through the same PropertyValue editors the
	// grid cells use. Anytype stages edits then commits on Save; ours
	// applies per pick but keeps the menu open, so several properties
	// can be set in one session. Semantics mirror theirs: scalars and
	// single-select overwrite on every record; multi-value lists (tag,
	// object links) MERGE - the initial value is the cross-record
	// intersection, and only the delta the user added/removed touches
	// each record, so untouched per-record values survive.
	let showProps = $state(false);
	let editKey = $state("");
	let propFilter = $state("");

	const propRels = $derived.by((): RelationDefJSON[] => {
		const all = spaceRelations(store.relations, spaceId).filter((r) => !r.hidden && !r.readOnly && !RESERVED_KEYS[r.key]);
		const head = (relationKeys ?? [])
			.map((k) => all.find((r) => r.key === k))
			.filter((r): r is RelationDefJSON => !!r);
		const rest = all.filter((r) => !head.includes(r));
		return [...head, ...rest];
	});
	const filteredRels = $derived.by(() => {
		const q = propFilter.trim().toLowerCase();
		return q ? propRels.filter((r) => (r.name || r.key).toLowerCase().includes(q)) : propRels;
	});
	const editRel = $derived(propRels.find((r) => r.key === editKey));

	/** Multi-value formats merge; everything else overwrites (their Select rule). */
	const MERGE_FORMATS: Record<string, true> = { tag: true, object: true };
	const isMerge = (rel: RelationDefJSON) => !!MERGE_FORMATS[rel.format];

	async function valuesOf(id: string): Promise<Record<string, ValueJSON> | undefined> {
		return fieldsOf?.(id) ?? (await fetchObject(id)).fields;
	}

	function listOf(fields: Record<string, ValueJSON> | undefined, key: string): string[] {
		return (fields?.[key]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean);
	}

	/** The cross-record intersection, or undefined when records differ / are unknown. */
	async function initialFor(key: string): Promise<ValueJSON | undefined> {
		const rel = propRels.find((r) => r.key === key);
		if (!rel) return undefined;
		const all = await Promise.all(ids.map(valuesOf));
		if (isMerge(rel)) {
			let inter = listOf(all[0], key);
			for (const f of all.slice(1)) inter = inter.filter((v) => listOf(f, key).includes(v));
			return { valuesValue: { items: inter.map((s) => ({ stringValue: s })) } };
		}
		const first = all[0]?.[key];
		for (const f of all.slice(1)) {
			if (JSON.stringify(f?.[key] ?? null) !== JSON.stringify(first ?? null)) return undefined;
		}
		return first;
	}
	let editValue = $state<ValueJSON | undefined>(undefined);

	async function openProp(key: string) {
		editKey = key;
		editValue = await initialFor(key);
	}

	async function applyProp(rel: RelationDefJSON, v: ValueJSON) {
		const targets = [...ids];
		if (isMerge(rel)) {
			const base = (editValue?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean);
			const next = (v.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean);
			const added = next.filter((x) => !base.includes(x));
			const removed = base.filter((x) => !next.includes(x));
			for (const id of targets) {
				const cur = listOf(await valuesOf(id), rel.key);
				const merged = [...cur.filter((x) => !removed.includes(x)), ...added.filter((x) => !cur.includes(x))];
				await note.setField(id, rel.key, { valuesValue: { items: merged.map((s) => ({ stringValue: s })) } });
			}
		} else {
			for (const id of targets) await note.setField(id, rel.key, v);
		}
		await onchanged();
		// Back to the property list; the menu stays open for the next one.
		editKey = "";
		editValue = undefined;
	}

	async function addTo(collectionId: string) {
		const targets = [...ids];
		onclose();
		const col = await fetchObject(collectionId);
		const items = col.fields["collectionIds"]?.valuesValue?.items ?? [];
		const have = items.map((i) => i.stringValue).filter((s): s is string => typeof s === "string");
		const merged = [...have, ...targets.filter((id) => !have.includes(id))];
		if (merged.length !== have.length) {
			await note.setField(collectionId, "collectionIds", {
				valuesValue: { items: merged.map((id) => ({ stringValue: id })) },
			});
		}
		await onchanged();
	}
</script>

<button
	class="ctx-backdrop"
	aria-label="Close menu"
	onclick={onclose}
	oncontextmenu={(e) => {
		e.preventDefault();
		onclose();
	}}
></button>
<div
	class="ctx-menu"
	style="left: {Math.min(x, window.innerWidth - 230)}px; top: {Math.min(y, window.innerHeight - 320)}px"
	role="menu"
>
	<button
		role="menuitem"
		onclick={() => {
			const targets = [...ids];
			onclose();
			// One object opens in place; a multi-selection opens Anytype-style:
			// each object becomes an in-app tab in the strip.
			if (targets.length === 1) {
				location.href = `/app/object/${targets[0]}`;
				return;
			}
			for (const id of targets) tabs.open(`/app/object/${id}`);
		}}>Open{suffix}</button
	>
	<button
		role="menuitem"
		onclick={() => {
			const targets = [...ids];
			onclose();
			for (const id of targets) tabs.open(`/app/object/${id}`);
		}}>Open in new tab{suffix}</button
	>
	<button role="menuitem" onclick={() => { showTypes = !showTypes; showCols = false; showProps = false; }}>⇄ Change type{suffix} ▸</button>
	{#if showTypes}
		<div class="ctx-sub">
			{#each types as t (t.id)}
				<button role="menuitem" onclick={() => void retype(t.key)}>{t.icon || typeGlyph(t.key)} {t.name || t.key}</button>
			{/each}
		</div>
	{/if}
	<!-- Anytype objectContext 'relation' item -> popup 'relation': edit one
	     property across every selected record. -->
	<button role="menuitem" onclick={() => { showProps = !showProps; showTypes = false; showCols = false; editKey = ""; }}>✎ Edit properties{suffix} ▸</button>
	{#if showProps}
		<div class="ctx-sub props">
			{#if editRel}
				<button role="menuitem" class="ctx-back" onclick={() => { editKey = ""; editValue = undefined; }}>← {editRel.iconEmoji ? editRel.iconEmoji + " " : ""}{editRel.name || editRel.key}</button>
				<div class="ctx-editor">
					<PropertyValue rel={editRel} value={editValue} onsave={(v) => void applyProp(editRel, v)} />
				</div>
			{:else}
				{#if propRels.length > 6}
					<input class="ctx-filter" placeholder="Filter properties..." bind:value={propFilter} />
				{/if}
				{#each filteredRels as r (r.key)}
					<button role="menuitem" onclick={() => void openProp(r.key)}>{r.iconEmoji ? r.iconEmoji + " " : ""}{r.name || r.key}</button>
				{/each}
				{#if filteredRels.length === 0}
					<span class="ctx-none">No properties</span>
				{/if}
			{/if}
		</div>
	{/if}
	<button role="menuitem" onclick={() => { showCols = !showCols; showTypes = false; showProps = false; }}>▣ Add to collection{suffix} ▸</button>
	{#if showCols}
		<div class="ctx-sub">
			{#each collections as c (c.id)}
				<button role="menuitem" onclick={() => void addTo(c.id)}>{objectIcon(c.icon, c.typeKey)} {c.name || "Untitled"}</button>
			{/each}
			{#if collections.length === 0}
				<span class="ctx-none">No collections in this space</span>
			{/if}
		</div>
	{/if}
	{#if onremove}
		<button role="menuitem" onclick={() => void remove()}>⊖ Remove from collection{suffix}</button>
	{/if}
	<div class="ctx-sep"></div>
	<button role="menuitem" class="danger" onclick={() => void bin()}>🗑 Move to bin{suffix}</button>
</div>

<style>
	.ctx-backdrop {
		position: fixed;
		inset: 0;
		z-index: 90;
		background: none;
		border: none;
		cursor: default;
	}
	.ctx-menu {
		position: fixed;
		z-index: 91;
		min-width: 190px;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 4px;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
		display: flex;
		flex-direction: column;
	}
	.ctx-menu button {
		display: block;
		width: 100%;
		text-align: left;
		background: none;
		border: none;
		color: var(--fg);
		font-size: 13px;
		padding: 7px 10px;
		border-radius: 6px;
		cursor: pointer;
	}
	.ctx-menu button:hover {
		background: var(--hover);
	}
	.ctx-menu .danger {
		color: var(--red);
	}
	.ctx-sep {
		height: 1px;
		background: var(--border);
		margin: 4px 6px;
	}
	.ctx-sub {
		display: flex;
		flex-direction: column;
		max-height: 220px;
		overflow-y: auto;
		margin: 0 0 2px;
		padding: 2px 0 2px 10px;
		border-left: 1px solid var(--border);
	}
	.ctx-sub.props {
		max-height: 320px;
		overflow-y: auto;
	}
	.ctx-filter {
		display: block;
		width: calc(100% - 12px);
		margin: 4px 6px 6px;
		padding: 5px 8px;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--fg);
		font-size: 13px;
		outline: none;
	}
	.ctx-back {
		font-weight: 600;
	}
	.ctx-editor {
		padding: 2px 6px 8px;
	}
	.ctx-none {
		color: var(--muted);
		font-size: 12px;
		padding: 4px 8px;
	}
</style>
