<script lang="ts">
	/**
	 * /setup - put an agent on a computer, in order.
	 *
	 * Pair → choose the computer → choose the kind → satisfy what the kind
	 * requires on that computer → fill the kind's fields → create. Every step
	 * reads the DAG the way the app does (machine objects, descriptor cards,
	 * install rows) and writes through the same calls, so the result is one
	 * ordinary `agent` object the chosen machine's harness adopts by `served_by`.
	 * Secret values never touch the agent: they go to the machine's credential
	 * store through the requirements step.
	 */
	import { onMount } from "svelte";
	import { fetchChannels, note } from "$lib/api";
	import { backend, isLocalBackend } from "$lib/client-backend";
	import { pairedSession, onPairingChange } from "$lib/local-transport";
	import { loadKey } from "$lib/engine/keys";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import type { Card } from "$lib/card-shape";
	import { adoptLocally, agentCreateFields, loadKinds, localMachineId as fetchLocalMachineId, sv, type KindCard } from "$lib/agent-kinds";
	import type { SpaceJSON, ValueJSON } from "$lib/types";
	import PairGate from "$lib/components/PairGate.svelte";
	import KeyGate from "$lib/components/KeyGate.svelte";
	import SetupRequirements from "$lib/components/SetupRequirements.svelte";

	const STEPS = ["pair", "computer", "kind", "requirements", "fields", "done"] as const;
	type Step = (typeof STEPS)[number];
	const TITLES: Record<Step, string> = { pair: "Pair", computer: "Computer", kind: "Kind", requirements: "Requirements", fields: "Details", done: "Done" };

	let step = $state<Step>("pair");
	let ready = $state(false);
	let booting = $state(false);
	let bootError = $state("");
	let loadError = $state("");

	let machines = $state<MachineRow[]>([]);
	let cards = $state<Card[]>([]);
	let kinds = $state<KindCard[]>([]);
	let spaces = $state<SpaceJSON[]>([]);
	/** machine_id of the harness this tab is paired with; "" when unpaired or unreachable. */
	let localMachineId = $state("");

	let machineId = $state("");
	let kindKey = $state("");
	let satisfied = $state(false);
	let draft = $state<Record<string, string>>({});
	let name = $state("");
	let spaceId = $state("");
	let creating = $state(false);
	let createError = $state("");
	let createdId = $state("");
	let runsHere = $state(false);

	const machine = $derived(machines.find((m) => m.machineId === machineId));
	const kind = $derived(kinds.find((k) => k.card.key === kindKey));
	const stepIndex = $derived(STEPS.indexOf(step));

	async function load() {
		try {
			const [{ machines: roster }, { cards: next, kinds: nextKinds }, channels] = await Promise.all([fetchMachines(), loadKinds(), fetchChannels()]);
			machines = roster;
			cards = next;
			kinds = nextKinds;
			// Oldest first: the first channel is the default space owning unstamped objects.
			spaces = channels.slice().sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
			if (!spaceId && spaces[0]) spaceId = spaces[0].id;
			if (!kindKey && kinds.length) kindKey = (kinds.find((k) => k.card.key === "assistant") ?? kinds[0]).card.key;
			loadError = "";
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
		}
		localMachineId = await fetchLocalMachineId();
	}

	async function boot() {
		if (booting) return;
		booting = true;
		bootError = "";
		try {
			await backend.start();
			ready = true;
			await load();
			if (step === "pair") step = "computer";
		} catch (e) {
			bootError = e instanceof Error ? e.message : String(e);
			backend.stop();
		} finally {
			booting = false;
		}
	}

	onMount(() => {
		if (isLocalBackend ? pairedSession() : loadKey()) void boot();
		// A harness registers itself and publishes its cards seconds after it
		// starts; the picker has to notice without a reload.
		const timer = setInterval(() => { if (ready && step !== "done") void load(); }, 5_000);
		const offPairing = onPairingChange(() => {
			if (!isLocalBackend || pairedSession()) return;
			ready = false;
			step = "pair";
			bootError = "Pairing expired or was removed. Enter a fresh terminal code to reconnect.";
			backend.stop();
		});
		return () => {
			clearInterval(timer);
			offPairing();
			backend.stop();
		};
	});

	function chooseKind(key: string) {
		kindKey = key;
		draft = {};
	}

	function next() {
		const i = STEPS.indexOf(step);
		if (i < STEPS.length - 1) step = STEPS[i + 1];
	}
	function back() {
		const i = STEPS.indexOf(step);
		if (i > 1) step = STEPS[i - 1];
	}

	const canAdvance = $derived.by(() => {
		if (step === "computer") return !!machine;
		if (step === "kind") return !!kind;
		if (step === "requirements") return satisfied || !kind?.agent.requires.length;
		return false;
	});

	/** Contract 2: one `agent` object; secret FieldSpec values are never written here. */
	async function create() {
		if (!machine || !kind || creating) return;
		creating = true;
		createError = "";
		try {
			const fields: Record<string, ValueJSON> = agentCreateFields(kind.card.key, machine.machineId, kind);
			if (spaceId) fields.channel = sv(spaceId);
			for (const f of kind.card.fields) {
				if (f.secret) continue;
				const value = (draft[f.key] ?? "").trim();
				if (value) fields[f.key] = sv(value);
			}
			const { id } = await note.create(name.trim() || kind.card.name, "agent", fields);
			createdId = id;
			// The harness adopts by served_by; when that harness is the one this
			// tab is paired with, claim it on the local roster now as well.
			runsHere = localMachineId !== "" && localMachineId === machine.machineId && (await adoptLocally(id));
			step = "done";
		} catch (e) {
			createError = e instanceof Error ? e.message : String(e);
		} finally {
			creating = false;
		}
	}

	const inputType = (format: string) => (format === "email" ? "email" : format === "url" ? "url" : "text");
