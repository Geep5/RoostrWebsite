<script lang="ts">
	/**
	 * The line under an object's title: what type it is (opens the type) and
	 * how many objects link here (opens the list). Properties live in the
	 * right pane; these two are facts about the object, not settings on it.
	 * A query also names what it queries (Anytype's "Object Type: X" chip):
	 * its source types, each removable from the chip's card.
	 */
	import type { ObjectJSON } from "$lib/types";
	import { store } from "$lib/data.svelte";
	import { note } from "$lib/api";
	import { badgeStyle } from "$lib/options";
	import { typeGlyph } from "$lib/create";
	import PropIcon from "./PropIcon.svelte";
	import { TYPE_GLYPHS } from "$lib/icons";
	import { fetchBacklinks, type Backlink } from "$lib/backlinks";
	import TypeSuggest from "./TypeSuggest.svelte";

	let { object }: { object: ObjectJSON } = $props();

	const typeDef = $derived(store.types.find((t) => t.key === object.typeKey));
	/** The chip's icon: the type's own, else the built-in glyph for kinds with no type object (collection, query). */
	const typeIcon = $derived(typeDef?.icon || TYPE_GLYPHS[object.typeKey] || "");
	// A space's type (`channel`) is engine infrastructure with no type page; people call it a Space.
	// Built-in kinds with no type object read as words, like the table's type column: `query` → "Query".
	const typeName = $derived(
		typeDef?.name || (object.typeKey === "channel" ? "Space" : (object.typeKey.charAt(0).toUpperCase() + object.typeKey.slice(1)).replaceAll("_", " ")),
	);

	/** A query's source types (`setOf` keys), resolved to this space's copy of each type. */
	const sourceKeys = $derived(
		object.typeKey === "query" ? (object.fields["setOf"]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean) : [],
	);
	const sources = $derived.by(() => {
		const space = object.fields["channel"]?.stringValue ?? "";
		return sourceKeys.map((key) => {
			const matches = store.types.filter((t) => t.key === key);
			const t = matches.find((x) => x.space === space) ?? matches[0];
			return { key, id: t?.id ?? "", name: t?.name || key, icon: t?.icon || typeGlyph(key) };
		});
	});
	const isQuery = $derived(object.typeKey === "query");
	function saveSources(next: string[]) {
		return note.setField(object.id, "setOf", { valuesValue: { items: next.map((k) => ({ stringValue: k })) } });
	}

	let backlinks = $state<Backlink[]>([]);
	let open = $state<"" | "sources" | "backlinks">("");
	$effect(() => {
		const id = object.id;
		// A query with no type yet shows nothing: open its type card so the
		// first thing asked is what to query.
		open = object.typeKey === "query" && sourceKeys.length === 0 ? "sources" : "";
		backlinks = [];
		void fetchBacklinks(id).then((b) => {
			if (object.id === id) backlinks = b;
		});
	});
	const toggle = (which: "sources" | "backlinks") => (open = open === which ? "" : which);
</script>

