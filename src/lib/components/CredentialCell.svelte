<script lang="ts">
	/**
	 * A view cell that reconnects credentials: a not-connected credential's
	 * Status or Error, or another object's "credential signed out: …" Error
	 * (credential-actions.ts reconnectTarget). The cell's content (the
	 * caller's snippet) becomes a button opening a small popover with each
	 * credential's Reconnect; the click never reaches the row or card under it.
	 */
	import { tick, type Snippet } from "svelte";
	import { fetchAllQuery } from "$lib/api";
	import { credentialsForError, linkedCredentialIds, signedOutNames, type CredentialRef } from "$lib/credential-actions";
	import { fieldStr, guestAgents, type ValueJSON } from "$lib/types";
	import CredentialReconnect from "./CredentialReconnect.svelte";

	let {
		row,
		target,
		onchanged,
		children,
		label,
	}: {
		row: { id: string; typeKey: string; fields: Record<string, ValueJSON> };
		target: { credentialId: string } | { error: string };
		/** Re-read the view's rows. */
		onchanged: () => Promise<void>;
		children: Snippet;
		/** The trigger's accessible name. */
		label: string;
	} = $props();

	let trigger = $state<HTMLButtonElement | null>(null);
	let pop = $state<HTMLDivElement | null>(null);
	let at = $state<{ x: number; y: number } | null>(null);
	let ids = $state<string[] | null>(null);
	let missing = $state<string[]>([]);
	let findError = $state("");

	/** The credentials a signed-out Error names, those the object (or its agent) lists first. */
	async function resolve(error: string) {
		ids = null;
		missing = [];
		findError = "";
		try {
			const agents = row.typeKey === "agent" ? [] : guestAgents(row.fields);
			const [all, agentRows] = await Promise.all([
				fetchAllQuery({ filters: [{ key: "typeKey", condition: "equal", value: "credential" }] }),
				agents.length > 0 ? fetchAllQuery({ filters: [{ key: "id", condition: "in", value: agents }] }) : Promise.resolve([]),
			]);
			const linkedIds = new Set([...linkedCredentialIds(row.fields), ...agentRows.flatMap((a) => linkedCredentialIds(a.fields))]);
			const refs: CredentialRef[] = all.map((c) => ({ id: c.id, name: fieldStr(c.fields, "name") }));
			const found = credentialsForError(error, refs.filter((c) => linkedIds.has(c.id)), refs);
			ids = found.map((c) => c.id);
			missing = signedOutNames(error).filter((n) => !found.some((c) => c.name === n));
		} catch (e) {
			findError = e instanceof Error ? e.message : String(e);
			ids = [];
		}
	}

	async function open() {
		if (!trigger) return;
		const r = trigger.getBoundingClientRect();
		at = { x: Math.max(8, Math.min(r.left, window.innerWidth - 328)), y: r.bottom + 4 };
		if ("credentialId" in target) ids = [target.credentialId];
		else void resolve(target.error);
		await tick();
		pop?.focus();
	}

	function close(refocus: boolean) {
		at = null;
		if (refocus) trigger?.focus();
	}

	function onWindowDown(e: MouseEvent) {
		const t = e.target as Node;
		if (at && !pop?.contains(t) && !trigger?.contains(t)) close(false);
	}
</script>

<svelte:window onmousedown={onWindowDown} />

<button
	bind:this={trigger}
	class="cred-cell"
	aria-haspopup="dialog"
	aria-expanded={at !== null}
	aria-label={label}
	title={label}
	data-testid="credential-cell"
	onclick={(e) => {
		e.stopPropagation();
		if (at) close(false);
		else void open();
	}}
	onkeydown={(e) => {
		// Enter/Space press this button; the row or card under it must not take them too.
		if (e.key === "Enter" || e.key === " ") e.stopPropagation();
	}}
>{@render children()}</button>

{#if at}
	<!-- Clicks and keys inside stay here: the row or card under the cell must not open. -->
	<div
		bind:this={pop}
		class="cred-pop cell-pop"
		role="dialog"
		aria-label="Reconnect credential"
		tabindex="-1"
		style="left:{at.x}px; top:{at.y}px"
		onclick={(e) => e.stopPropagation()}
		oncontextmenu={(e) => e.stopPropagation()}
		onkeydown={(e) => {
			e.stopPropagation();
			if (e.key === "Escape") close(true);
		}}
	>
		{#if ids === null}
			<p class="muted">Finding {signedOutNames("error" in target ? target.error : "").join(", ")}…</p>
		{:else}
			{#each ids as id (id)}
				<CredentialReconnect {id} {onchanged} />
			{/each}
			{#each missing as name (name)}
				<p class="muted">No credential named {name}.</p>
			{/each}
			{#if findError}<p class="error" role="alert">{findError}</p>{/if}
		{/if}
	</div>
{/if}

<style>
	.cred-cell {
		display: inline-block;
		vertical-align: bottom;
		max-width: 100%;
		border: none;
		background: none;
		padding: 0;
		margin: 0;
		font: inherit;
		color: inherit;
		text-align: left;
		cursor: pointer;
		border-radius: 6px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.cred-cell:hover,
	.cred-cell:focus-visible {
		outline: 1px solid var(--accent);
		outline-offset: 1px;
	}
	.cred-pop {
		position: fixed;
		z-index: 140;
		width: 300px;
		box-sizing: border-box;
		display: flex;
		flex-direction: column;
		gap: 12px;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px 12px;
		cursor: default;
		text-align: left;
		color: var(--fg);
		font-size: 13px;
		white-space: normal;
	}
	.muted { margin: 0; color: var(--muted); font-size: 12.5px; }
	.error { margin: 0; color: var(--red); font-size: 12.5px; white-space: pre-wrap; }
</style>
