<script lang="ts">
	import { onMount } from "svelte";
	import type { AgentMessage, ObjectJSON } from "$lib/types";
	import { fieldStr } from "$lib/types";
	import { mailbox } from "$lib/api";
	import { discussionUI } from "$lib/data.svelte";
	import { agoShort, lastMessage, loadObjectAgents, objectThreads, whoName } from "$lib/conversations";
	import type { ObjectAgentOption } from "$lib/threads";
	import { objectIcon } from "$lib/icons";
	import Discussion from "./Discussion.svelte";

	let {
		object,
		onchanged,
		floating = false,
		onpopout,
		ondock,
	}: {
		object: ObjectJSON;
		onchanged: () => Promise<void>;
		/** True once the human has popped the pane out into a card. */
		floating?: boolean;
		onpopout?: () => void;
		ondock?: () => void;
	} = $props();

	let view = $state<"list" | "thread" | "new">("list");
	let activeId = $state("");
	let options = $state<ObjectAgentOption[]>([]);
	let selected = $state<string[]>([]);
	let search = $state("");
	let title = $state("");
	let draft = $state("");
	let requestReply = $state(true);
	let loading = $state(false);
	let sending = $state(false);
	let error = $state("");
	let refreshError = $state("");
	let pendingSend: AgentMessage | undefined;

	const disc = $derived(lastMessage(object));
	const rows = $derived(objectThreads(object));
	const isDiscussion = $derived(activeId === "__discussion__");
	const activeTitle = $derived(isDiscussion ? "Discussion" : rows.find((t) => t.id === activeId)?.title ?? "Conversation");
	const visibleOptions = $derived(options.filter((option) => `${option.name} ${option.agentName}`.toLowerCase().includes(search.toLowerCase())));
	$effect(() => { discussionUI.convCount = rows.length + 1; });

	function openThread(id: string) {
		activeId = id;
		view = "thread";
	}

	async function newExchange() {
		view = "new";
		error = "";
		loading = true;
		try {
			options = await loadObjectAgents(object);
			selected = selected.filter((id) => options.some((option) => option.endpoint.objectId === id));
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			loading = false;
		}
	}

	async function createExchange() {
		if (sending || !draft.trim() || !selected.length) return;
		const recipients = options.filter((option) => selected.includes(option.endpoint.objectId)).map((option) => option.endpoint);
		if (!recipients.length) return;
		sending = true;
		error = "";
		try {
			const text = draft.trim();
			const exchangeTitle = title.trim() || "Object exchange";
			if (!pendingSend || pendingSend.text !== text || pendingSend.title !== exchangeTitle
				|| pendingSend.requestReply !== requestReply || JSON.stringify(pendingSend.recipients) !== JSON.stringify(recipients)) {
				pendingSend = {
					id: crypto.randomUUID(), exchangeId: crypto.randomUUID(),
					sender: { objectId: object.id, agentId: "" }, recipients,
					text, title: exchangeTitle, replyTo: "", sentAt: Date.now(),
					requestReply, historical: false, operation: "", author: "",
				};
			}
			const sent = await mailbox.send(pendingSend);
			pendingSend = undefined;
			draft = "";
			title = "";
			selected = [];
			openThread(sent.threadId);
			await refreshActive();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			sending = false;
		}
	}

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

	function back() {
		view = "list";
		activeId = "";
	}

	function onKey(e: KeyboardEvent) {
		if (e.key !== "Escape" || e.defaultPrevented) return;
		if (view !== "list") back();
		else discussionUI.open = false;
	}

	onMount(() => {
		if (!rows.length) openThread("__discussion__");
		const timer = setInterval(() => void refreshActive(), 8000);
		return () => clearInterval(timer);
	});
</script>

<svelte:window onkeydown={onKey} />

