<script lang="ts" module>
	/**
	 * Menu and toolbar icons: a fixed subset of Lucide (ISC), inlined so
	 * there is no dependency and no icon font. All 24×24, 2px stroke,
	 * currentColor - one visual weight everywhere, unlike the mix of
	 * Unicode glyphs and emoji this replaced.
	 */
	const PATHS = {
		"arrow-up-right": "M7 7h10v10M7 17 17 7",
		"external-link": "M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6",
		"arrow-left-right": "M8 3 4 7l4 4M4 7h16m-4 14 4-4-4-4M20 17H4",
		"sliders-horizontal": "M10 5H3M12 19H3M14 3v4M16 17v4M21 12h-9M21 19h-5M21 5h-7M8 10v4M8 12H3",
		"folder-plus": "M12 10v6M9 13h6M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
		"folder-minus": "M9 13h6M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
		trash: "M10 11v6M14 11v6M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
		"chevron-right": "m9 18 6-6-6-6",
		"chevron-left": "m15 18-6-6 6-6",
		copy: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2M8 8h14v14H8z",
		pencil: "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497zm-6.174-1.812 4 4",
		plus: "M5 12h14M12 5v14",
		check: "M20 6 9 17l-5-5",
		"align-left": "M21 5H3M15 12H3M17 19H3",
		type: "M12 20h-1a2 2 0 0 1-2-2 2 2 0 0 1-2 2H6M13 8h7a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2h-7M5 16H4a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2h1M6 4h1a2 2 0 0 1 2 2 2 2 0 0 1 2-2h1M9 6v12",
		palette: "M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8zM13.5 6.5h.01M17.5 10.5h.01M6.5 12.5h.01M8.5 7.5h.01",
		"paint-bucket": "M11 7 6 2M18.992 12H2.041M21.145 18.38A3.34 3.34 0 0 1 20 16.5a3.3 3.3 0 0 1-1.145 1.88c-.575.46-.855 1.02-.855 1.595A2 2 0 0 0 20 22a2 2 0 0 0 2-2.025c0-.58-.285-1.13-.855-1.595m-12.645-13.88 2.148-2.148a1.205 1.205 0 0 1 1.704 0l7.296 7.296a1.205 1.205 0 0 1 0 1.704l-7.592 7.592a3.615 3.615 0 0 1-5.112 0l-3.888-3.888a3.615 3.615 0 0 1 0-5.112L5.67 7.33",
		eraser: "M21 21H8a2 2 0 0 1-1.42-.587l-3.994-3.999a2 2 0 0 1 0-2.828l10-10a2 2 0 0 1 2.829 0l5.999 6a2 2 0 0 1 0 2.828L12.834 21m-7.752-9.91 8.828 8.828",
		box: "M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Zm-17.7-1 8.7 5 8.7-5M12 22V12",
	} as const;

	export type IconName = keyof typeof PATHS;
</script>

<script lang="ts">
	let { name, size = 16 }: { name: IconName; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
	<path d={PATHS[name]} />
</svg>

<style>
	svg {
		display: inline-block;
		flex: none;
		vertical-align: -3px;
	}
</style>
