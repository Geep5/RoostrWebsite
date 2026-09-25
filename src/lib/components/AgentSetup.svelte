<script lang="ts">
	/**
	 * An agent object's setup, on its page: which computer serves it, which
	 * kind it is, what that kind needs there, and the kind's details. The
	 * same contract /setup writes at creation, edited one field at a time
	 * through `note.setField`; secrets never land on the agent.
	 */
	import { onMount } from "svelte";
	import { fetchQuery, note } from "$lib/api";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import type { Card } from "$lib/card-shape";
	import { adoptLocally, agentCreateFields, loadKinds, localMachineId as fetchLocalMachineId, sv, type KindCard } from "$lib/agent-kinds";
	import type { ObjectJSON } from "$lib/types";
	import SetupRequirements from "$lib/components/SetupRequirements.svelte";

	let { object, onchanged }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	let machines = $state<MachineRow[]>([]);
	let cards = $state<Card[]>([]);
	let kinds = $state<KindCard[]>([]);
	/** machine_id of the harness this tab is paired with; "" when unpaired or unreachable. */
	let localMachineId = $state("");
	let loadError = $state("");
	let saveError = $state("");
	let satisfied = $state(false);
	let draft = $state<Record<string, string>>({});
	/** Objects whose `agent` field names this one: what it answers for. */
	let answersFor = $state<Array<{ id: string; name: string }>>([]);

	const servedBy = $derived(object.fields["served_by"]?.stringValue ?? "");
	const kindKey = $derived(object.fields["kind"]?.stringValue ?? "");
	const machine = $derived(machines.find((m) => m.machineId === servedBy));
	const kind = $derived(kinds.find((k) => k.card.key === kindKey));
	const runsHere = $derived(!!servedBy && servedBy === localMachineId);

	async function load() {
		try {
			const [{ machines: roster }, { cards: next, kinds: nextKinds }, assigned] = await Promise.all([
				fetchMachines(),
				loadKinds(),
				fetchQuery({ filters: [{ key: "agent", condition: "equal", value: object.id }], limit: 200 }),
			]);
			machines = roster;
			cards = next;
			kinds = nextKinds;
			answersFor = assigned.records.map((r) => ({ id: r.id, name: r.fields["name"]?.stringValue || r.id.slice(0, 8) }));
			loadError = "";
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		}
		localMachineId = await fetchLocalMachineId();
	}

	onMount(() => {
		void load();
		// A harness registers itself and publishes its cards seconds after it
		// starts; the pickers have to notice without a reload.
		const timer = setInterval(() => void load(), 5_000);
		return () => clearInterval(timer);
	});

	async function save(run: () => Promise<unknown>) {
		saveError = "";
		try {
			await run();
			await onchanged();
		} catch (e) {
			saveError = e instanceof Error ? e.message : String(e);
		}
	}

	function chooseMachine(machineId: string) {
		if (machineId === servedBy) return;
		return save(async () => {
			await note.setField(object.id, "served_by", sv(machineId));
			// The harness adopts by served_by; when it is the one this tab is
			// paired with, claim it on the local roster now as well.
			if (localMachineId && machineId === localMachineId) await adoptLocally(object.id);
		});
	}

	function chooseKind(k: KindCard) {
		if (k.card.key === kindKey) return;
		return save(async () => {
			// The kind's fields from the creation contract, minus the computer, which stays as picked.
			for (const [key, value] of Object.entries(agentCreateFields(k.card.key, servedBy, k))) {
				if (key !== "served_by") await note.setField(object.id, key, value);
			}
			draft = {};
		});
	}

	function saveField(key: string) {
		const value = (draft[key] ?? "").trim();
		if (!value || value === (object.fields[key]?.stringValue ?? "")) return;
		return save(() => note.setField(object.id, key, sv(value)));
	}

	const nameOf = (m: MachineRow) => m.name || `${m.machineId.slice(0, 8)}…`;
</script>

