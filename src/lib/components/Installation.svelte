<script lang="ts">
	/**
	 * An installation object's page: the row a machine published about one
	 * capability - where it lives, what state it is in - plus the actions
	 * that change it. Staging writes a capability request into this object's
	 * own mailbox (any device can stage); approval executes on the owning
	 * machine after a paired human clicks here.
	 */
	import { onMount } from "svelte";
	import { fetchAllQuery } from "$lib/api";
	import { listCapabilityRequests, resolveCapabilityRequest, stageCapabilityRequest, thisMachineId, type CapabilityRequest } from "$lib/capability-actions";
	import { capabilityLabel, cardFor, loadCards } from "$lib/cards";
	import { pairedSession, onPairingChange } from "$lib/local-transport";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let { object }: { object: ObjectJSON } = $props();

	let machines = $state<MachineRow[]>([]);
	let capabilityId = $state("");
	let loadError = $state("");
	// capabilityLabel reads a non-reactive module cache; bump after loadCards.
	let cardsReady = $state(0);

	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let requests = $state<CapabilityRequest[]>([]);
	let busy = $state("");
	let actionError = $state("");
	let actionNotice = $state("");
	let draft = $state<Record<string, string>>({});
	let requestPoll: ReturnType<typeof setInterval> | undefined;

	const key = $derived(fieldStr(object.fields, "key"));
	const machineId = $derived(fieldStr(object.fields, "machine_id"));
	const status = $derived(fieldStr(object.fields, "status") || "unknown");
	const account = $derived(fieldStr(object.fields, "account"));
	const auth = $derived(fieldStr(object.fields, "auth"));
	const error = $derived(fieldStr(object.fields, "error"));
	const checkedAt = $derived(object.fields["checked_at"]?.intValue ?? 0);

	const machine = $derived(machines.find((m) => m.machineId === machineId));
	const label = $derived.by(() => {
		void cardsReady; // capabilityLabel's cache is a plain module variable
		return key ? capabilityLabel(key) : "Installation";
	});
	const card = $derived.by(() => {
		void cardsReady;
		return key ? cardFor(key) : undefined;
	});
	const isSkill = $derived(card?.kind === "skill");
	const installed = $derived(status !== "" && status !== "unknown" && status !== "missing");
	const statusClass = $derived(
		status === "active" ? "st-active" : status === "broken" || status === "error" ? "st-broken" : status.startsWith("needs") ? "st-needs" : "st-missing",
	);
	/** Requests waiting on this installation, open ones first. */
	const pending = $derived(requests.filter((r) => ["pending", "awaiting_approval", "processing", "failed"].includes(r.status)));

	async function load() {
		try {
			await loadCards();
			cardsReady++;
			const [{ machines: roster }, capabilities] = await Promise.all([fetchMachines(), fetchAllQuery({ type: "capability" })]);
			machines = roster;
			capabilityId = capabilities.find((c) => fieldStr(c.fields, "key") === key)?.id ?? "";
			loadError = "";
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		}
	}

	async function loadRequests() {
		if (!pairedSession()) return;
		try {
			requests = (await listCapabilityRequests()).filter((r) => r.objectId === object.id);
			const open = requests.some((r) => ["pending", "awaiting_approval", "processing"].includes(r.status));
			if (open && !requestPoll) requestPoll = setInterval(() => void loadRequests(), 2000);
			if (!open && requestPoll) {
				clearInterval(requestPoll);
				requestPoll = undefined;
			}
		} catch {
			/* harness unreachable: actions report on click */
		}
	}

	async function stage(operation: string) {
		if (busy) return;
		busy = `stage:${operation}`;
		actionError = "";
		actionNotice = "";
		try {
			await stageCapabilityRequest(key, operation, account);
			actionNotice = "Request staged - it appears below when this machine picks it up.";
			await loadRequests();
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = "";
		}
	}

	async function resolve(request: CapabilityRequest, action: "approve" | "reject" | "finish-login") {
		if (busy) return;
		busy = request.messageId;
		actionError = "";
		try {
			const fields = Object.fromEntries((request.fields ?? []).map((f) => [f.key, draft[`${request.messageId}:${f.key}`] ?? ""]));
			const result = await resolveCapabilityRequest(request, action, fields);
			if (action === "finish-login" && !result.active) actionNotice = "Authentication is not confirmed yet. Finish signing in on that machine, then check again.";
			for (const f of request.fields ?? []) delete draft[`${request.messageId}:${f.key}`];
			await loadRequests();
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = "";
		}
	}

	onMount(() => {
		void load();
		void thisMachineId().then((id) => (thisMachine = id));
		void loadRequests();
		const unsubscribe = onPairingChange(() => {
			paired = pairedSession() !== null;
			if (paired) void thisMachineId().then((id) => (thisMachine = id));
		});
		// The owning machine republishes this row as it checks; the status
		// has to notice without a reload.
		const timer = setInterval(() => void load(), 5_000);
		return () => {
			unsubscribe();
			clearInterval(timer);
			if (requestPoll) clearInterval(requestPoll);
		};
	});
