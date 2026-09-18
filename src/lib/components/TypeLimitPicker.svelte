<script lang="ts">
	/**
	 * Anytype's "Limit object types" (relation edit panel): an Object
	 * property can name the types it accepts, and the value picker then
	 * offers those alone. Empty means any object.
	 *
	 * Shared by every create surface (slash suggest, the table's property
	 * flow, the sidebar's New property) so the rule lives in one place -
	 * and it matches PropertyPanel, which edits the same field on an
	 * existing property.
	 */
	import { store } from "$lib/data.svelte";
	import { currentSpaceId } from "$lib/relations";

	let { selected = $bindable<string[]>([]) }: { selected: string[] } = $props();

	// Bundled types carry no space stamp; they belong to every space.
	const types = $derived(store.types.filter((t) => !t.space || t.space === currentSpaceId()));

	function toggle(id: string) {
		selected = selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id];
	}
</script>

<div class="limit">
	<div class="limit-head">Limit object types</div>
	<div class="limit-list">
		{#each types as t (t.id)}
			<button class="limit-item" class:on={selected.includes(t.id)} onclick={() => toggle(t.id)}>
				<span class="li-glyph">{t.icon || "▣"}</span>
				<span class="li-name">{t.name || t.key}</span>
				{#if selected.includes(t.id)}<span class="li-check">✓</span>{/if}
			</button>
		{/each}
		{#if types.length === 0}
			<span class="limit-note">This space has no types yet.</span>
		{/if}
	</div>
	<span class="limit-note">{selected.length === 0 ? "Any object can be linked." : "Only these types can be linked."}</span>
</div>

<style>
	.limit {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.limit-head {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--muted);
		padding: 6px 6px 2px;
	}
	.limit-list {
		display: flex;
		flex-direction: column;
		max-height: 148px;
		overflow-y: auto;
	}
	.limit-item {
		display: flex;
		align-items: center;
		gap: 8px;
		border: none;
		background: none;
		color: inherit;
		text-align: left;
		padding: 6px 8px;
		border-radius: 6px;
		cursor: pointer;
		font-size: 13px;
	}
	.limit-item:hover {
		background: var(--hover);
	}
	.limit-item.on {
		background: var(--hl-light);
	}
	.li-glyph {
		flex: none;
		width: 18px;
		text-align: center;
	}
	.li-name {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.li-check {
		flex: none;
		color: var(--accent);
		font-size: 12px;
	}
	.limit-note {
		color: var(--muted);
		font-size: 11px;
		padding: 2px 6px 4px;
	}
</style>
