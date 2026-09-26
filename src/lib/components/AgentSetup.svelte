<script lang="ts">
	/**
	 * An agent is an object; its configuration is other objects linked by
	 * properties. Computer is the `served_by` badge, system prompt the
	 * `prompt` badge, credentials the `install` badges - all in the property
	 * row above. "Answers for" is the agent type page's query. This section
	 * keeps only what no property covers: the holdup banner and where it runs.
	 */
	import type { ObjectJSON } from "$lib/types";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import { onMount } from "svelte";

	let { object }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	let machines = $state<MachineRow[]>([]);
	const servedBy = $derived(object.fields["served_by"]?.stringValue ?? "");
	const machine = $derived(machines.find((m) => m.machineId === servedBy));
	const nameOf = (m: MachineRow) => m.name || `${m.machineId.slice(0, 8)}…`;

	onMount(() => {
		void fetchMachines().then((out) => (machines = out.machines)).catch(() => {});
	});
</script>

<section class="agent-setup" data-testid="agent-setup">
	{#if object.fields["error"]?.stringValue}
		<!-- The holdup reason the harness stamped: why this agent cannot run. -->
		<p class="holdup-banner" role="alert" data-testid="agent-holdup">{object.fields.error.stringValue}</p>
	{/if}
	<p class="status" data-testid="agent-status">
		{#if machine}
			Runs on <strong>{nameOf(machine)}</strong>.
		{:else if servedBy}
			Runs on <strong>{servedBy.slice(0, 8)}…</strong>, a computer that has not registered yet.
		{:else}
			Not on any computer yet - set <strong>Computer</strong> in the properties above.
		{/if}
	</p>
</section>

<style>
	.agent-setup { display: flex; flex-direction: column; gap: 12px; margin: 16px 0 0 48px; max-width: 560px; }
	@media (max-width: 720px) { .agent-setup { margin: 16px 16px 0; } }
	.status { margin: 0; font-size: 14px; color: var(--muted); }
	.holdup-banner { margin: 0; color: #ff9f0a; background: rgb(255 159 10 / 0.1); border: 1px solid rgb(255 159 10 / 0.35); border-radius: 8px; padding: 8px 12px; font-size: 13px; line-height: 1.5; }
</style>


