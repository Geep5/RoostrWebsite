<script lang="ts">
	import { onMount } from "svelte";
	import type { ObjectJSON, RelationDefJSON } from "$lib/types";
	import { fieldStr } from "$lib/types";
	import { discussionUI } from "$lib/data.svelte";
	import { objectIcon } from "$lib/icons";
	import Discussion from "./Discussion.svelte";
	import PropertiesPane from "./PropertiesPane.svelte";

	let {
		object,
		relations,
		onchanged,
		floating = false,
		onpopout,
		ondock,
	}: {
		object: ObjectJSON;
		relations: RelationDefJSON[];
		onchanged: () => Promise<void>;
		/** True once the human has popped the pane out into a card. */
		floating?: boolean;
		onpopout?: () => void;
		ondock?: () => void;
	} = $props();

	let tab = $state<"chat" | "props">("chat");
	let refreshError = $state("");

	let refreshing = false;
	async function refreshActive() {
		if (refreshing) return;
		refreshing = true;
		try {
			await onchanged();
			refreshError = "";
		} catch (err) {
			refreshError = err instanceof Error ? err.message : String(err);
		} finally {
			refreshing = false;
		}
	}

	function onKey(e: KeyboardEvent) {
		if (e.key !== "Escape" || e.defaultPrevented) return;
		if (tab === "props") tab = "chat";
		else discussionUI.open = false;
	}

	onMount(() => {
		const timer = setInterval(() => void refreshActive(), 8000);
		return () => clearInterval(timer);
	});
</script>

<svelte:window onkeydown={onKey} />

<header class="dd-head">
	<div class="dd-tabs" role="tablist" aria-label="Pane">
		<button class="dd-tab" class:active={tab === "chat"} role="tab" aria-selected={tab === "chat"} onclick={() => (tab = "chat")}>
			<span class="dd-tab-icon">{objectIcon("", "chat")}</span>Chat
		</button>
		<button class="dd-tab" class:active={tab === "props"} role="tab" aria-selected={tab === "props"} onclick={() => (tab = "props")}>
			<span class="dd-tab-icon">🧩</span>Properties
		</button>
	</div>
	<span class="dd-sub">{fieldStr(object.fields, "name") || "Untitled"}</span>
	<!-- Affixed is the default; popping out hands the pane to the pointer as
	     a card that can be dragged anywhere. Subtle on purpose: it sits with
	     the close control, not as a call to action. -->
	{#if floating}
		<button class="dd-affix" data-tip="Affix to the side" onclick={() => ondock?.()}>⇥</button>
	{:else}
		<button class="dd-affix" data-tip="Pop out" onclick={() => onpopout?.()}>⇱</button>
	{/if}
	<button class="dd-close" data-tip="Close" onclick={() => (discussionUI.open = false)}>»</button>
</header>

{#if refreshError}
	<p class="dd-error" role="status">{refreshError} <button onclick={() => void refreshActive()}>Refresh</button></p>
{/if}
{#if tab === "props"}
	<div class="dd-props">
		<PropertiesPane {object} {relations} {onchanged} />
	</div>
{:else}
	<div class="dd-body">
		{#key object.id}
			<Discussion {object} full threadId="__discussion__" onchanged={refreshActive} onexchange={() => {}} />
		{/key}
	</div>
{/if}

<style>
	.dd-head {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 14px;
		flex: none;
		position: relative;
	}
	/* The divider is inset like the object header's, not edge to edge. */
	.dd-head::after {
		content: "";
		position: absolute;
		left: 14px;
		right: 14px;
		bottom: 0;
		height: 1px;
		background: var(--border);
	}
	.dd-tabs {
		display: flex;
		gap: 2px;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 9px;
		padding: 2px;
		flex: none;
	}
	.dd-tab {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		background: none;
		border: none;
		border-radius: 7px;
		padding: 4px 10px;
		font-size: 12.5px;
		font-weight: 600;
		color: var(--muted);
		cursor: pointer;
	}
	.dd-tab:hover { color: var(--fg); }
	.dd-tab.active {
		background: var(--panel-2, var(--hover));
		color: var(--fg);
	}
	.dd-tab-icon { font-size: 13px; }
	.dd-sub {
		flex: 1;
		min-width: 0;
		font-size: 11.5px;
		color: var(--muted);
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		text-align: right;
	}
	.dd-close,
	.dd-affix {
		background: none;
		border: 1px solid var(--border);
		border-radius: 7px;
		color: var(--muted);
		font-size: 14px;
		width: 26px;
		height: 26px;
		cursor: pointer;
		flex: none;
	}
	/* Subtle until wanted: no border at rest, so the pane header stays quiet. */
	.dd-affix {
		border-color: transparent;
		opacity: 0.6;
	}
	.dd-close:hover,
	.dd-affix:hover {
		color: var(--fg);
		border-color: var(--muted);
		opacity: 1;
	}
	.dd-body {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.dd-error {
		color: var(--orange, #ff9f0a);
		font-size: 12px;
		padding: 0 14px;
		overflow-wrap: anywhere;
	}
	.dd-props {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: 8px 14px;
	}
	.dd-body :global(.discussion.full) {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0 14px 12px;
	}
	.dd-body :global(.discussion.full .messages) {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		max-height: none;
	}
</style>