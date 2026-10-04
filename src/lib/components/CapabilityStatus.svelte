<script lang="ts">
	/**
	 * A capability's Status editor. The serving computer's harness writes the
	 * status, so instead of options it says the status in words and offers
	 * the requests that change it - only in a tab paired with the computer its
	 * Served by names, since requests stage on this capability and are
	 * approved there. Waiting requests list inline with Approve / Reject.
	 */
	import { onMount } from "svelte";
	import { capabilityStatusText, listCapabilityRequests, OPEN_REQUEST, resolveCapabilityRequest, stageCapabilityRequest, thisMachineId, type CapabilityRequest } from "$lib/capability-actions";
	import { onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, servedByMachineId, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let { object, onchanged }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	let machines = $state<MachineRow[]>([]);
	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let ready = $state(false);
	let busy = $state(false);
	let actionError = $state("");
	let notice = $state("");
	let requests = $state<CapabilityRequest[]>([]);
	let requestPoll: ReturnType<typeof setInterval> | undefined;
	/** Set after staging: the serving harness receives the request asynchronously, so poll until it shows. */
	let expecting = false;

	const isTemplate = $derived(object.typeKey === "template");
	const key = $derived(fieldStr(object.fields, "key"));
	const status = $derived(fieldStr(object.fields, "status") || "missing");
	const error = $derived(fieldStr(object.fields, "error"));
	const servedBy = $derived(servedByMachineId(object.fields, machines));
	const computer = $derived(servedBy ? machineName(machines, servedBy) : "");
	const canAct = $derived(paired && !!thisMachine && thisMachine === servedBy);
	/** Requests on this capability still waiting, running, or failed. */
	const waiting = $derived(requests.filter((r) => r.objectId === object.id && (OPEN_REQUEST.includes(r.status) || r.status === "failed")));
	const actions = $derived.by((): Array<{ operation: string; label: string }> => {
		switch (status) {
			case "active":
				return [{ operation: "skill.check", label: "Check" }, { operation: "skill.disable", label: "Switch off" }, { operation: "skill.uninstall", label: "Uninstall" }];
			case "disabled":
				return [{ operation: "skill.enable", label: "Switch on" }, { operation: "skill.uninstall", label: "Uninstall" }];
			case "missing":
			case "broken":
				return [{ operation: "skill.install", label: "Install" }];
			default:
				return [];
		}
	});

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
		await loadRequests();
	}

	async function loadRequests() {
		if (!pairedSession()) return;
		try {
			requests = (await listCapabilityRequests()).filter((r) => r.objectId === object.id);
		} catch {
			/* harness unreachable: actions report on click */
			return;
		}
		const open = requests.some((r) => OPEN_REQUEST.includes(r.status));
		if (open) expecting = false;
		if ((open || expecting) && !requestPoll) requestPoll = setInterval(() => void loadRequests().then(() => onchanged().catch(() => {})), 2000);
		if (!open && !expecting && requestPoll) {
			clearInterval(requestPoll);
			requestPoll = undefined;
		}
	}

	async function act(run: () => Promise<void>, done: string) {
		if (busy) return;
		busy = true;
		actionError = "";
		notice = "";
		try {
			await run();
			notice = done;
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
			await loadRequests();
			await onchanged().catch(() => {});
		}
	}

	onMount(() => {
		void Promise.all([
			fetchMachines().then(({ machines: roster }) => (machines = roster)).catch(() => {}),
			loadPairing(),
		]).finally(() => (ready = true));
		const unsubscribe = onPairingChange(() => void loadPairing());
		return () => {
			unsubscribe();
			if (requestPoll) clearInterval(requestPoll);
		};
	});
</script>

<div class="cap-status" data-testid="capability-status">
	<p class="now">{capabilityStatusText(status)}</p>
	{#if error}<p class="error">{error}</p>{/if}
	{#if isTemplate}
		<p class="muted">A template does not install; each capability made from it does.</p>
	{:else if ready}
		{#if canAct}
			{#if actions.length > 0}
				<div class="actions">
					{#each actions as action (action.operation)}
						<button class="act" disabled={busy} data-testid="capability-request-{action.operation.slice('skill.'.length)}" onclick={() => void act(async () => { await stageCapabilityRequest(object.id, key, action.operation); expecting = true; }, "Request staged. Approve it below.")}>{action.label}</button>
					{/each}
				</div>
			{/if}
			{#each waiting as request (request.messageId)}
				<div class="request" data-testid="capability-request">
					<div class="actions">
						<span class="chip">{request.operation} · {request.status.replaceAll("_", " ")}</span>
						{#if request.status === "pending" || request.status === "awaiting_approval"}
							{#if request.canApprove}
								<button class="act" disabled={busy} onclick={() => void act(() => resolveCapabilityRequest(request, "approve"), "Approved.")}>Approve</button>
								<button class="act" disabled={busy} onclick={() => void act(() => resolveCapabilityRequest(request, "reject"), "Rejected.")}>Reject</button>
							{:else}
								<span class="muted">approve on {computer}</span>
							{/if}
						{/if}
					</div>
					{#if request.error}<p class="muted" role="status">{request.error}</p>{/if}
				</div>
			{/each}
		{:else}
			<p class="muted">{servedBy ? `Open Roostr on ${computer} to install or check this capability.` : "This capability has no computer yet."}</p>
			{#if !paired}<PairGate compact onready={() => void loadPairing()} />{/if}
		{/if}
	{/if}
	{#if notice}<p class="muted" role="status">{notice}</p>{/if}
	{#if actionError}<p class="error" role="alert" data-testid="capability-error">{actionError}</p>{/if}
</div>

<style>
	.cap-status { display: flex; flex-direction: column; gap: 8px; min-width: 220px; }
	.now { margin: 0; font-size: 14px; font-weight: 600; color: var(--fg); }
	.muted { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.45; }
	.error { margin: 0; color: #ff6961; font-size: 12.5px; line-height: 1.45; white-space: pre-wrap; }
	.actions { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
	.act { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--fg); }
	.act:hover:not(:disabled) { border-color: var(--accent); }
	.act:disabled { opacity: 0.5; cursor: default; }
	.request { border-top: 1px solid var(--border); padding-top: 6px; display: flex; flex-direction: column; gap: 4px; }
	.chip { font-size: 11px; font-weight: 600; letter-spacing: 0.04em; border-radius: 6px; padding: 2px 8px; color: var(--muted); background: var(--hl-light, rgb(255 255 255 / 0.06)); }
</style>
