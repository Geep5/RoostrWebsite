<script lang="ts">
	/**
	 * The line under an object's title: what type it is (opens the type) and
	 * how many objects link here (opens the list). Properties live in the
	 * right pane; these two are facts about the object, not settings on it.
	 */
	import type { ObjectJSON } from "$lib/types";
	import { store } from "$lib/data.svelte";
	import { badgeStyle } from "$lib/options";
	import PropIcon from "./PropIcon.svelte";
	import { fetchBacklinks, type Backlink } from "$lib/backlinks";

	let { object }: { object: ObjectJSON } = $props();

	const typeDef = $derived(store.types.find((t) => t.key === object.typeKey));
	// A space's type (`channel`) is engine infrastructure with no type page; people call it a Space.
	const typeName = $derived(typeDef?.name || (object.typeKey === "channel" ? "Space" : object.typeKey));

	let backlinks = $state<Backlink[]>([]);
	let open = $state(false);
	$effect(() => {
		const id = object.id;
		open = false;
		backlinks = [];
		void fetchBacklinks(id).then((b) => {
			if (object.id === id) backlinks = b;
		});
	});
</script>

<div class="meta">
	{#if typeDef}
		<a class="badge" style={badgeStyle("")} title="Type" href="/app/object/{typeDef.id}">
			{#if typeDef.icon}<span class="emoji">{typeDef.icon}</span>{:else}<PropIcon icon="dot" />{/if}{typeName}
		</a>
	{:else}
		<span class="badge" style={badgeStyle("")} title="Type"><PropIcon icon="dot" />{typeName}</span>
	{/if}
	{#if backlinks.length > 0}
		<span class="wrap">
			<button class="badge" style={badgeStyle("")} title="Backlinks" onclick={() => (open = !open)}>
				<PropIcon icon="link" />{backlinks.length} backlink{backlinks.length === 1 ? "" : "s"}
			</button>
			{#if open}
				<div class="pop">
					<div class="pop-name">Linked from</div>
					{#each backlinks as b (b.id)}
						<a class="backlink" href="/app/object/{b.id}" onclick={() => (open = false)}>
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
	<button class="backdrop" aria-label="Close" onclick={() => (open = false)}></button>
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
