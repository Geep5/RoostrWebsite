<script lang="ts">
	/**
	 * A capability object's setup, on its page: which computer serves it and
	 * whether the linked install is active there. The computer moves through
	 * `note.setField`; logins and installs are staged as capability requests
	 * (the same mailbox shape This machine sends) and approved there - a
	 * paired click grants execution, the DAG only carries the intent.
	 */
	import { onMount } from "svelte";
	import { fetchAllQuery, mailbox, note, type QueryResultRow } from "$lib/api";
	import { harnessFetch } from "$lib/local-transport";
	import { fetchMachines, machineName, type MachineRow } from "$lib/serving";
	import { loadCards, type Card } from "$lib/cards";
	import type { ObjectJSON } from "$lib/types";

	let { object, onchanged }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	let machines = $state<MachineRow[]>([]);
	let cards = $state<Card[]>([]);
	let installs = $state<QueryResultRow[]>([]);
	let loadError = $state("");
	let saveError = $state("");
	let requestBusy = $state("");
	let requestError = $state("");
	let requestNotice = $state("");

	const key = $derived(object.fields["key"]?.stringValue ?? "");
	const servedBy = $derived(object.fields["served_by"]?.stringValue ?? "");
	/** The install this capability points at; falling back to key + machine. */
	const installId = $derived(object.fields["install"]?.linkValue?.targetId ?? object.fields["install"]?.stringValue ?? "");
	const install = $derived(
		installs.find((r) => r.id === installId) ??
			installs.find((r) => r.fields["key"]?.stringValue === key && !!servedBy && r.fields["machine_id"]?.stringValue === servedBy),
	);
	const status = $derived(install?.fields["status"]?.stringValue ?? "");
	const isActive = $derived(status === "active");

	async function load() {
		try {
			const [{ machines: roster }, nextCards, installRows] = await Promise.all([fetchMachines(), loadCards(), fetchAllQuery({ type: "install" })]);
			machines = roster;
			cards = nextCards;
			installs = installRows;
			loadError = "";
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		}
	}

	onMount(() => {
		void load();
		// The harness publishes installs and machines seconds after it starts;
		// the status line and picker have to notice without a reload.
		const timer = setInterval(() => void load(), 5_000);
		return () => clearInterval(timer);
	});

	async function chooseMachine(machineId: string) {
		if (machineId === servedBy) return;
		saveError = "";
		try {
			await note.setField(object.id, "served_by", { stringValue: machineId });
			await onchanged();
		} catch (e) {
			saveError = e instanceof Error ? e.message : String(e);
		}
	}

	/** The DAG carries only the intent. A separate paired click grants execution. */
	async function requestOperation(operation: string): Promise<boolean> {
		if (requestBusy) return false;
		requestBusy = `stage:${key}:${operation}`;
		requestError = "";
		requestNotice = "";
		try {
			const res = await harnessFetch("/machine");
			if (!res.ok) throw new Error("Cannot identify the owning machine.");
			const machine = (await res.json()) as { id: string };
			const [installRows, machineRows] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "machine" })]);
			const installationId = installRows.find((row) => row.fields["key"]?.stringValue === key && row.fields["machine_id"]?.stringValue === machine.id && (row.fields["account"]?.stringValue ?? "") === "")?.id;
			if (!installationId) throw new Error("This machine has not published the installation object yet.");
			const source = machineRows.find((row) => row.fields["machine_id"]?.stringValue === machine.id);
			if (!source) throw new Error("This machine has not published its object yet.");
			await mailbox.send({ id: crypto.randomUUID(), exchangeId: crypto.randomUUID(), sender: { objectId: source.id, agentId: "" }, recipients: [{ objectId: installationId, agentId: "" }], text: `Request ${operation} for ${key}.`, replyTo: "", sentAt: Date.now(), title: "Capability request", requestReply: true, historical: false, operation, author: "" });
			requestNotice = "Request sent to its installation object. Approve it under This machine.";
			return true;
		} catch (error) {
			requestError = error instanceof Error ? error.message : "Cannot stage capability request.";
			return false;
		} finally {
			requestBusy = "";
		}
	}

	const nameOf = (m: MachineRow) => m.name || `${m.machineId.slice(0, 8)}…`;
