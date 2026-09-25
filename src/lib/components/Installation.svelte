<script lang="ts">
	/**
	 * An installation object's page: the row a machine published about one
	 * capability - where it lives, what state it is in, and when it was
	 * last checked. Read-only: state changes are staged as capability
	 * requests and approved under This machine, never edited here.
	 */
	import { onMount } from "svelte";
	import { fetchAllQuery } from "$lib/api";
	import { capabilityLabel, loadCards } from "$lib/cards";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";

	let { object }: { object: ObjectJSON } = $props();

	let machines = $state<MachineRow[]>([]);
	let capabilityId = $state("");
	let loadError = $state("");
	// capabilityLabel reads a non-reactive module cache; bump after loadCards.
	let cardsReady = $state(0);

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
	const statusClass = $derived(
		status === "active" ? "st-active" : status === "broken" || status === "error" ? "st-broken" : status.startsWith("needs") ? "st-needs" : "st-missing",
	);
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

	onMount(() => {
		void load();
		// The owning machine republishes this row as it checks; the status
		// has to notice without a reload.
		const timer = setInterval(() => void load(), 5_000);
		return () => clearInterval(timer);
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

	<p class="muted">State changes are staged from the capability's page or This machine, and run after a paired approval.</p>
	{#if loadError}<p class="error" role="alert" data-testid="installation-error">{loadError}</p>{/if}
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
</style>
