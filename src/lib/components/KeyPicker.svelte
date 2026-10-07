<script lang="ts">
	/**
	 * A property chooser for filter and sort rules: a button showing the
	 * current property; opening it gives a search box over the list (a vault
	 * has dozens of properties, too many to scan in a plain select).
	 */
	import { keepInView } from "$lib/popover";

	let {
		value,
		keys,
		label,
		onpick,
	}: {
		value: string;
		keys: string[];
		/** How a key reads in the list and on the button (emoji + name). */
		label: (key: string) => string;
		onpick: (key: string) => void;
	} = $props();

	let open = $state(false);
	let query = $state("");
	let active = $state(0);
	let wrapEl = $state<HTMLElement>();

	const matches = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return q ? keys.filter((k) => label(k).toLowerCase().includes(q) || k.toLowerCase().includes(q)) : keys;
	});

	function toggle() {
		open = !open;
		query = "";
		active = Math.max(0, keys.indexOf(value));
	}

	function pick(key: string) {
		open = false;
		if (key !== value) onpick(key);
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === "ArrowDown") { e.preventDefault(); active = Math.min(active + 1, matches.length - 1); }
		else if (e.key === "ArrowUp") { e.preventDefault(); active = Math.max(active - 1, 0); }
		else if (e.key === "Enter") { e.preventDefault(); if (matches[active]) pick(matches[active]); }
		else if (e.key === "Escape") { e.preventDefault(); open = false; }
	}

	function onWindowPointerdown(e: PointerEvent) {
		if (open && wrapEl && !wrapEl.contains(e.target as Node)) open = false;
	}
</script>

<svelte:window onpointerdown={onWindowPointerdown} />

<span class="key-picker" bind:this={wrapEl}>
	<button class="current" aria-haspopup="listbox" aria-expanded={open} onclick={toggle}>{label(value)} <span class="chev">▾</span></button>
	{#if open}
		<div class="menu" use:keepInView={{ placement: "below", align: "left" }}>
			<!-- svelte-ignore a11y_autofocus -->
			<input
				class="search"
				placeholder="Search properties"
				bind:value={query}
				oninput={() => (active = 0)}
				onkeydown={onKeydown}
				autofocus
			/>
			<div class="list" role="listbox">
				{#each matches as k, i (k)}
					<button role="option" aria-selected={k === value} class:active={i === active} class:on={k === value} onmouseenter={() => (active = i)} onclick={() => pick(k)}>{label(k)}</button>
				{:else}
					<div class="empty">No property matches “{query}”.</div>
				{/each}
			</div>
		</div>
	{/if}
</span>

<style>
	.key-picker {
		position: relative;
		display: inline-flex;
	}
	.current {
		background: var(--panel);
		border: 1px solid var(--border);
		color: var(--fg);
		border-radius: 6px;
		padding: 4px 8px;
		font-size: 13px;
		cursor: pointer;
		white-space: nowrap;
	}
	.current[aria-expanded="true"] {
		border-color: var(--accent);
	}
	.chev {
		color: var(--muted);
		font-size: 10px;
	}
	.menu {
		position: fixed;
		z-index: 120;
		width: 240px;
		display: flex;
		flex-direction: column;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 6px;
		box-shadow: 0 16px 48px rgb(0 0 0 / 0.5);
	}
	.search {
		background: var(--bg);
		border: 1px solid var(--border);
		color: var(--fg);
		border-radius: 6px;
		padding: 6px 8px;
		font-size: 13px;
		margin-bottom: 4px;
		outline: none;
	}
	.search:focus {
		border-color: var(--accent);
	}
	.list {
		overflow-y: auto;
		min-height: 0;
		max-height: 300px;
		display: flex;
		flex-direction: column;
	}
	.list button {
		flex-shrink: 0;
		text-align: left;
		background: none;
		border: none;
		color: var(--fg);
		font-size: 13px;
		padding: 6px 8px;
		border-radius: 6px;
		cursor: pointer;
	}
	.list button.active {
		background: var(--hover);
	}
	.list button.on {
		color: var(--accent);
	}
	.empty {
		color: var(--muted);
		font-size: 12px;
		padding: 6px 8px;
	}
</style>
