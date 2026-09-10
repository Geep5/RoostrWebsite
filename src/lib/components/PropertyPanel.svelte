<script lang="ts">
	/**
	 * Property (relation) object page body — Anytype's property view:
	 * every object currently holding a value for this property, with
	 * the value rendered in the relation's own format.
	 */
	import type { ObjectJSON, ValueJSON } from "$lib/types";
	import { fieldStr } from "$lib/types";
	import { fetchQuery, note, type QueryResultRow, fetchAllQuery, fetchObject } from "$lib/api";
	import { objectIcon } from "$lib/icons";
	import { store } from "$lib/data.svelte";
	import { currentSpaceId } from "$lib/relations";
	import { spaceRelations } from "$lib/relations";

	let { object }: { object: ObjectJSON } = $props();

	// ── Definition: what this property MEANS - agents read it. ──────
	let defDraft = $state("");
	$effect(() => {
		defDraft = fieldStr(object.fields, "description");
	});
	async function saveDef() {
		if (defDraft.trim() === fieldStr(object.fields, "description")) return;
		await note.setField(object.id, "description", { stringValue: defDraft.trim() });
	}

	const key = $derived(fieldStr(object.fields, "key"));
	const format = $derived(fieldStr(object.fields, "format") || "shorttext");

	// ── Object-format restriction (Anytype "Limit Object Types") ─────
	// relationFormatObjectTypes: type ids the picker limits candidates to.
	// Roostr extension Anytype does not have: object_source, a query or
	// collection whose MEMBERS are the only pickable objects (it wins over
	// the type list when set - "Active Publishers" should mean exactly that).
	const objectTypes = $derived((object.fields["object_types"]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean));
	const objectSource = $derived(fieldStr(object.fields, "object_source"));
	const sourceName = $derived(store.summaries.find((s) => s.id === objectSource)?.name || "");
	const typeNameOf = (id: string) => store.types.find((t) => t.id === id)?.name || id.slice(0, 8);
	const typeIconOf = (id: string) => store.types.find((t) => t.id === id)?.icon || "";

	let addingType = $state(false);
	let addingSource = $state(false);
	const spaceId = $derived(fieldStr(object.fields, "channel") || currentSpaceId());
	const addableTypes = $derived(store.types.filter((t) => (!t.space || t.space === spaceId) && !objectTypes.includes(t.id)));
	const sourceCandidates = $derived(
		store.summaries.filter((s) => (s.typeKey === "query" || s.typeKey === "collection") && (s.channelId === spaceId || !s.channelId) && s.id !== objectSource),
	);

	async function setObjectTypes(ids: string[]) {
		await note.setField(object.id, "object_types", { valuesValue: { items: ids.map((id) => ({ stringValue: id })) } });
	}
	async function addType(id: string) {
		addingType = false;
		await setObjectTypes([...objectTypes, id]);
	}
	async function removeType(id: string) {
		await setObjectTypes(objectTypes.filter((x) => x !== id));
	}
	async function setSource(id: string) {
		addingSource = false;
		if (id) await note.setField(object.id, "object_source", { stringValue: id });
		else await note.deleteField(object.id, "object_source");
	}

	let rows = $state<QueryResultRow[]>([]);
	let loaded = $state(false);

	async function load() {
		if (!key) return;
		// Spaces are self-contained: a stamped property lists its own
		// space's holders; a bundled (global) one lists the space you are
		// browsing from. The default space also owns unstamped objects.
		const own = fieldStr(object.fields, "channel") || currentSpaceId();
		const spaceFilter =
			own === (store.channels[0]?.id ?? "")
				? { key: "channel", condition: "in", value: [own, ""] }
				: { key: "channel", condition: "equal", value: own };
		const records = await fetchAllQuery({ filters: [{ key, condition: "notEmpty" }, spaceFilter] });
		// Definition objects (relations/types) carry system fields like
		// `key`/`format` themselves — keep the listing to real records.
		rows = records
			.filter((r) => r.id !== object.id && r.typeKey !== "relation" && r.typeKey !== "type")
			.toSorted((a, b) => b.updatedAt - a.updatedAt);
		loaded = true;
	}
	$effect(() => {
		void key;
		void load();
	});

	function fmt(v: ValueJSON | undefined): string {
		if (!v) return "";
		if (v.boolValue !== undefined) return v.boolValue ? "☑" : "☐";
		if (v.stringValue !== undefined) return v.stringValue;
		if (v.intValue !== undefined) return format === "date" ? new Date(v.intValue).toLocaleDateString() : String(v.intValue);
		if (v.floatValue !== undefined) return String(v.floatValue);
		if (v.valuesValue) return v.valuesValue.items.map((i) => i.stringValue ?? i.linkValue?.targetId.slice(0, 8) ?? "").filter(Boolean).join(", ");
		if (v.linkValue) return v.linkValue.targetId.slice(0, 8);
		if (v.listValue) return v.listValue.values.join(", ");
		return "";
	}
</script>