<section class="agent-setup" data-testid="agent-setup">
	{#if object.fields["error"]?.stringValue}
		<!-- The holdup reason the harness stamped: why this agent cannot run. -->
		<p class="holdup-banner" role="alert" data-testid="agent-holdup">{object.fields["error"].stringValue}</p>
	{/if}
	<p class="status" data-testid="agent-status">
		{#if machine}
			Runs on <strong>{nameOf(machine)}</strong>{runsHere ? " · this one" : ""}{kind?.agent.model ? ` · model ${kind.agent.model}` : ""}
		{:else if servedBy}
			Runs on <strong>{servedBy.slice(0, 8)}…</strong>{runsHere ? " · this one" : ""}, a computer that has not registered yet.
		{:else}
			Not on any computer yet - pick one below.
		{/if}
	</p>

	<div class="sec">
		<div class="sec-name">Computer</div>
		{#if machines.length === 0}
			<p class="muted">No computer has registered yet. Start <code>./glon-odin serve</code> and the harness on one; it appears here within seconds.</p>
		{/if}
		<div class="choices">
			{#each machines as m (m.machineId)}
				<button class="choice" class:picked={m.machineId === servedBy} data-testid={`agent-machine-${m.machineId}`} onclick={() => void chooseMachine(m.machineId)}>
					<span class="choice-name">{nameOf(m)}{m.machineId === localMachineId ? " · this one" : ""}</span>
					<span class="choice-sub">{m.capabilities.length ? m.capabilities.map((k) => cards.find((c) => c.key === k)?.name ?? k).join(", ") : "no capabilities yet"}</span>
				</button>
			{/each}
		</div>
	</div>

	{#if answersFor.length > 0}
		<div class="sec" data-testid="agent-answers-for">
			<div class="sec-name">Answers for</div>
			<ul class="answers">
				{#each answersFor as o (o.id)}
					<li><a href="/app/object/{o.id}">{o.name}</a></li>
				{/each}
			</ul>
		</div>
	{/if}

	<div class="sec">
		<div class="sec-name">Kind</div>
		{#if kinds.length === 0}
			<p class="muted">No machine has published an agent kind yet. A harness newer than this catalog will.</p>
		{/if}
		<div class="choices">
			{#each kinds as k (k.card.key)}
				<button class="choice" class:picked={k.card.key === kindKey} data-testid={`agent-kind-${k.card.key}`} onclick={() => void chooseKind(k)}>
					<span class="choice-name">{k.card.name}</span>
					<span class="choice-sub">{k.card.description || "no description"}</span>
					{#if k.agent.requires.length || k.agent.model}
						<span class="choice-meta">{k.agent.model ? `model ${k.agent.model}` : ""}{k.agent.model && k.agent.requires.length ? " · " : ""}{k.agent.requires.length ? `needs ${k.agent.requires.map((r) => cards.find((c) => c.key === r)?.name ?? r).join(", ")}` : ""}</span>
					{/if}
				</button>
			{/each}
		</div>
	</div>

	{#if machine && kind}
		<div class="sec">
			<div class="sec-name">Requirements</div>
			<p class="muted">What {kind.card.name} needs on {nameOf(machine)}.{runsHere ? "" : " Approvals and secrets happen on that computer."}</p>
			{#key `${machine.machineId}:${kind.card.key}`}
				<SetupRequirements {machine} requires={kind.agent.requires} {cards} {localMachineId} bind:satisfied />
			{/key}
		</div>
	{/if}

	{#if kind && kind.card.fields.length}
		<div class="sec">
			<div class="sec-name">Details</div>
			<div class="fields">
				{#each kind.card.fields as f (f.key)}
					<label class="field">
						<span>{f.label}</span>
						{#if f.secret}
							<input type="password" disabled value="" placeholder="set under Requirements, on the computer" data-testid={`agent-field-${f.key}`} />
							<span class="note">Secret: stored only on {machine ? nameOf(machine) : "that computer"}, never on the agent.</span>
						{:else}
							<input
								type={f.format === "email" ? "email" : f.format === "url" ? "url" : "text"}
								data-testid={`agent-field-${f.key}`}
								value={draft[f.key] ?? object.fields[f.key]?.stringValue ?? ""}
								oninput={(e) => (draft = { ...draft, [f.key]: e.currentTarget.value })}
								onblur={() => void saveField(f.key)}
								onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); void saveField(f.key); } }}
							/>
							{#if f.note}<span class="note">{f.note}</span>{/if}
						{/if}
					</label>
				{/each}
			</div>
		</div>
	{/if}

	{#if loadError || saveError}<p class="error" role="alert" data-testid="agent-setup-error">{saveError || loadError}</p>{/if}
</section>

<style>
	.agent-setup { display: flex; flex-direction: column; gap: 20px; margin: 16px 0 0 48px; max-width: 560px; }
	@media (max-width: 720px) { .agent-setup { margin: 16px 16px 0; } }
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
	.choice-meta { color: var(--muted); font-size: 11.5px; }
	.fields { display: flex; flex-direction: column; gap: 10px; }
	.field { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: var(--muted); }
	.field input { padding: 8px 10px; border-radius: 6px; border: 1px solid var(--border); background: var(--hover, rgba(255, 255, 255, 0.04)); color: var(--fg); font: inherit; font-size: 14px; }
	.field input:focus { border-color: var(--accent); outline: none; }
	.field input:disabled { opacity: 0.5; }
	.note { color: var(--muted); font-size: 12px; }
	.answers { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 4px; font-size: 13px; }
	.answers a { color: var(--fg); text-decoration: none; }
	.answers a:hover { text-decoration: underline; }
	.error { margin: 0; color: #ff6961; font-size: 13px; }
	.holdup-banner { margin: 0; color: #ff9f0a; background: rgb(255 159 10 / 0.1); border: 1px solid rgb(255 159 10 / 0.35); border-radius: 8px; padding: 8px 12px; font-size: 13px; line-height: 1.5; }
</style>
