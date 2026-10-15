<script lang="ts">
	/**
	 * A space with nothing open on this device: a light page. Nothing is
	 * loaded here - a query (the space's pinned "All Objects" included) runs
	 * only when you open it from the sidebar, so switching to a big space
	 * never draws thousands of rows first. Phones never show this page:
	 * their space screen is the pinned cards.
	 */
	import InstallComputer from "$lib/components/InstallComputer.svelte";
	import { store } from "$lib/data.svelte";
</script>

<InstallComputer />

{#if store.loaded}
	<p class="empty">Nothing open in this space yet. Pick something in the sidebar - All Objects lists everything here - or make something new from the New menu.</p>
{/if}

<style>
	.empty {
		color: var(--muted);
		font-size: 14px;
		margin: 24px 0;
	}
</style>
