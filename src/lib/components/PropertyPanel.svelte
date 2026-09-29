<script lang="ts">
	/**
	 * Property (relation) object page body — Anytype's property view:
	 * every object currently holding a value for this property, with
	 * the value rendered in the relation's own format.
	 */
	import type { ObjectJSON, ValueJSON } from "$lib/types";
	import { fieldStr } from "$lib/types";
	import { note, type QueryResultRow, fetchAllQuery } from "$lib/api";
	import { objectIcon } from "$lib/icons";
	import { store } from "$lib/data.svelte";
	import { currentSpaceId } from "$lib/relations";

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

	/** Object ids a link value names (a link, or a list of links). */
	function linkedIds(v: ValueJSON | undefined): string[] {
		if (!v) return [];
		if (v.linkValue?.targetId) return [v.linkValue.targetId];
		return (v.valuesValue?.items ?? []).map((i) => i.linkValue?.targetId ?? "").filter(Boolean);
	}
	function linkedObject(id: string): { name: string; icon?: string; typeKey: string } | undefined {
		const s = store.summaries.find((x) => x.id === id);
		if (s) return s;
		const a = store.agents.find((x) => x.id === id);
		return a ? { name: a.name, icon: a.icon, typeKey: "agent" } : undefined;
	}

	function fmt(v: ValueJSON | undefined): string {
		if (!v || format === "repeat") return "";
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
	{#if loaded && rows.length === 0}
		<p class="muted">No objects have a value for this property yet.</p>
	{:else if rows.length > 0}
		<table>
			<thead><tr><th>Name</th><th>Value</th><th>Type</th></tr></thead>
			<tbody>
				{#each rows as r (r.id)}
					<tr>
						<td><a href="/app/object/{r.id}"><span class="obj-icon">{objectIcon(r.fields["iconEmoji"]?.stringValue, r.typeKey)}</span> {r.name || fieldStr(r.fields, "name") || "Untitled"}</a></td>
						<td class="val">
							{#if linkedIds(r.fields[key]).length > 0}
								{#each linkedIds(r.fields[key]) as id (id)}
									{@const o = linkedObject(id)}
									<a class="linked" href="/app/object/{id}"><span class="obj-icon">{objectIcon(o?.icon, o?.typeKey ?? "")}</span>{o?.name || "Untitled"}</a>
								{/each}
							{:else}
								{fmt(r.fields[key])}
							{/if}
						</td>
						<td class="muted">{r.typeKey}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>

<style>
	.linked {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-right: 10px;
		color: var(--fg);
		text-decoration: none;
	}
	.linked:hover {
		text-decoration: underline;
	}
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
