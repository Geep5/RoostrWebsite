<script lang="ts" module>
	/** Small muted line icons a property row leads with, one per property kind. */
	export type LineIcon =
		| "text" | "number" | "date" | "checkbox" | "status" | "tag" | "url" | "email" | "phone"
		| "object" | "file" | "person" | "computer" | "repeat" | "clock" | "warning" | "color"
		| "key" | "tool" | "brain" | "folder" | "skill" | "cpu" | "search" | "box" | "globe";
</script>

<script lang="ts">
	/**
	 * The glyph a property badge leads with: a 16px circle in the badge's
	 * icon colour with a white mark, or an outline where the state is
	 * "nothing yet" (dashed ring, dot). Colour comes from `--badge-icon`.
	 * With `line` instead: a thin outline icon for a property's kind, in the
	 * current text colour.
	 */
	import type { BadgeIcon } from "$lib/options";

	let { icon = "dot", line, size = 16 }: { icon?: BadgeIcon; line?: LineIcon; size?: number } = $props();
</script>

{#if line}
	<svg class="li" width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
		{#if line === "text"}
			<path d="M3 4h10M3 8h10M3 12h6" />
		{:else if line === "number"}
			<path d="M6.5 2.5l-1.5 11M11.5 2.5l-1.5 11M3 6h10.5M2.5 10h10.5" />
		{:else if line === "date"}
			<rect x="2.5" y="3.5" width="11" height="10" rx="2" /><path d="M2.5 6.8h11M5.5 2v2.6M10.5 2v2.6" />
		{:else if line === "checkbox"}
			<rect x="2.5" y="2.5" width="11" height="11" rx="2.5" /><path d="M5.5 8.2l1.8 1.8 3.4-3.6" />
		{:else if line === "status"}
			<circle cx="8" cy="8" r="5.5" /><path d="M8 2.5a5.5 5.5 0 0 1 0 11z" fill="currentColor" stroke="none" />
		{:else if line === "tag"}
			<path d="M2.5 2.5h5l6 6-5 5-6-6z" /><circle cx="5.3" cy="5.3" r="0.9" />
		{:else if line === "url"}
			<path d="M6.8 9.2l2.4-2.4M5.6 10.4L4.5 11.5a2.1 2.1 0 0 1-3-3l2-2a2.1 2.1 0 0 1 3 0M10.4 5.6l1.1-1.1a2.1 2.1 0 0 1 3 3l-2 2a2.1 2.1 0 0 1-3 0" />
		{:else if line === "email"}
			<circle cx="8" cy="8" r="2.6" /><path d="M10.6 8c0 1.6.6 2.4 1.6 2.4S14 9.4 14 8a6 6 0 1 0-2.4 4.8" />
		{:else if line === "phone"}
			<path d="M4 2.5h2.2l1.2 3-1.6 1.1a6 6 0 0 0 3.6 3.6l1.1-1.6 3 1.2V12A1.5 1.5 0 0 1 12 13.5C6.6 13.5 2.5 9.4 2.5 4A1.5 1.5 0 0 1 4 2.5z" />
		{:else if line === "object"}
			<path d="M7 3H3.5v9.5H13V9M9.5 2.5h4v4M13.5 2.5L7.5 8.5" />
		{:else if line === "file"}
			<path d="M4 2h5l3.5 3.5V14H4zM9 2v3.5h3.5" />
		{:else if line === "person"}
			<circle cx="8" cy="5.5" r="2.5" /><path d="M3 13.5c.8-2.6 2.8-4 5-4s4.2 1.4 5 4" />
		{:else if line === "computer"}
			<rect x="2" y="3" width="12" height="8" rx="1.5" /><path d="M6 14h4M8 11v3" />
		{:else if line === "repeat"}
			<path d="M3 7.5a5 5 0 0 1 8.9-3M13 2v2.5h-2.5M13 8.5a5 5 0 0 1-8.9 3M3 14v-2.5h2.5" />
		{:else if line === "clock"}
			<circle cx="8" cy="8" r="5.5" /><path d="M8 5v3l2 1.3" />
		{:else if line === "warning"}
			<path d="M8 2.5l6 10.5H2zM8 6.5v3M8 11.4v.1" />
		{:else if line === "color"}
			<path d="M8 2.5c2.5 3 4 5 4 7a4 4 0 0 1-8 0c0-2 1.5-4 4-7z" />
		{:else if line === "key"}
			<circle cx="5.5" cy="10.5" r="3" /><path d="M7.6 8.4L13 3M11 5l1.5 1.5" />
		{:else if line === "tool"}
			<path d="M10 2.5a3 3 0 0 0-2.8 4L2.8 10.9a1.4 1.4 0 0 0 2 2l4.4-4.4a3 3 0 0 0 4-2.8l-1.8 1.8-1.7-.4-.4-1.7z" />
		{:else if line === "brain"}
			<path d="M7.5 3.2A2.3 2.3 0 0 0 3.6 4.8a2.4 2.4 0 0 0-.9 4 2.4 2.4 0 0 0 1.9 3.6 2.2 2.2 0 0 0 2.9.6V3.2zM8.5 3.2a2.3 2.3 0 0 1 3.9 1.6 2.4 2.4 0 0 1 .9 4 2.4 2.4 0 0 1-1.9 3.6 2.2 2.2 0 0 1-2.9.6" />
		{:else if line === "folder"}
			<path d="M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
		{:else if line === "skill"}
			<path d="M8 2l1.3 3.7L13 7l-3.7 1.3L8 12l-1.3-3.7L3 7l3.7-1.3z" />
		{:else if line === "cpu"}
			<rect x="4" y="4" width="8" height="8" rx="1" /><path d="M6.5 2v2M9.5 2v2M6.5 12v2M9.5 12v2M2 6.5h2M2 9.5h2M12 6.5h2M12 9.5h2" />
		{:else if line === "search"}
			<circle cx="7" cy="7" r="4" /><path d="M10 10l3.5 3.5" />
		{:else if line === "box"}
			<path d="M2.5 5L8 2.5 13.5 5v6L8 13.5 2.5 11zM2.5 5L8 7.5l5.5-2.5M8 7.5v6" />
		{:else}
			<circle cx="8" cy="8" r="5.5" /><path d="M2.5 8h11M8 2.5c-2 2-2 9 0 11 2-2 2-9 0-11" />
		{/if}
	</svg>
{:else}
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
{/if}

<style>
	.pi {
		flex: none;
		color: var(--badge-icon, var(--muted));
		display: block;
	}
	.li {
		flex: none;
		display: block;
	}
</style>
