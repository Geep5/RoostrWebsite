<script lang="ts">
	/**
	 * One credential's Reconnect, as a view's popover offers it: the same
	 * pairing gate, harness Connect and status follow as its Properties
	 * Status row (credential-actions.ts), plus Open credential. Progress lives
	 * in credential-reconnect.svelte.ts, so the cell shows it too and it
	 * carries on when the popover closes.
	 */
	import { onMount } from "svelte";
	import { goto } from "$app/navigation";
	import { fetchObject } from "$lib/api";
	import { thisMachineId } from "$lib/capability-actions";
	import { canConnect, connectingHint, connectLabel, credentialActionGate, credentialStatusText } from "$lib/credential-actions";
	import { reconnect, reconnectState } from "$lib/credential-reconnect.svelte";
	import { onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, servedByMachineId, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let { id, onchanged }: { id: string; onchanged: () => Promise<void> } = $props();

	let object = $state<ObjectJSON | null>(null);
	let loadError = $state("");
	let machines = $state<MachineRow[]>([]);
	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let ready = $state(false);

	const fields = $derived(object?.fields ?? {});
	const name = $derived(fieldStr(fields, "name") || "this credential");
	const status = $derived(fieldStr(fields, "status") || "missing");
	const servedBy = $derived(servedByMachineId(fields, machines));
	const computer = $derived(servedBy ? machineName(machines, servedBy) : "");
	const gate = $derived(credentialActionGate(paired, thisMachine, servedBy, computer));
	const progress = $derived(reconnectState(id));
	const running = $derived(progress?.phase === "starting" || progress?.phase === "waiting");

	async function loadObject() {
		try {
			object = await fetchObject(id);
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		}
	}

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
	}

	/** The view re-reads its rows, and this block its credential. */
	async function changed() {
		await Promise.all([onchanged(), loadObject()]);
	}

	onMount(() => {
		void Promise.all([
			loadObject(),
			fetchMachines().then(({ machines: roster }) => (machines = roster)).catch(() => {}),
			loadPairing(),
		]).then(() => {
			ready = true;
			// A sign-in already under way (started in the pane or another tab): follow it here too.
			if (fieldStr(object?.fields ?? {}, "status") === "connecting") reconnect(id, changed, true);
		});
		return onPairingChange(() => void loadPairing());
	});
</script>

<div class="cred-reconnect" data-testid="credential-reconnect">
	{#if loadError}
		<p class="error" role="alert">{loadError}</p>
	{:else if !ready || !object}
		<p class="muted">Loading…</p>
	{:else}
		<p class="now"><span class="name">{name}</span> · {credentialStatusText(status)}</p>
		{#if !canConnect(fields)}
			<p class="muted">{name} signs in with pasted keys - open it to replace them.</p>
		{:else if gate.canAct}
			<button class="act primary" disabled={running} data-testid="credential-reconnect-run" onclick={() => reconnect(id, changed)}>{connectLabel(status)} {name}</button>
		{:else}
			<p class="muted">{gate.reason}</p>
			{#if !paired}<PairGate compact onready={() => void loadPairing()} />{/if}
		{/if}
		{#if progress?.phase === "starting"}
			<p class="muted" role="status">Opening the sign-in on {computer || "its computer"}…</p>
		{:else if progress?.phase === "waiting" || (!progress && status === "connecting")}
			<p class="muted" role="status">Waiting for sign-in… {connectingHint(fields, computer)}</p>
		{:else if progress?.phase === "connected"}
			<p class="ok" role="status">Connected</p>
		{:else if progress?.phase === "failed"}
			<p class="error" role="alert" data-testid="credential-reconnect-error">{progress.error}</p>
		{/if}
	{/if}
	<button class="act" onclick={() => void goto(`/app/object/${id}`)}>Open credential</button>
</div>

<style>
	.cred-reconnect { display: flex; flex-direction: column; align-items: flex-start; gap: 6px; }
	.now { margin: 0; font-size: 13px; color: var(--fg); }
	.name { font-weight: 600; }
	.muted { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.45; }
	.ok { margin: 0; color: var(--fg); font-size: 12.5px; font-weight: 600; }
	.error { margin: 0; color: var(--red); font-size: 12.5px; line-height: 1.45; white-space: pre-wrap; }
	.act { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--fg); }
	.act:hover:not(:disabled), .act:focus-visible { border-color: var(--accent); }
	.act.primary { border-color: var(--accent); }
	.act:disabled { opacity: 0.5; cursor: default; }
</style>
