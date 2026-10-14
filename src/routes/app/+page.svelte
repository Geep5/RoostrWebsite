<script lang="ts">
	/**
	 * A space with nothing open on this device. Every space has a pinned
	 * "All Objects" query (the engine seeds it: `bundled-query-all-<space id
	 * prefix>`), so this opens that - the list of everything used to live
	 * here, with no way back to it once you left. If the query was deleted,
	 * the sidebar is the way in. Phones never show this page: their space
	 * screen is the pinned cards.
	 */
	import { goto } from "$app/navigation";
	import { activeSpace } from "$lib/space.svelte";
	import { store } from "$lib/data.svelte";
	import InstallComputer from "$lib/components/InstallComputer.svelte";

	const channelId = $derived(activeSpace.id || store.channels[0]?.id || "");
	const allObjectsId = $derived(channelId ? `bundled-query-all-${channelId.slice(0, 8)}` : "");
	const allObjects = $derived(store.summaries.find((s) => s.id === allObjectsId));

	$effect(() => {
		if (allObjects) void goto(`/app/object/${allObjects.id}`, { replaceState: true });
	});
</script>

<InstallComputer />

{#if store.loaded && !allObjects}
	<p class="empty">Nothing open in this space yet. Pick something in the sidebar, or make something new from the New menu.</p>
{/if}

<style>
	.empty {
		color: var(--muted);
		font-size: 14px;
		margin: 24px 0;
	}
</style>