</script>

<section class="capability-setup" data-testid="capability-setup">
	<p class="status" data-testid="capability-status">
		{#if install && status}
			{status.replaceAll("_", " ")} on <strong>{machineName(machines, install.fields["machine_id"]?.stringValue ?? servedBy)}</strong> · {key}
		{:else if servedBy}
			Not set up on <strong>{machineName(machines, servedBy)}</strong> yet.
		{:else}
			Not set up - pick a computer below.
		{/if}
	</p>

	<div class="sec">
		<div class="sec-name">Computer</div>
		{#if machines.length === 0}
			<p class="muted">No computer has registered yet. Start <code>./glon-odin serve</code> and the harness on one; it appears here within seconds.</p>
		{/if}
		<div class="choices">
			{#each machines as m (m.machineId)}
				<button class="choice" class:picked={m.machineId === servedBy} data-testid={`capability-machine-${m.machineId}`} onclick={() => void chooseMachine(m.machineId)}>
					<span class="choice-name">{nameOf(m)}</span>
					<span class="choice-sub">{m.capabilities.length ? m.capabilities.map((k) => cards.find((c) => c.key === k)?.name ?? k).join(", ") : "no capabilities yet"}</span>
				</button>
			{/each}
		</div>
	</div>

	<div class="sec">
		<div class="sec-name">Setup</div>
		<div class="actions">
			{#if isActive}
				<button class="subtle-btn" disabled={!!requestBusy} data-testid="capability-request-check" onclick={() => void requestOperation("auth.check")}>Request check</button>
			{:else}
				<button class="subtle-btn" disabled={!!requestBusy} data-testid="capability-request-login" onclick={() => void requestOperation("auth.login")}>Request login</button>
				<button class="subtle-btn" disabled={!!requestBusy} data-testid="capability-request-install" onclick={() => void requestOperation("skill.install")}>Request install</button>
			{/if}
		</div>
		<p class="muted">Requests stage here and execute after approval - approve it under This machine.</p>
		{#if requestNotice}<p class="muted" data-testid="capability-request-notice">{requestNotice}</p>{/if}
	</div>

	{#if loadError || saveError || requestError}<p class="error" role="alert" data-testid="capability-setup-error">{requestError || saveError || loadError}</p>{/if}
</section>

<style>
	.capability-setup { display: flex; flex-direction: column; gap: 20px; margin: 16px 0 0 48px; max-width: 560px; }
	@media (max-width: 720px) { .capability-setup { margin: 16px 16px 0; } }
	.sec { display: flex; flex-direction: column; gap: 8px; }
	.sec-name { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
	.status { margin: 0; font-size: 14px; color: var(--fg); }
	.muted { margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; }
	code { font-family: ui-monospace, monospace; font-size: 12px; }
	.choices { display: flex; flex-direction: column; gap: 6px; }
	.choice { text-align: left; display: flex; flex-direction: column; gap: 2px; padding: 9px 12px; border-radius: 10px; border: 1px solid var(--border); background: none; color: var(--fg); cursor: pointer; font: inherit; }
	.choice:hover { border-color: var(--muted); }
	.choice.picked { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent) inset; }
	.choice-name { font-weight: 600; font-size: 14px; }
	.choice-sub { color: var(--muted); font-size: 12.5px; line-height: 1.4; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.subtle-btn { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--muted); }
	.subtle-btn:hover:not(:disabled) { border-color: var(--accent); }
	.subtle-btn:disabled { opacity: 0.5; cursor: default; }
	.error { margin: 0; color: #ff6961; font-size: 13px; }
</style>