</script>

<svelte:head>
	<title>Set up an agent — Roostr</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<div class="page">
	<nav>
		<a class="brand" href="/"><img class="logo" src="/logo.png" alt="" /> Roostr</a>
		<a class="nav-link" href="/app">Open the app</a>
	</nav>

	<main>
		<div class="card" data-testid="setup" data-step={step}>
			<ol class="stepper" aria-label="Setup steps">
				{#each STEPS as s, i (s)}
					<li class:current={s === step} class:past={i < stepIndex} data-testid={`setup-step-${s}`}>{TITLES[s]}</li>
				{/each}
			</ol>

			{#if step === "pair"}
				{#if booting}
					<p class="muted" role="status">Opening your vault…</p>
				{:else if isLocalBackend}
					<PairGate compact onready={() => void boot()} />
				{:else}
					<KeyGate onready={() => void boot()} />
				{/if}
				{#if bootError}<p class="error" role="alert" data-testid="setup-error">{bootError}</p>{/if}
			{:else if step === "computer"}
				<h1>Which computer runs it?</h1>
				<p class="muted">Agents run on a computer you own: it answers, fires recurring work, and holds the logins.</p>
				{#if machines.length === 0}
					<p class="muted">No computer has registered yet. Start <code>./glon-odin serve</code> and the harness on one; it appears here within seconds.</p>
				{/if}
				<div class="choices">
					{#each machines as m (m.machineId)}
						<button class="choice" class:picked={m.machineId === machineId} data-testid={`setup-machine-${m.machineId}`} onclick={() => (machineId = m.machineId)}>
							<span class="choice-name">{m.name || `${m.machineId.slice(0, 8)}…`}{m.machineId === localMachineId ? " · this one" : ""}</span>
							<span class="choice-sub">{m.capabilities.length ? m.capabilities.map((k) => cards.find((c) => c.key === k)?.name ?? k).join(", ") : "no capabilities yet"}</span>
						</button>
					{/each}
				</div>
			{:else if step === "kind"}
				<h1>What kind of agent?</h1>
				{#if kinds.length === 0}
					<p class="muted">No machine has published an agent kind yet. A harness newer than this catalog will.</p>
				{/if}
				<div class="choices">
					{#each kinds as k (k.card.key)}
						<button class="choice" class:picked={k.card.key === kindKey} data-testid={`setup-kind-${k.card.key}`} onclick={() => chooseKind(k.card.key)}>
							<span class="choice-name">{k.card.name}</span>
							<span class="choice-sub">{k.card.description || "no description"}</span>
							{#if k.agent.requires.length || k.agent.model}
								<span class="choice-meta">{k.agent.model ? `model ${k.agent.model}` : ""}{k.agent.model && k.agent.requires.length ? " · " : ""}{k.agent.requires.length ? `needs ${k.agent.requires.map((r) => cards.find((c) => c.key === r)?.name ?? r).join(", ")}` : ""}</span>
							{/if}
						</button>
					{/each}
				</div>
			{:else if step === "requirements" && machine && kind}
				<h1>What {kind.card.name} needs on {machine.name || "that computer"}</h1>
				<p class="muted">Each item must be ready on that computer before the agent can do its work.</p>
				{#key `${machine.machineId}:${kind.card.key}`}
					<SetupRequirements {machine} requires={kind.agent.requires} {cards} {localMachineId} bind:satisfied />
				{/key}
			{:else if step === "fields" && machine && kind}
				<h1>Details</h1>
				<form class="fields" onsubmit={(e) => { e.preventDefault(); void create(); }}>
					<label class="field">
						<span>Name</span>
						<input type="text" data-testid="setup-name" placeholder={kind.card.name} bind:value={name} />
					</label>
					{#if spaces.length > 0}
						<label class="field">
							<span>Space</span>
							<select data-testid="setup-space" bind:value={spaceId}>
								{#each spaces as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
							</select>
						</label>
					{/if}
					{#each kind.card.fields as f (f.key)}
						<label class="field">
							<span>{f.label}</span>
							{#if f.secret}
								<input type="password" disabled value="" placeholder="set under Requirements, on the computer" data-testid={`setup-field-${f.key}`} />
								<span class="note">Secret: stored only on {machine.name || "that computer"}, never on the agent.</span>
							{:else}
								<input type={inputType(f.format)} data-testid={`setup-field-${f.key}`} value={draft[f.key] ?? ""} oninput={(e) => (draft = { ...draft, [f.key]: e.currentTarget.value })} />
								{#if f.note}<span class="note">{f.note}</span>{/if}
							{/if}
						</label>
					{/each}
					<p class="muted small">Runs on {machine.name || machine.machineId}{kind.agent.model ? ` · model ${kind.agent.model}` : ""}{kind.agent.requires.length ? ` · needs ${kind.agent.requires.join(", ")}` : ""}</p>
					{#if createError}<p class="error" role="alert" data-testid="setup-error">{createError}</p>{/if}
					<button class="btn primary" type="submit" disabled={creating} data-testid="setup-create">{creating ? "Creating…" : "Create agent"}</button>
				</form>
			{:else if step === "done"}
				<h1>{name.trim() || kind?.card.name || "Agent"} is set up</h1>
				<p class="muted">
					It runs on {machine?.name || "the chosen computer"}{runsHere ? " and is on this machine's roster now" : ""}. The harness there adopts it on its next pass.
				</p>
				<a class="btn primary" href={`/app/object/${createdId}`} data-testid="setup-done-link">Open the agent</a>
				<button class="btn" onclick={() => { createdId = ""; name = ""; draft = {}; step = "computer"; }}>Set up another</button>
			{/if}

			{#if loadError && step !== "pair"}<p class="error" role="alert" data-testid="setup-error">{loadError}</p>{/if}

			{#if step !== "pair" && step !== "done" && step !== "fields"}
				<div class="nav-row">
					<button class="btn" disabled={stepIndex <= 1} data-testid="setup-back" onclick={back}>Back</button>
					<button class="btn primary" disabled={!canAdvance} data-testid="setup-next" onclick={next}>Continue</button>
				</div>
			{:else if step === "fields"}
				<div class="nav-row">
					<button class="btn" data-testid="setup-back" onclick={back}>Back</button>
				</div>
			{/if}
		</div>
	</main>
</div>

<style>
	:global(body) {
		margin: 0;
		background: #1e1e20;
		color: #f5f5f7;
		font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", Inter, sans-serif;
		-webkit-font-smoothing: antialiased;
	}
	:global(*) { box-sizing: border-box; }
	.page { max-width: 1060px; margin: 0 auto; padding: 0 24px; min-height: 100vh; display: flex; flex-direction: column; }
	nav { display: flex; align-items: center; justify-content: space-between; padding: 22px 0; }
	.brand { display: flex; align-items: center; gap: 8px; font-weight: 700; font-size: 17px; color: #f5f5f7; text-decoration: none; }
	.nav-link { color: #98989d; text-decoration: none; font-size: 14px; }
	.nav-link:hover { color: #f5f5f7; }
	.logo { width: 22px; height: 22px; }
	main { flex: 1; display: flex; align-items: flex-start; justify-content: center; padding: 24px 0 96px; }
	.card { width: 560px; max-width: 100%; border: 1px solid #45454a; border-radius: 12px; padding: 28px; background: #2b2b2e; box-shadow: 0 18px 50px rgb(0 0 0 / 0.35); display: flex; flex-direction: column; gap: 14px; }
	.stepper { display: flex; gap: 6px; list-style: none; margin: 0 0 6px; padding: 0; font-size: 12px; color: #6e6e73; flex-wrap: wrap; }
	.stepper li { padding: 3px 9px; border-radius: 999px; border: 1px solid #3a3a3e; }
	.stepper li.past { color: #98989d; border-color: #45454a; }
	.stepper li.current { color: #fff; background: #0a84ff; border-color: #0a84ff; }
	h1 { font-size: 22px; letter-spacing: -0.01em; margin: 0; }
	.muted { margin: 0; color: #98989d; font-size: 14.5px; line-height: 1.6; }
	.muted.small { font-size: 13px; }
	code { font-family: ui-monospace, monospace; font-size: 13px; }
	.choices { display: flex; flex-direction: column; gap: 8px; }
	.choice { text-align: left; display: flex; flex-direction: column; gap: 3px; padding: 12px 14px; border-radius: 10px; border: 1px solid #45454a; background: #1e1e20; color: #f5f5f7; cursor: pointer; }
	.choice:hover { border-color: #6e6e73; }
	.choice.picked { border-color: #0a84ff; box-shadow: 0 0 0 1px #0a84ff inset; }
	.choice-name { font-weight: 600; font-size: 15px; }
	.choice-sub { color: #98989d; font-size: 13px; line-height: 1.4; }
	.choice-meta { color: #6e6e73; font-size: 12px; }
	.fields { display: flex; flex-direction: column; gap: 12px; }
	.field { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: #d0d0d5; }
	.field input, .field select { padding: 9px 10px; border-radius: 6px; border: 1px solid #45454a; background: #1e1e20; color: #f5f5f7; font-size: 14px; }
	.field input:disabled { opacity: 0.5; }
	.note { color: #6e6e73; font-size: 12px; }
	.nav-row { display: flex; justify-content: space-between; gap: 8px; padding-top: 6px; }
	.btn { display: inline-block; padding: 10px 24px; border-radius: 8px; text-decoration: none; font-weight: 500; font-size: 15px; color: #f5f5f7; background: #2b2b2e; border: 1px solid #45454a; cursor: pointer; text-align: center; }
	.btn.primary { background: #0a84ff; border-color: #0a84ff; color: #fff; }
	.btn.primary:hover { background: #3395ff; }
	.btn:disabled { opacity: 0.5; cursor: default; }
	.error { margin: 0; color: #ff6961; font-size: 13.5px; }
</style>
