<script lang="ts">
	// ── Machines ───────────────────────────────────────────────────
	//
	// The roster from the DAG, what each device can do, and what the
	// engine routes to it: space defaults, pinned objects, capability
	// needs. Shared by the machine modal and space settings.
	//
	// Serving is resolved client-side by the engine, one call per
	// candidate: every space (its default) plus every object carrying a
	// pin or a capability need. Anything else follows its space and is
	// not listed.
	import { onMount } from "svelte";
	import { fetchAllQuery, type QueryResultRow } from "$lib/api";
	import { UNSERVED_TYPES, capabilityLabel, fetchMachines, resolveMany, type MachineRow } from "$lib/serving";

	const SERVES_SHOWN = 20;
	interface MachineView extends MachineRow {
		serves: string[];
	}
	let machines = $state<MachineView[] | null>(null);
	let machinesError = $state("");

	async function loadMachines() {
		try {
			const [{ rows, machines: roster }, spaces, pinned, needing] = await Promise.all([
				fetchMachines(),
				fetchAllQuery({ type: "channel" }),
				fetchAllQuery({ filters: [{ key: "served_by", condition: "exists" }] }),
				fetchAllQuery({ filters: [{ key: "requires", condition: "exists" }] }),
			]);
			// Oldest first: the first channel is the default space owning unstamped objects.
			spaces.sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
			const seen = new Set<string>();
			const objects: QueryResultRow[] = [];
			for (const r of [...spaces, ...pinned, ...needing]) {
				if (seen.has(r.id) || (r.typeKey !== "channel" && UNSERVED_TYPES[r.typeKey])) continue;
				seen.add(r.id);
				objects.push(r);
			}
			const resolved = await resolveMany(objects, spaces, rows);
			machines = roster.map((m) => ({
				...m,
				serves: objects.filter((_, i) => resolved[i].machineId === m.machineId).map((o) => o.fields["name"]?.stringValue || "Untitled"),
			}));
			machinesError = "";
		} catch (error) {
			machines = null;
			machinesError = error instanceof Error ? error.message : "Cannot load machines.";
		}
	}

	onMount(() => {
		void loadMachines();
		// Credential/capability edits change what machines can do; the
		// machine modal signals those so the roster reloads in place.
		const onChanged = () => void loadMachines();
		window.addEventListener("roostr:machines-changed", onChanged);
		return () => window.removeEventListener("roostr:machines-changed", onChanged);
	});
</script>

<div class="machines">
	<h3>Machines</h3>
	{#if machinesError}
		<p class="hint" role="alert">{machinesError}</p>
	{:else if machines === null}
		<p class="hint">Loading machines…</p>
	{:else if machines.length === 0}
		<p class="hint">No machine has published itself yet — run the harness on a device to add one.</p>
	{:else}
		<p class="hint">Every device running a harness, what it can do, and what the engine routes to it: space defaults, pinned objects, and capability needs. Pin or edit needs from any object's header.</p>
		{#each machines as m (m.id)}
			<div class="machine">
				<div class="machine-row">
					<span class="machine-name">🖥️ {m.name || `${m.machineId.slice(0, 8)}…`}</span>
					{#each m.capabilities as c (c)}
						<span class="chip on">{capabilityLabel(c)}</span>
					{/each}
					{#if m.capabilities.length === 0}
						<span class="chip">no capabilities</span>
					{/if}
				</div>
				<p class="machine-serves">
					{#if m.serves.length === 0}
						serves nothing
					{:else}
						serves {m.serves.length}: {m.serves.slice(0, SERVES_SHOWN).join(", ")}{#if m.serves.length > SERVES_SHOWN} +{m.serves.length - SERVES_SHOWN} more{/if}
					{/if}
				</p>
			</div>
		{/each}
	{/if}
</div>

<style>
	.hint {
		font-size: 12px;
		color: var(--muted);
		line-height: 1.5;
	}
	.chip {
		font-size: 11px;
		padding: 2px 8px;
		border-radius: 10px;
		background: var(--hover, #2a2a2a);
		color: var(--muted);
	}
	.chip.on {
		background: rgb(48 209 88 / 0.16);
		color: var(--green);
	}
	.machine {
		border-top: 1px solid var(--border);
		padding: 8px 0 6px;
	}
	.machine:first-of-type {
		border-top: none;
	}
	.machine-row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 6px;
	}
	.machine-name {
		font-size: 13px;
		font-weight: 600;
		margin-right: 4px;
	}
	.machine-serves {
		margin: 3px 0 0;
		font-size: 12px;
		color: var(--muted);
		line-height: 1.45;
	}
</style>
