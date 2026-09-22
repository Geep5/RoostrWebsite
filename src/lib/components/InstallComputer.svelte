<script lang="ts">
	/**
	 * A space with no computer is inert: agents are minted and answered by a
	 * machine's harness, recurring work fires there, and credentials live
	 * there. Nothing used to say so - a fresh space simply never replied,
	 * which reads as broken rather than as unfinished setup.
	 *
	 * Three states, decided by the DAG, not by a flag:
	 *   no machines at all   → install one (what it does, then how)
	 *   machines, none here  → assign one (writes the space's served_by)
	 *   a machine serves it  → nothing is rendered
	 */
	import { onMount } from "svelte";
	import { fetchObject, note } from "$lib/api";
	import { store, refreshAll } from "$lib/data.svelte";
	import { activeSpace } from "$lib/space.svelte";
	import { fetchMachines, type MachineRow } from "$lib/serving";
	import { loadCards, type Card } from "$lib/cards";
	import { spaceComputer } from "$lib/space-computer";
	import { isLocalBackend } from "$lib/client-backend";

	let machines = $state<MachineRow[]>([]);
	/** The space's default server, read from the channel object itself - the
	 *  same `served_by` the engine's resolver reads. */
	let servedBy = $state("");
	let busy = $state("");
	let error = $state("");
	let dismissed = $state(false);

	const channelId = $derived(activeSpace.id || store.channels[0]?.id || "");
	const decision = $derived(spaceComputer(machines, servedBy));

	/**
	 * Capability names come from the descriptor cards in the vault, which
	 * arrive after first paint - so the lookup is derived state, not a bare
	 * function call, or a key would render raw forever.
	 */
	let cardList = $state<Card[]>([]);
	const labelOf = $derived.by(() => {
		const names = new Map(cardList.map((c) => [c.key, c.name]));
		return (key: string) => names.get(key) ?? key;
	});

	async function load() {
		if (!channelId) return;
		try {
			cardList = await loadCards();
			const [{ machines: rows }, channel] = await Promise.all([fetchMachines(), fetchObject(channelId)]);
			machines = rows;
			servedBy = channel.fields["served_by"]?.stringValue ?? "";
			error = "";
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	onMount(() => {
		void load();
		// A harness registers itself seconds after it starts, so the card has
		// to notice without a reload - that moment is the whole payoff.
		const timer = setInterval(() => void load(), 5_000);
		return () => clearInterval(timer);
	});

	async function assign(machineId: string) {
		if (!channelId) return;
		busy = machineId;
		error = "";
		try {
			await note.setField(channelId, "served_by", { stringValue: machineId });
			await refreshAll();
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = "";
		}
	}
</script>

{#if channelId && decision.state !== "served" && !dismissed}
	<section class="install">
		<div class="ic-head">
			<span class="ic-glyph">🖥️</span>
			<div class="ic-titles">
				<h2>{decision.state === "install" ? "Install a computer on this space" : "Choose the computer for this space"}</h2>
				<p class="ic-sub">
					Agents run on a computer you own: it answers discussions, fires recurring work, and holds the
					logins. Until one is installed, this space stores objects but nothing acts on them.
				</p>
			</div>
			<button class="ic-dismiss" title="Hide for now" onclick={() => (dismissed = true)}>✕</button>
		</div>

		{#if decision.state === "choose"}
			<div class="ic-machines">
				{#each machines as m (m.machineId)}
					<button class="ic-machine" disabled={busy !== ""} onclick={() => void assign(m.machineId)}>
						<span class="ic-name">{m.name || `${m.machineId.slice(0, 8)}…`}</span>
						<span class="ic-caps">
							{m.capabilities.length ? m.capabilities.map(labelOf).join(", ") : "no capabilities yet"}
						</span>
						<span class="ic-go">{busy === m.machineId ? "…" : "Use this one"}</span>
					</button>
				{/each}
			</div>
			<p class="ic-note">Any computer signed in with your key can serve this space. You can change it later in space settings. <a href="/setup">Set up an agent on one, step by step.</a></p>
		{:else}
			<ol class="ic-steps">
				<li>
					On the computer you want to use, install Roostr and start it:
					<code>./glon-odin serve</code> and <code>bun run src/index.ts serve</code> in <code>harness/</code>.
				</li>
				<li>Sign that computer in with <strong>your own key</strong> — same identity, so it joins this space instead of making a new one.</li>
				<li>It registers itself here within seconds, and claims this space automatically if no other computer has.</li>
			</ol>
			<div class="ic-waiting">
				<span class="ic-dot"></span>
				<span>Waiting for a computer to appear…</span>
				{#if isLocalBackend}
					<span class="ic-hint">This browser is paired to a local daemon — start its harness and it will show up here.</span>
				{/if}
			</div>
		{/if}
		{#if error}<p class="ic-err">{error}</p>{/if}
	</section>
{/if}

<style>
	.install {
		border: 1px solid var(--border);
		border-radius: 14px;
		background: var(--panel);
		padding: 14px 16px;
		margin: 0 0 14px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.ic-head {
		display: flex;
		align-items: flex-start;
		gap: 12px;
	}
	.ic-glyph {
		font-size: 22px;
		line-height: 1.1;
		flex: none;
	}
	.ic-titles {
		flex: 1;
		min-width: 0;
	}
	h2 {
		margin: 0 0 4px;
		font-size: 15px;
		font-weight: 600;
	}
	.ic-sub {
		margin: 0;
		color: var(--muted);
		font-size: 13px;
		line-height: 1.45;
	}
	.ic-dismiss {
		flex: none;
		background: none;
		border: 1px solid transparent;
		border-radius: 7px;
		color: var(--muted);
		width: 26px;
		height: 26px;
		cursor: pointer;
	}
	.ic-dismiss:hover {
		color: var(--fg);
		border-color: var(--border);
	}
	.ic-machines {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.ic-machine {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		background: var(--hl-light);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 8px 10px;
		color: inherit;
		cursor: pointer;
		font-size: 13px;
	}
	.ic-machine:hover:enabled {
		border-color: var(--muted);
	}
	.ic-name {
		font-weight: 550;
	}
	.ic-caps {
		flex: 1;
		color: var(--muted);
		font-size: 12px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.ic-go {
		flex: none;
		color: var(--accent);
		font-size: 12px;
	}
	.ic-steps {
		margin: 0;
		padding-left: 20px;
		color: var(--muted);
		font-size: 13px;
		line-height: 1.6;
	}
	.ic-steps code {
		background: var(--hl-light);
		border-radius: 5px;
		padding: 1px 5px;
		font-size: 12px;
		color: var(--fg);
	}
	.ic-note,
	.ic-err {
		margin: 0;
		font-size: 12px;
		color: var(--muted);
	}
	.ic-err {
		color: var(--red);
	}
	.ic-waiting {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 8px;
		font-size: 12px;
		color: var(--muted);
	}
	.ic-hint {
		width: 100%;
	}
	/* The pulse is the only moving thing on the card: it says "still looking",
	   which is the difference between waiting and broken. */
	.ic-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--accent);
		animation: ic-pulse 1.6s ease-in-out infinite;
	}
	@keyframes ic-pulse {
		0%,
		100% {
			opacity: 0.25;
		}
		50% {
			opacity: 1;
		}
	}
</style>
