<script lang="ts">
	/**
	 * The glyph a property badge leads with: a 16px circle in the badge's
	 * icon colour with a white mark, or an outline where the state is
	 * "nothing yet" (dashed ring, dot). Colour comes from `--badge-icon`.
	 */
	import type { BadgeIcon } from "$lib/options";

	let { icon, size = 16 }: { icon: BadgeIcon; size?: number } = $props();
</script>

<svg class="pi" width={size} height={size} viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
	{#if icon === "check"}
		<circle cx="8" cy="8" r="8" fill="currentColor" />
		<path d="M4.5 8.2l2.3 2.3 4.7-4.8" stroke="var(--panel)" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
	{:else if icon === "x"}
		<circle cx="8" cy="8" r="8" fill="currentColor" />
		<path d="M5.3 5.3l5.4 5.4M10.7 5.3l-5.4 5.4" stroke="var(--panel)" stroke-width="1.8" stroke-linecap="round" />
	{:else if icon === "clock"}
		<circle cx="8" cy="8" r="8" fill="currentColor" />
		<path d="M8 4.2V8l2.6 1.6" stroke="var(--panel)" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" />
	{:else if icon === "half"}
		<circle cx="8" cy="8" r="6.9" stroke="currentColor" stroke-width="2" />
		<path d="M8 3.6a4.4 4.4 0 0 1 0 8.8z" fill="currentColor" />
	{:else if icon === "dashed"}
		<circle cx="8" cy="8" r="6.6" stroke="currentColor" stroke-width="1.8" stroke-dasharray="2.6 2.4" stroke-linecap="round" />
	{:else if icon === "calendar"}
		<rect x="2.5" y="3.5" width="11" height="10" rx="2" stroke="currentColor" stroke-width="1.6" />
		<path d="M2.5 6.8h11M5.5 2v2.6M10.5 2v2.6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
	{:else if icon === "link"}
		<path d="M6.8 9.2l2.4-2.4M5.6 10.4L4.5 11.5a2.1 2.1 0 0 1-3-3l2-2a2.1 2.1 0 0 1 3 0M10.4 5.6l1.1-1.1a2.1 2.1 0 0 1 3 3l-2 2a2.1 2.1 0 0 1-3 0" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
	{:else if icon === "at"}
		<circle cx="8" cy="8" r="2.6" stroke="currentColor" stroke-width="1.6" />
		<path d="M10.6 8c0 1.6.6 2.4 1.6 2.4S14 9.4 14 8a6 6 0 1 0-2.4 4.8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
	{:else if icon === "phone"}
		<path d="M4 2.5h2.2l1.2 3-1.6 1.1a6 6 0 0 0 3.6 3.6l1.1-1.6 3 1.2V12A1.5 1.5 0 0 1 12 13.5C6.6 13.5 2.5 9.4 2.5 4A1.5 1.5 0 0 1 4 2.5z" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" />
	{:else}
		<circle cx="8" cy="8" r="4.2" fill="currentColor" />
	{/if}
</svg>

<style>
	.pi {
		flex: none;
		color: var(--badge-icon, var(--muted));
		display: block;
	}
</style>
