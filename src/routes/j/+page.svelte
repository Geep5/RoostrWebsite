<script lang="ts">
	/**
	 * Public join landing: getroostr.fly.dev/j#r2.<blob>. The link carries
	 * NO key - it only names a space and its owner. "Request to join"
	 * gift-wraps your npub to the owner (NIP-59); they approve in space
	 * settings, and the space key arrives gift-wrapped back to you.
	 * Engine imports are dynamic so this page never throws on load.
	 */
	import { onMount } from "svelte";
	import { decodeJoinLink, type SpaceJoinLink } from "$lib/invite";

	let checked = $state(false);
	let link = $state<SpaceJoinLink | null>(null);
	let hasKey = $state(false);
	let busy = $state(false);
	let sent = $state(false);
	let error = $state("");

	onMount(async () => {
		link = decodeJoinLink(location.hash.slice(1));
		try {
			const sync = await import("$lib/engine/sync");
			hasKey = !!sync.myNpub();
		} catch {
			hasKey = false;
		}
		checked = true;
	});

	async function request() {
		if (!link || busy) return;
		busy = true;
		error = "";
		try {
			const sync = await import("$lib/engine/sync");
			await sync.sendJoinRequest(link);
			sent = true;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
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
				<p class="muted">Reading link…</p>
			{:else if !link}
				<h1>Invalid link</h1>
				<p class="muted">This join link is invalid or truncated.</p>
			{:else if sent}
				<h1>Request sent</h1>
				<p class="muted">
					The owner of {link.name || "the space"} will see your request in their space settings.
					Once they approve, the space key is delivered to you and the space appears in your app.
				</p>
				<a class="btn primary" href="/app">Open Roostr Web</a>
			{:else}
				<h1>Request to join {link.name || "a shared space"}</h1>
				<p class="muted">
					A Roostr space{link.owner ? " owned by" : ""}
					{#if link.owner}<code class="npub" title={link.owner}>{link.owner.slice(0, 20)}…</code>{/if}
				</p>
				{#if link.relays.length > 0}
					<p class="relays">via {link.relays.join(", ")}</p>
				{/if}
				<p class="muted">This link grants nothing by itself — the owner approves each request.</p>
				{#if hasKey}
					<button class="btn primary" disabled={busy} onclick={() => void request()}>
						{busy ? "Sending…" : "Request to join"}
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
