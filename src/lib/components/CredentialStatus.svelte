<script lang="ts">
	/**
	 * A credential's Status editor. The status is the harness's to write, so
	 * instead of options it says the status in words and offers the actions
	 * that change it - only in a tab paired with the computer its Served by
	 * names, since that computer's harness opens the sign-in window.
	 */
	import { onMount } from "svelte";
	import { thisMachineId } from "$lib/capability-actions";
	import { canConnect, credentialStatusText, GOOGLE_ACCOUNT, runCredentialAction, type CredentialAction } from "$lib/credential-actions";
	import { onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, servedByMachineId, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let {
		object,
		onchanged,
		pollError = "",
	}: {
		object: ObjectJSON;
		onchanged: () => Promise<void>;
		/** The pane's connect poll gave up (the pane follows a `connecting` status, popover open or not). */
		pollError?: string;
	} = $props();

	let machines = $state<MachineRow[]>([]);
	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let ready = $state(false);
	let busy = $state<CredentialAction | "">("");
	let actionError = $state("");

	const isTemplate = $derived(object.typeKey === "template");
	const status = $derived(fieldStr(object.fields, "status") || "missing");
	const servedBy = $derived(servedByMachineId(object.fields, machines));
	const computer = $derived(servedBy ? machineName(machines, servedBy) : "");
	const canAct = $derived(paired && !!thisMachine && thisMachine === servedBy);
	const isGoogle = $derived(fieldStr(object.fields, "service") === GOOGLE_ACCOUNT);

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
	}

	async function run(action: CredentialAction) {
		if (busy) return;
		if (action === "disconnect" && !confirm(`Disconnect ${fieldStr(object.fields, "name") || "this credential"}? This removes its sign-in from ${computer || "the computer that keeps it"}.`)) return;
		busy = action;
		actionError = "";
		try {
			await runCredentialAction(action, object.id);
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = "";
			await onchanged().catch(() => {});
		}
	}

	onMount(() => {
		void Promise.all([
			fetchMachines().then(({ machines: roster }) => (machines = roster)).catch(() => {}),
			loadPairing(),
		]).finally(() => (ready = true));
		return onPairingChange(() => void loadPairing());
	});
</script>

<div class="cred-status" data-testid="credential-status">
	<p class="now">{credentialStatusText(status)}</p>
	{#if isTemplate}
		<p class="muted">A template does not connect; each credential made from it does.</p>
	{:else if ready}
		{#if canAct}
			<div class="actions">
				{#if canConnect(object.fields)}
					<button class="act" disabled={!!busy} data-testid="credential-connect" onclick={() => void run("connect")}>{status === "active" || status === "needs_auth" ? "Reconnect" : "Connect"}</button>
				{/if}
				<button class="act" disabled={!!busy} data-testid="credential-check" onclick={() => void run("check")}>Check now</button>
				<button class="act" disabled={!!busy} data-testid="credential-disconnect" onclick={() => void run("disconnect")}>Disconnect</button>
			</div>
		{:else}
			<p class="muted">{servedBy ? `Open Roostr on ${computer} to connect or check this credential.` : "Set Served by to a computer first."}</p>
			{#if !paired}<PairGate compact onready={() => void loadPairing()} />{/if}
		{/if}
		{#if status === "connecting"}
			<p class="muted" role="status">
				{#if isGoogle}
					Sign in to {fieldStr(object.fields, "account") || "the Google account"} in the browser window gws opens on {computer || "its computer"}.
				{:else}
					Sign in in the Chrome window that just opened on {computer || "its computer"}.
				{/if}
			</p>
		{/if}
	{/if}
	{#if actionError || pollError}<p class="error" role="alert" data-testid="credential-error">{actionError || pollError}</p>{/if}
</div>

<style>
	.cred-status { display: flex; flex-direction: column; gap: 8px; min-width: 220px; }
	.now { margin: 0; font-size: 14px; font-weight: 600; color: var(--fg); }
	.muted { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.45; }
	.error { margin: 0; color: #ff6961; font-size: 12.5px; line-height: 1.45; white-space: pre-wrap; }
	.actions { display: flex; gap: 6px; flex-wrap: wrap; }
	.act { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--fg); }
	.act:hover:not(:disabled) { border-color: var(--accent); }
	.act:disabled { opacity: 0.5; cursor: default; }
</style>
