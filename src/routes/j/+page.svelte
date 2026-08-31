<script lang="ts">
	/**
	 * Universal invite landing: getroostr.fly.dev/j#r1.<blob>. The space
	 * key lives in the URL fragment — it never reaches a server. Accepting
	 * imports the key on this device and the space syncs into /app.
	 * Engine imports are dynamic so this page never throws on load.
	 */
	import { onMount } from "svelte";
	import { decodeInvite, type SpaceInvite } from "$lib/invite";

	let checked = $state(false);
	let inv = $state<SpaceInvite | null>(null);
	let hasKey = $state(false);
	let busy = $state(false);
	let accepted = $state(false);
	let error = $state("");

	onMount(async () => {
		inv = decodeInvite(location.hash.slice(1));
		try {
			const sync = await import("$lib/engine/sync");
			hasKey = !!sync.myNpub();
		} catch {
			hasKey = false;
		}
		checked = true;
	});

	async function accept() {
		if (!inv || busy) return;
		busy = true;
		error = "";
		try {
			const sync = await import("$lib/engine/sync");
			await sync.importSpaceInvite(inv);
			accepted = true;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	const appLink = $derived(inv?.object ? `/app/object/${inv.object}` : "/app");
</script>

<svelte:head>
	<title>Join a space — Roostr</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<div class="page">
	<nav>
		<a class="brand" href="/"><img class="logo" src="/logo.png" alt="" /> Roostr</a>
	</nav>

	<main>
		<div class="card">
			{#if !checked}
				<p class="muted">Reading invite…</p>
			{:else if !inv}
				<h1>Invalid invite</h1>
				<p class="muted">This invite link is invalid or truncated.</p>
			{:else if accepted}
				<h1>Invite accepted</h1>
				<p class="muted">Syncing the space — it will appear in your app as changes arrive from the relays.</p>
				<a class="btn primary" href={appLink}>Open Roostr Web</a>
			{:else}
				<h1>Join {inv.name || "a shared space"}</h1>
				<p class="muted">
					You've been invited to a Roostr space{inv.owner ? " by" : ""}
					{#if inv.owner}<code class="npub" title={inv.owner}>{inv.owner.slice(0, 20)}…</code>{/if}
				</p>
				{#if inv.relays.length > 0}
					<p class="relays">via {inv.relays.join(", ")}</p>
				{/if}
				{#if hasKey}
					<button class="btn primary" disabled={busy} onclick={() => void accept()}>
						{busy ? "Accepting…" : "Accept invite"}
					</button>
				{:else}
					<p class="muted">Set up your key in the app first, then reopen this link.</p>
					<a class="btn primary" href="/app">Open Roostr Web</a>
				{/if}
				{#if error}
					<p class="error">{error}</p>
				{/if}
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
	:global(*) {
		box-sizing: border-box;
	}
	.page {
		max-width: 1060px;
		margin: 0 auto;
		padding: 0 24px;
		min-height: 100vh;
		display: flex;
		flex-direction: column;
	}
	nav {
		display: flex;
		align-items: center;
		padding: 22px 0;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 8px;
		font-weight: 700;
		font-size: 17px;
		color: #f5f5f7;
		text-decoration: none;
	}
	.logo {
		width: 22px;
		height: 22px;
	}
	main {
		flex: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px 0 96px;
	}
	.card {
		width: 420px;
		max-width: 100%;
		border: 1px solid #45454a;
		border-radius: 12px;
		padding: 28px;
		background: #2b2b2e;
		box-shadow: 0 18px 50px rgb(0 0 0 / 0.35);
		text-align: center;
		display: flex;
		flex-direction: column;
		gap: 14px;
		align-items: center;
	}
	h1 {
		font-size: 24px;
		letter-spacing: -0.01em;
		margin: 0;
	}
	.muted {
		margin: 0;
		color: #98989d;
		font-size: 14.5px;
		line-height: 1.6;
	}
	.npub {
		font-family: ui-monospace, monospace;
		font-size: 13px;
		color: #f5f5f7;
	}
	.relays {
		margin: 0;
		color: #6e6e73;
		font-size: 12.5px;
		word-break: break-all;
	}
	.btn {
		display: inline-block;
		padding: 10px 24px;
		border-radius: 8px;
		text-decoration: none;
		font-weight: 500;
		font-size: 15px;
		color: #f5f5f7;
		background: #2b2b2e;
		border: 1px solid #45454a;
		cursor: pointer;
	}
	.btn.primary {
		background: #0a84ff;
		border-color: #0a84ff;
		color: #fff;
	}
	.btn.primary:hover {
		background: #3395ff;
	}
	.btn:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.error {
		margin: 0;
		color: #ff6961;
		font-size: 13.5px;
	}
</style>