<header class="dd-head">
	{#if view !== "list"}
		<button class="dd-back" data-tip="Back to conversations (Esc)" onclick={back}>‹</button>
		<div class="dd-titles">
			<span class="dd-title">{view === "new" ? "New exchange" : activeTitle}</span>
			<span class="dd-sub">{fieldStr(object.fields, "name") || "Untitled"}</span>
		</div>
	{:else}
		<span class="dd-icon">{objectIcon("", "chat")}</span>
		<div class="dd-titles">
			<span class="dd-title">Conversations</span>
			<span class="dd-sub">{fieldStr(object.fields, "name") || "Untitled"}</span>
		</div>
		<span class="dd-count">{rows.length + 1}</span>
	{/if}
	{#if view !== "new"}
		<button class="dd-back" aria-label="New exchange" title="New exchange or group" onclick={() => void newExchange()}>+</button>
	{/if}
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
{#if view === "list"}
	<div class="dd-list">
		<button class="conv" onclick={() => void openThread("__discussion__")}>
			<span class="glyph">{objectIcon("", "chat")}</span>
			<span class="conv-main">
				<span class="conv-top">
					<span class="conv-title">Discussion</span>
					<span class="conv-time">{agoShort(disc.last)}</span>
				</span>
				<span class="conv-snippet">
					{#if disc.text}<b>{whoName(disc.author)}:</b> {disc.text}{:else}Start a discussion{/if}
				</span>
			</span>
			{#if disc.count > 0}<span class="conv-count">{disc.count}</span>{/if}
		</button>
		{#each rows as t (t.id)}
			<button class="conv" onclick={() => void openThread(t.id)}>
				<span class="glyph">{objectIcon("", "agent")}</span>
				<span class="conv-main">
					<span class="conv-top">
						<span class="conv-title">{t.title}</span>
						<span class="conv-time">{agoShort(t.last)}</span>
					</span>
					<span class="conv-snippet">
						{#if t.snippet}<b>{t.snippetWho}:</b> {t.snippet}{:else}No messages yet{/if}
					</span>
					{#if t.legacy}<span class="conv-snippet">Read-only · awaiting migration</span>{/if}
					{#if t.problems}<span class="conv-problem">{t.problems} delivery or processing problem{t.problems === 1 ? "" : "s"}</span>{/if}
				</span>
				<span class="conv-count">{t.count}</span>
			</button>
		{/each}
	</div>
{:else if view === "new"}
	<form class="dd-compose" onsubmit={(event) => { event.preventDefault(); void createExchange(); }}>
		<label>Exchange name<input bind:value={title} placeholder="What is this group discussing?" disabled={sending} /></label>
		<label>Find object agents<input type="search" bind:value={search} placeholder="Search existing agents" disabled={sending} /></label>
		<p class="compose-hint">Choose one or more existing object agents. You can include this object's own agent.</p>
		<div class="recipient-list">
			{#each visibleOptions as option (option.endpoint.objectId)}
				<label class="recipient">
					<input type="checkbox" bind:group={selected} value={option.endpoint.objectId} disabled={sending} />
					<span class="glyph">{objectIcon(option.icon, "agent")}</span>
					<span><b>{option.name}</b><small>{option.agentName}{option.endpoint.objectId === object.id ? " · this object" : ""}</small></span>
				</label>
			{/each}
			{#if loading}<p class="compose-hint">Loading object agents…</p>
			{:else if !options.length}<p class="compose-hint">No existing agents in this space. Add an agent to an object first.</p>
			{:else if !visibleOptions.length}<p class="compose-hint">No matching object agents.</p>{/if}
		</div>
		<span class="compose-hint">{selected.length} recipient{selected.length === 1 ? "" : "s"} selected</span>
		<label>Message<textarea bind:value={draft} rows={4} placeholder="Write the first message…" disabled={sending}></textarea></label>
		<label class="request-reply"><input type="checkbox" bind:checked={requestReply} disabled={sending} /> Ask agents to respond</label>
		{#if error}<p class="dd-error" role="alert">{error}</p>{/if}
		{#if !loading && !options.length}<button type="button" onclick={() => void newExchange()}>Reload agents</button>{/if}
		<button class="create-exchange" type="submit" disabled={sending || loading || !draft.trim() || !selected.length}>{sending ? "Sending…" : "Start exchange"}</button>
	</form>
{:else}
	<div class="dd-body">
		{#key activeId}
			<Discussion {object} full threadId={activeId} onchanged={refreshActive} onexchange={openThread} />
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
	.dd-icon {
		font-size: 16px;
	}
	.dd-back {
		background: none;
		border: 1px solid var(--border);
		border-radius: 7px;
		color: var(--muted);
		font-size: 16px;
		width: 26px;
		height: 26px;
		line-height: 1;
		cursor: pointer;
		flex: none;
	}
	.dd-back:hover {
		color: var(--fg);
		border-color: var(--muted);
	}
	.dd-titles {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	.dd-title {
		font-size: 13.5px;
		font-weight: 600;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.dd-sub {
		font-size: 11.5px;
		color: var(--muted);
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.dd-count {
		font-size: 12px;
		color: var(--muted);
		flex: none;
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
	.dd-list {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: 8px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.conv {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		background: none;
		border: 1px solid transparent;
		border-radius: 10px;
		color: var(--fg);
		text-align: left;
		padding: 9px 10px;
		cursor: pointer;
	}
	.conv:hover {
		background: var(--hover);
	}
	.glyph {
		font-size: 16px;
		flex: none;
	}
	.conv-main {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
	}
	.conv-top {
		display: flex;
		align-items: baseline;
		gap: 8px;
	}
	.conv-title {
		flex: 1;
		min-width: 0;
		font-size: 13.5px;
		font-weight: 600;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.conv-time {
		font-size: 11px;
		color: var(--muted);
		flex: none;
	}
	.conv-snippet {
		font-size: 12px;
		color: var(--muted);
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.conv-snippet b {
		font-weight: 600;
		color: var(--fg);
	}
	.conv-count {
		font-size: 11px;
		color: var(--muted);
		border: 1px solid var(--border);
		border-radius: 999px;
		padding: 1px 7px;
		flex: none;
	}
	.dd-body {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.dd-compose {
		display: flex;
		flex: 1;
		min-height: 0;
		flex-direction: column;
		gap: 12px;
		padding: 14px;
		overflow-y: auto;
	}
	.dd-compose label {
		display: flex;
		flex-direction: column;
		gap: 5px;
		font-size: 12px;
		color: var(--muted);
	}
	.dd-compose input:not([type="checkbox"]),
	.dd-compose textarea {
		width: 100%;
		box-sizing: border-box;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 7px;
		padding: 8px;
		color: var(--fg);
		font: inherit;
	}
	.dd-compose textarea { resize: vertical; }
	.compose-hint { color: var(--muted); font-size: 12px; margin: 0; }
	.recipient-list { max-height: 240px; overflow-y: auto; }
	.dd-compose .recipient,
	.dd-compose .request-reply { flex-direction: row; align-items: center; gap: 8px; }
	.recipient { padding: 7px 0; }
	.recipient b { color: var(--fg); font-weight: 500; }
	.recipient small { display: block; margin-top: 2px; }
	.create-exchange {
		background: var(--accent);
		color: #fff;
		border: none;
		border-radius: 7px;
		padding: 9px 12px;
		cursor: pointer;
	}
	.create-exchange:disabled { opacity: 0.4; cursor: default; }
	.dd-error,
	.conv-problem { color: var(--orange, #ff9f0a); font-size: 12px; }
	.dd-error { padding: 0 14px; overflow-wrap: anywhere; }
	/* The full-variant Discussion fills the drawer: messages scroll,
	   composer pinned at the bottom. */
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