</script>

<section class="installation" data-testid="installation">
	<p class="status-line" data-testid="installation-status">
		<span class="chip {statusClass}">{status.replaceAll("_", " ")}</span>
		<span class="what">{label}{#if account}&nbsp;<span class="muted">({account})</span>{/if}</span>
		{#if machine}
			on <a class="machine-link" href="/app/object/{machine.id}">{machine.name || `${machineId.slice(0, 8)}…`}</a>
		{:else if machineId}
			on <span class="muted">unknown computer {machineId.slice(0, 8)}…</span>
		{/if}
	</p>

	<div class="sec">
		<div class="sec-name">Details</div>
		<dl class="rows">
			{#if capabilityId}
				<div class="row"><dt>Capability</dt><dd><a href="/app/object/{capabilityId}">{label}</a></dd></div>
			{:else if key}
				<div class="row"><dt>Capability</dt><dd>{label}</dd></div>
			{/if}
			{#if machine}
				<div class="row"><dt>Computer</dt><dd><a href="/app/object/{machine.id}">{machine.name || machine.machineId}</a></dd></div>
			{/if}
			{#if account}<div class="row"><dt>Account</dt><dd>{account}</dd></div>{/if}
			{#if auth}<div class="row"><dt>Auth method</dt><dd>{auth.replaceAll("_", " ")}</dd></div>{/if}
			{#if checkedAt}<div class="row"><dt>Last checked</dt><dd>{new Date(checkedAt).toLocaleString()}</dd></div>{/if}
		</dl>
	</div>

	{#if error}
		<div class="sec">
			<div class="sec-name">Last error</div>
			<p class="error" role="alert">{error}</p>
		</div>
	{/if}

	<div class="sec">
		<div class="sec-name">Actions</div>
		<div class="actions">
			{#if isSkill}
				{#if installed}
					<button class="subtle-btn" disabled={!!busy} onclick={() => void stage(status === "active" ? "skill.disable" : "skill.enable")}>{status === "active" ? "Disable" : "Enable"}</button>
				{:else}
					<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("skill.install")}>Request install</button>
				{/if}
				<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("auth.check")}>Request check</button>
				{#if installed}
					<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("skill.uninstall")}>Remove from {machine?.name || "this computer"}</button>
				{/if}
			{:else}
				{#if card?.install?.docsUrl}<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("auth.login")}>Request login</button>{/if}
				{#if (card?.fields.length ?? 0) > 0}<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("auth.save")}>Request key save</button>{/if}
				<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("auth.check")}>Request check</button>
				{#if installed}<button class="subtle-btn" disabled={!!busy} onclick={() => void stage("auth.revoke")}>Request removal</button>{/if}
			{/if}
		</div>
		<p class="muted">Requests stage into this object's mailbox and run on {machine?.name || "the owning computer"} after a paired approval.</p>
	</div>

	{#if pending.length > 0}
		<div class="sec">
			<div class="sec-name">Waiting requests</div>
			{#each pending as request (request.messageId)}
				<div class="request">
					<div class="request-row">
						<span class="chip">{request.operation} · {request.status === "processing" && request.operation === "auth.login" ? "waiting for login" : request.status.replaceAll("_", " ")}</span>
						{#if request.status === "pending" || request.status === "awaiting_approval"}
							{#if request.canApprove}
								<button class="subtle-btn" disabled={!!busy} onclick={() => void resolve(request, "approve")}>{request.operation === "auth.save" ? "Approve & save locally" : "Approve on this computer"}</button>
								<button class="subtle-btn" disabled={!!busy} onclick={() => void resolve(request, "reject")}>Reject</button>
							{:else}
								<span class="muted">approve on {machine?.name || "the owning computer"}</span>
							{/if}
						{:else if request.status === "processing" && request.operation === "auth.login"}
							{#if request.canApprove}
								<button class="subtle-btn" disabled={!!busy} onclick={() => void resolve(request, "finish-login")}>Done signing in — verify</button>
							{:else}
								<span class="muted">finish signing in on {machine?.name || "the owning computer"}</span>
							{/if}
						{:else if request.status === "failed" && request.canApprove}
							<button class="subtle-btn" disabled={!!busy} onclick={() => void stage(request.operation)}>Stage a new request</button>
						{/if}
					</div>
					{#if request.error}<p class="muted" role="status">{request.error}</p>{/if}
					{#if (request.status === "pending" || request.status === "awaiting_approval") && request.operation === "auth.save" && request.canApprove}
						<p class="muted">These values go only to that machine's local store, never into the request or object history.</p>
						{#each request.fields ?? [] as field (field.key)}
							<label class="cred-field">
								<span>{field.label}</span>
								<input type="password" autocomplete="off" value={draft[`${request.messageId}:${field.key}`] ?? ""} oninput={(e) => (draft[`${request.messageId}:${field.key}`] = e.currentTarget.value)} />
							</label>
						{/each}
					{/if}
				</div>
			{/each}
			{#if !paired}
				<PairGate compact onready={() => void thisMachineId().then((id) => (thisMachine = id))} />
			{/if}
		</div>
	{/if}

	<p class="muted">Requests stage into this object's mailbox and run on the owning computer after a paired approval.</p>
</section>


<style>
	.installation { display: flex; flex-direction: column; gap: 20px; margin: 16px 0 0 48px; max-width: 560px; }
	@media (max-width: 720px) { .installation { margin: 16px 16px 0; } }
	.status-line { margin: 0; font-size: 14px; color: var(--fg); display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
	.what { font-weight: 600; }
	.chip { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; border-radius: 6px; padding: 2px 8px; }
	.st-active { color: #30d158; background: rgb(48 209 88 / 0.12); }
	.st-missing { color: var(--muted); background: var(--hl-light, rgb(255 255 255 / 0.06)); }
	.st-needs { color: #ff9f0a; background: rgb(255 159 10 / 0.12); }
	.st-broken { color: #ff6961; background: rgb(255 105 97 / 0.12); }
	.sec { display: flex; flex-direction: column; gap: 8px; }
	.sec-name { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
	.rows { margin: 0; display: flex; flex-direction: column; gap: 6px; }
	.row { display: flex; gap: 12px; font-size: 14px; }
	.row dt { flex: none; width: 110px; color: var(--muted); }
	.row dd { margin: 0; color: var(--fg); }
	a { color: var(--fg); }
	a:hover { color: var(--accent); }
	.muted { margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; }
	.error { margin: 0; color: #ff6961; font-size: 13px; line-height: 1.5; white-space: pre-wrap; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.subtle-btn { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--muted); }
	.subtle-btn:hover:not(:disabled) { border-color: var(--accent); }
	.subtle-btn:disabled { opacity: 0.5; cursor: default; }
	.request { border-top: 1px solid var(--border); padding: 8px 0 6px; display: flex; flex-direction: column; gap: 6px; }
	.request:first-of-type { border-top: none; }
	.request-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
	.cred-field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); }
	.cred-field input { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 6px 10px; color: var(--fg); font: inherit; }
</style>