<div class="meta">
	{#if typeDef}
		<a class="badge" style={badgeStyle("")} title="Type" href="/app/object/{typeDef.id}">
			{#if typeIcon}<span class="emoji">{typeIcon}</span>{:else}<PropIcon icon="dot" />{/if}{typeName}
		</a>
	{:else}
		<span class="badge" style={badgeStyle("")} title="Type">{#if typeIcon}<span class="emoji">{typeIcon}</span>{:else}<PropIcon icon="dot" />{/if}{typeName}</span>
	{/if}
	{#if isQuery}
		<span class="wrap">
			<button class="badge" style={badgeStyle("")} title="What this query shows" onclick={() => toggle("sources")}>
				{#if sources.length}
					<span class="emoji">{sources[0].icon}</span>Object {sources.length === 1 ? "Type" : "Types"}: {sources.map((s) => s.name).join(", ")}
				{:else}
					<PropIcon icon="dot" />Choose an object type
				{/if}
			</button>
			{#if open === "sources"}
				<div class="pop">
					{#each sources as s (s.key)}
						<div class="source">
							{#if s.id}
								<a class="source-main" href="/app/object/{s.id}" onclick={() => (open = "")}>
									<span class="source-ico">{s.icon}</span>
									<span class="source-text"><span class="source-label">Object Type</span><span class="source-name">{s.name}</span></span>
								</a>
							{:else}
								<span class="source-main">
									<span class="source-ico">{s.icon}</span>
									<span class="source-text"><span class="source-label">Object Type</span><span class="source-name">{s.name}</span></span>
								</span>
							{/if}
							<button class="source-rm" aria-label={`Stop querying ${s.name}`} title="Remove" onclick={() => void saveSources(sourceKeys.filter((k) => k !== s.key))}>×</button>
						</div>
					{/each}
					<TypeSuggest
						exclude={sourceKeys}
						placeholder={sources.length ? "Add another type…" : "Search types… (e.g. p → Person, Project)"}
						onpick={(key) => void saveSources([...sourceKeys, key])}
						onclose={() => (open = "")}
					/>
				</div>
			{/if}
		</span>
	{/if}
	{#if backlinks.length > 0}
		<span class="wrap">
			<button class="badge" style={badgeStyle("")} title="Backlinks" onclick={() => toggle("backlinks")}>
				<PropIcon icon="link" />{backlinks.length} backlink{backlinks.length === 1 ? "" : "s"}
			</button>
			{#if open === "backlinks"}
				<div class="pop">
					<div class="pop-name">Linked from</div>
					{#each backlinks as b (b.id)}
						<a class="backlink" href="/app/object/{b.id}" onclick={() => (open = "")}>
							<span class="bl-icon">{b.icon || "▨"}</span>{b.name}
							<span class="bl-kind">{b.typeKey}</span>
						</a>
					{/each}
				</div>
			{/if}
		</span>
	{/if}
</div>
{#if open}
	<button class="backdrop" aria-label="Close" onclick={() => (open = "")}></button>
{/if}

<style>
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 2px 0 14px 48px;
		font-size: 13px;
	}
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		padding: 0 10px 0 6px;
		border: none;
		border-radius: 8px;
		background: var(--badge-bg);
		color: var(--badge-fg);
		font: inherit;
		font-size: 13px;
		font-weight: 500;
		line-height: 1;
		white-space: nowrap;
		text-decoration: none;
		cursor: pointer;
		transition: filter 120ms;
	}
	.badge:hover {
		filter: brightness(1.18);
	}
	span.badge {
		cursor: default;
	}
	.emoji {
		font-size: 14px;
		line-height: 1;
		width: 16px;
		text-align: center;
	}
	.wrap {
		position: relative;
		display: inline-flex;
	}
	.pop {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 90;
		min-width: 280px;
		max-width: 380px;
		max-height: 360px;
		overflow-y: auto;
		background: var(--panel, #1a1d23);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.pop-name {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
		margin-bottom: 4px;
	}
	.backlink {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 5px 8px;
		border-radius: 6px;
		color: var(--fg);
		text-decoration: none;
		font-size: 13px;
	}
	.backlink:hover {
		background: var(--hover);
	}
	.bl-icon {
		flex: none;
		width: 18px;
		text-align: center;
	}
	.bl-kind {
		margin-left: auto;
		color: var(--muted);
		font-size: 11px;
	}
	/* Anytype's source card: a large icon tile, "Object Type" over the name, × on the right. */
	.source {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px;
		border-radius: 8px;
		background: var(--hl-med);
	}
	.source-main {
		display: flex;
		align-items: center;
		gap: 12px;
		flex: 1;
		min-width: 0;
		color: var(--fg);
		text-decoration: none;
	}
	.source-ico {
		flex: none;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		height: 40px;
		border-radius: 8px;
		background: var(--hover);
		font-size: 20px;
	}
	.source-text {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.source-label {
		font-size: 14px;
		color: var(--fg);
	}
	.source-name {
		font-size: 13px;
		color: var(--muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.source-rm {
		flex: none;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: 6px;
		background: none;
		color: var(--muted);
		font-size: 18px;
		cursor: pointer;
	}
	.source-rm:hover {
		background: var(--hover);
		color: var(--fg);
	}
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 80;
		background: none;
		border: none;
		cursor: default;
	}
	@media (max-width: 720px) {
		.meta {
			margin: 0 16px 10px;
		}
	}
</style>
