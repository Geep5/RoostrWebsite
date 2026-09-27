<script lang="ts">
	/** "Open the side pane" (Chat + Properties): a window with its right third marked off. */
	let { size = 18 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
	<rect x="3" y="4" width="18" height="16" rx="3" />
	<line x1="15" y1="4" x2="15" y2="20" />
</svg>
