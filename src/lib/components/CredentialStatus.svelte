<script lang="ts">
	/**
	 * A credential's Status editor. The status is the harness's to write, so
	 * instead of options it says the status in words and offers the actions
	 * that change it: Connect in a tab paired with any of the person's
	 * computers for a website or Google sign-in (the window opens on that
	 * computer and the login is handed to the Served by one), and Check now
	 * from any device - the Served by computer runs it. A Check's progress is
	 * the one the views' popover and cells show (credential-reconnect.svelte.ts).
	 */
	import { onMount } from "svelte";
	import { thisMachineId } from "$lib/capability-actions";
	import { canConnect, checkLine, connectingHint, connectLabel, credentialActionGate, credentialStatusText, performCredentialAction, type CredentialAction } from "$lib/credential-actions";
	import { check, checking, checkState, followCheck } from "$lib/credential-reconnect.svelte";
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
	/** This tab started the sign-in that is running (so its window is on this computer). */
	let startedHere = $state(false);

	const isTemplate = $derived(object.typeKey === "template");
	const status = $derived(fieldStr(object.fields, "status") || "missing");
	const servedBy = $derived(servedByMachineId(object.fields, machines));
	const computer = $derived(servedBy ? machineName(machines, servedBy) : "");
	const gate = $derived(credentialActionGate(paired, thisMachine, servedBy, computer, object.fields));
	const handsTo = $derived(thisMachine && thisMachine !== servedBy ? computer : "");
	const checkNow = $derived(checkState(object.id));
	const waitingForCheck = $derived(checking(object.id, object.fields));
	const checkSays = $derived(waitingForCheck ? checkLine({ phase: "checking", error: "", fields: object.fields, requestedAt: 0 }, computer) : checkNow ? checkLine(checkNow, computer) : null);

	// A Check another device asked for: follow it here too, so this row says when it is answered.
	$effect(() => {
		if (!isTemplate) followCheck(object.id, object.fields, onchanged);
	});

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
	}

	async function run(action: CredentialAction) {
		if (busy) return;
		busy = action;
		actionError = "";
		const { result, error } = await performCredentialAction(action, object.id, onchanged);
		actionError = error;
		if (action === "connect") startedHere = result?.status === "connecting";
		if (action === "disconnect") startedHere = false;
		busy = "";
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
		{#if gate.canConnect || gate.canCheck}
			<div class="actions">
				{#if gate.canConnect && canConnect(object.fields)}
					<button class="act" disabled={!!busy} data-testid="credential-connect" onclick={() => void run("connect")}>{connectLabel(status)}</button>
				{/if}
				{#if gate.canCheck}
					<button class="act" disabled={waitingForCheck} data-testid="credential-check" title={`${computer || "The computer serving it"} tests the login now`} onclick={() => check(object.id, object.fields, onchanged, { here: gate.checkHere })}>Check now</button>
				{/if}
				{#if gate.canConnect}
					<button class="act" disabled={!!busy} data-testid="credential-disconnect" title="Stops a sign-in running on this computer. The saved login stays on the credential and keeps working until the site refuses it - clear its Cookie or key properties to remove it." onclick={() => void run("disconnect")}>Disconnect</button>
				{/if}
			</div>
		{/if}
		{#if gate.reason}
			<p class="muted">{gate.reason}</p>
			{#if !paired}<PairGate compact onready={() => void loadPairing()} />{/if}
		{/if}
		{#if status === "connecting"}
			<p class="muted" role="status">{startedHere ? connectingHint(object.fields, "this computer", handsTo) : fieldStr(object.fields, "error") || connectingHint(object.fields, "", "")}</p>
		{/if}
		{#if checkSays}
			<p class={checkSays.tone} role={checkSays.tone === "error" ? "alert" : "status"} data-testid="credential-check-result">{checkSays.text}</p>
		{/if}
	{/if}
	{#if actionError || pollError}<p class="error" role="alert" data-testid="credential-error">{actionError || pollError}</p>{/if}
</div>

<style>
	.cred-status { display: flex; flex-direction: column; gap: 8px; min-width: 220px; }
	.now { margin: 0; font-size: 14px; font-weight: 600; color: var(--fg); }
	.muted { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.45; }
	.ok { margin: 0; color: var(--fg); font-size: 12.5px; font-weight: 600; }
	.error { margin: 0; color: #ff6961; font-size: 12.5px; line-height: 1.45; white-space: pre-wrap; }
	.actions { display: flex; gap: 6px; flex-wrap: wrap; }
	.act { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--fg); }
	.act:hover:not(:disabled) { border-color: var(--accent); }
	.act:disabled { opacity: 0.5; cursor: default; }
</style>
