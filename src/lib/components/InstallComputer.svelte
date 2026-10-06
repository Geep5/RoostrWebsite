<script lang="ts">
	/**
	 * Without a computer nothing acts: agents are minted and answered by a
	 * machine's harness, recurring work fires there, and credentials live
	 * there. Nothing used to say so - a fresh space simply never replied,
	 * which reads as broken rather than as unfinished setup.
	 *
	 * Shown only while no machine has registered at all. Once one has, each
	 * agent names the computer it runs on in its own Served by property.
	 */
	import { onMount } from "svelte";
	import { fetchMachines } from "$lib/serving";
	import { isLocalBackend } from "$lib/client-backend";

	let machineCount = $state(0);
	/** The first roster read has landed: until then the card stays hidden. */
	let loaded = $state(false);
	let error = $state("");
	let dismissed = $state(false);

	async function load() {
		try {
			machineCount = (await fetchMachines()).machines.length;
			error = "";
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loaded = true;
		}
	}

	onMount(() => {
		void load();
		// A harness registers itself seconds after it starts, so the card has
		// to notice without a reload - that moment is the whole payoff.
		const timer = setInterval(() => void load(), 5_000);
		return () => clearInterval(timer);
	});
</script>

{#if loaded && machineCount === 0 && !dismissed}
	<section class="install">
		<div class="ic-head">
			<span class="ic-glyph">🖥️</span>
			<div class="ic-titles">
				<h2>Install a computer</h2>
				<p class="ic-sub">
					Agents run on a computer you own: it answers discussions, fires recurring work, and holds the
					logins. Until one is installed, this space stores objects but nothing acts on them.
				</p>
			</div>
			<button class="ic-dismiss" title="Hide for now" onclick={() => (dismissed = true)}>✕</button>
		</div>

		<ol class="ic-steps">
			<li>
				On the computer you want to use, install Roostr and start it as a service, so it runs at boot
				and comes back if it crashes: <code>bun run service install</code> in <code>harness/</code>.
			</li>
			<li>Sign that computer in with <strong>your own key</strong> — same identity, so it joins this space instead of making a new one.</li>
			<li>It registers itself here within seconds. Then pick it in each agent's <strong>Served by</strong> property.</li>
		</ol>
		<div class="ic-waiting">
			<span class="ic-dot"></span>
			<span>Waiting for a computer to appear…</span>
			{#if isLocalBackend}
				<span class="ic-hint">This browser is paired to a local daemon — start its harness and it will show up here.</span>
			{/if}
		</div>
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
	.ic-err {
		margin: 0;
		font-size: 12px;
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