<div class="prop-panel">
	<textarea class="definition" placeholder="What is this property for? Agents read this." bind:value={defDraft} onblur={() => void saveDef()} rows="2"></textarea>
	<p class="meta">
		<span class="chip">{format}</span>
		{loaded ? `${rows.length} object${rows.length === 1 ? "" : "s"} with a value` : "Loading…"}
	</p>
	{#if format === "object"}
		<!-- Anytype relation settings: "Limit Object Types" (page/main/relation).
		     Plus the Roostr-only query/collection source. -->
		<div class="limit">
			<div class="limit-label">Limit object types</div>
			{#each objectTypes as tid (tid)}
				<div class="limit-row">
					<span>{typeIconOf(tid)} {typeNameOf(tid)}</span>
					<button class="x" title="Remove restriction" onclick={() => void removeType(tid)}>×</button>
				</div>
			{/each}
			{#if addingType}
				<div class="limit-pick">
					{#each addableTypes as t (t.id)}
						<button onclick={() => void addType(t.id)}>{t.icon || "□"} {t.name || t.key}</button>
					{/each}
					{#if addableTypes.length === 0}<span class="muted">No more types in this space</span>{/if}
					<button class="x" onclick={() => (addingType = false)}>Cancel</button>
				</div>
			{:else}
				<button class="limit-add" onclick={() => { addingType = true; addingSource = false; }}>＋ Add object type</button>
			{/if}
			{#if objectTypes.length === 0 && !objectSource}
				<div class="muted limit-note">Unrestricted - any object can be linked.</div>
			{/if}

			<div class="limit-label src">Limit to a query or collection</div>
			{#if objectSource}
				<div class="limit-row">
					<span>⛁ {sourceName || objectSource.slice(0, 8)}</span>
					<button class="x" title="Remove source" onclick={() => void setSource("")}>×</button>
				</div>
				<div class="muted limit-note">Only members of this {store.summaries.find((s) => s.id === objectSource)?.typeKey === "collection" ? "collection" : "query"} can be linked.</div>
			{:else if addingSource}
				<div class="limit-pick">
					{#each sourceCandidates as c (c.id)}
						<button onclick={() => void setSource(c.id)}>{objectIcon(c.icon, c.typeKey)} {c.name || "Untitled"} <span class="muted">{c.typeKey}</span></button>
					{/each}
					{#if sourceCandidates.length === 0}<span class="muted">No queries or collections in this space</span>{/if}
					<button class="x" onclick={() => (addingSource = false)}>Cancel</button>
				</div>
			{:else}
				<button class="limit-add" onclick={() => { addingSource = true; addingType = false; }}>＋ Choose query or collection</button>
			{/if}
		</div>
	{/if}
	{#if loaded && rows.length === 0}
		<p class="muted">No objects have a value for this property yet.</p>
	{:else if rows.length > 0}
		<table>
			<thead><tr><th>Name</th><th>Value</th><th>Type</th></tr></thead>
			<tbody>
				{#each rows as r (r.id)}
					<tr>
						<td><a href="/app/object/{r.id}"><span class="obj-icon">{objectIcon(r.fields["iconEmoji"]?.stringValue, r.typeKey)}</span> {r.name || fieldStr(r.fields, "name") || "Untitled"}</a></td>
						<td class="val">{fmt(r.fields[key])}</td>
						<td class="muted">{r.typeKey}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>

<style>
	.prop-panel {
		padding: 0 48px;
	}
	.meta {
		display: flex;
		align-items: center;
		gap: 10px;
		color: var(--muted);
		font-size: 13px;
	}
	.chip {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 2px 8px;
		font-size: 12px;
		color: var(--fg);
	}
	.limit {
		margin: 10px 0 4px;
		padding: 10px 12px;
		border: 1px solid var(--border);
		border-radius: 8px;
		max-width: 520px;
	}
	.limit-label {
		font-size: 12px;
		font-weight: 600;
		color: var(--muted);
		margin-bottom: 6px;
	}
	.limit-label.src {
		margin-top: 12px;
	}
	.limit-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px 0;
		font-size: 13px;
	}
	.limit-row .x {
		color: var(--muted);
	}
	.limit-add {
		color: var(--accent);
		font-size: 13px;
		padding: 4px 0;
	}
	.limit-pick {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		max-height: 200px;
		overflow-y: auto;
	}
	.limit-pick button {
		font-size: 13px;
		padding: 3px 6px;
		border-radius: 6px;
	}
	.limit-pick button:hover {
		background: var(--hl-light, rgba(255,255,255,0.06));
	}
	.limit-note {
		font-size: 12px;
		margin-top: 4px;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		margin-top: 8px;
	}
	th {
		text-align: left;
		font-size: 12px;
		font-weight: 500;
		color: var(--muted);
		padding: 6px 8px;
		border-bottom: 1px solid var(--border);
	}
	td {
		padding: 8px;
		border-bottom: 1px solid var(--border);
		font-size: 14px;
	}
	td a {
		color: var(--fg);
		text-decoration: none;
		font-weight: 500;
	}
	td a:hover {
		text-decoration: underline;
	}
	.val {
		color: var(--fg);
		max-width: 340px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.muted {
		color: var(--muted);
	}
	.definition {
		width: 100%;
		background: none;
		border: 1px solid transparent;
		border-radius: 8px;
		color: var(--fg);
		font: inherit;
		font-size: 13.5px;
		padding: 6px 8px;
		margin: 0 0 8px -8px;
		resize: vertical;
	}
	.definition:hover {
		border-color: var(--border);
	}
	.definition:focus {
		border-color: var(--accent);
		outline: none;
	}
</style>
