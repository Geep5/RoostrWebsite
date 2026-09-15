<script lang="ts">
	import { onMount } from "svelte";
	import { pairLocal, pairedSession, onPairingChange, unpairLocal, LOCAL_API } from "$lib/local-transport";
	import { settings } from "$lib/api";
	let { onready, compact = false }: { onready: () => void; compact?: boolean } = $props();
	let code = $state("");
	let busy = $state(false);
	let error = $state("");
	let session = $state<ReturnType<typeof pairedSession>>(pairedSession());
	let daemonCode = $state<string | null>(null);
	let copied = $state(false);
	let keyDraft = $state("");
	let keyBusy = $state(false);
	let keyError = $state("");
	let keyOpen = $state(false);
	onMount(() => {
		session = pairedSession();
		// The daemon guards outside access only, so it serves its current
		// pairing code to a UI running on this machine - no terminal trip.
		void (async () => {
			try {
				const res = await fetch(`${LOCAL_API}/api/pair/code`, { credentials: "omit", redirect: "error" });
				if (!res.ok) return;
				const value = await res.json();
				if (typeof value.code === "string" && /^[a-f0-9]{64}$/i.test(value.code)) daemonCode = value.code;
			} catch { /* daemon offline or older - the manual form still works */ }
		})();
		return onPairingChange(() => {
			const next = pairedSession();
			if (session && !next) error = "Pairing expired or was removed. Obtain a fresh terminal code to reconnect.";
			session = next;
		});
	});
	async function pair() {
		busy = true;
		error = "";
		try { await pairLocal(code); code = ""; onready(); }
		catch (cause) { error = cause instanceof Error ? cause.message : "Could not reach the local daemon. Start it and try again."; }
		finally { busy = false; }
	}
	async function copyCode() {
		if (!daemonCode) return;
		try {
			await navigator.clipboard.writeText(daemonCode);
			copied = true;
			setTimeout(() => (copied = false), 1600);
		} catch { /* clipboard unavailable - the code is selectable anyway */ }
	}
	async function importIdentity() {
		keyBusy = true;
		keyError = "";
		try { await settings.importKey(keyDraft); } // reloads the page on success
		catch (cause) { keyError = cause instanceof Error ? cause.message : "Key import failed."; }
		finally { keyBusy = false; }
	}
</script>

<section class:compact aria-label="Local daemon pairing">
	<h2>{session ? "Paired with this machine" : "Pair with your local daemon"}</h2>
	{#if session}
		<p>This tab is paired until {new Date(session.expiresAt).toLocaleString()}. The daemon’s private key stays on this machine.</p>
		<div class="row">
			<button class="primary" onclick={() => onready()}>Reconnect</button>
			<button class="ghost" onclick={() => unpairLocal()}>Unpair this tab</button>
		</div>
		<details class="identity" bind:open={keyOpen}>
			<summary>Sign in with a Nostr key instead</summary>
			<p class="hint">
				Paste an nsec (or 64-char hex) to make it this machine's identity — like signing in on the web app.
				This replaces the daemon identity for <strong>every</strong> paired tab; the previous identity's shared-space authority is revoked.
			</p>
			<form onsubmit={(event) => { event.preventDefault(); void importIdentity(); }}>
				<input type="password" bind:value={keyDraft} placeholder="nsec1… or 64-char hex" autocomplete="off" spellcheck="false" disabled={keyBusy} />
				<button class="primary" type="submit" disabled={keyBusy || !keyDraft.trim()}>{keyBusy ? "Importing…" : "Use this key"}</button>
			</form>
			{#if keyError}<p role="alert">{keyError}</p>{/if}
		</details>
	{:else}
		<p>Pairing allows this browser origin to access local data and machine controls for 24 hours. Codes are one-use and expire after five minutes.</p>
		{#if daemonCode}
			<div class="daemon-code">
				<span class="label">This daemon’s current code</span>
				<code>{daemonCode}</code>
				<div class="row">
					<button class="primary" type="button" disabled={busy} onclick={() => { code = daemonCode!; void pair(); }}>
						{busy ? "Pairing…" : "Pair with this code"}
					</button>
					<button class="ghost" type="button" onclick={copyCode}>{copied ? "Copied" : "Copy"}</button>
				</div>
			</div>
		{/if}
		<form onsubmit={(event) => { event.preventDefault(); void pair(); }}>
			<label for="local-pair-code">{daemonCode ? "Or paste a terminal code" : "Terminal pairing code"}</label>
			<input id="local-pair-code" type="password" bind:value={code} autocomplete="off" spellcheck="false" required disabled={busy} />
			<button class="primary" type="submit" disabled={busy || !code.trim()}>{busy ? "Pairing…" : "Pair this tab"}</button>
		</form>
		<p class="hint">No daemon? Open the browser-mode website to work offline with a browser-owned identity. Pairing never uploads or replaces that identity.</p>
	{/if}
	{#if error}<p role="alert">{error}</p>{/if}
</section>

<style>
	section {
		box-sizing: border-box;
		max-width: 480px;
		margin: 14vh auto;
		padding: 30px 32px;
		border: 1px solid #2c2c2e;
		border-radius: 18px;
		background: #161617;
		box-shadow: 0 16px 48px rgba(0, 0, 0, 0.45);
		color: #ececec;
		font: 14px/1.65 system-ui, sans-serif;
	}
	section.compact { margin: 12px 0; max-width: none; padding: 20px; }
	h2 { margin: 0 0 8px; font-size: 19px; font-weight: 600; letter-spacing: -0.01em; }
	p { margin: 0 0 16px; color: #a3a3a8; font-size: 13.5px; }
	.row { display: flex; gap: 10px; }
	.row .primary { flex: 1; }
	form { display: grid; gap: 10px; margin-top: 16px; }
	label { color: #8d8d93; font-size: 12.5px; }
	input {
		min-width: 0;
		box-sizing: border-box;
		width: 100%;
		padding: 10px 12px;
		background: #1e1e20;
		border: 1px solid #353537;
		border-radius: 9px;
		color: inherit;
		font: inherit;
		outline: none;
		transition: border-color 120ms ease, box-shadow 120ms ease;
	}
	input:focus { border-color: #a8873a; box-shadow: 0 0 0 3px rgba(242, 193, 78, 0.12); }
	button {
		padding: 10px 16px;
		border-radius: 9px;
		font: inherit;
		font-weight: 550;
		cursor: pointer;
		transition: background 120ms ease, border-color 120ms ease, color 120ms ease;
	}
	.primary { border: 1px solid #f2c14e; background: #f2c14e; color: #241a05; }
	.primary:hover:not(:disabled) { background: #f7cf6a; border-color: #f7cf6a; }
	.ghost { border: 1px solid #3a3a3d; background: transparent; color: #c9c9cf; }
	.ghost:hover:not(:disabled) { background: #232325; border-color: #4a4a4e; color: #fff; }
	button:disabled { opacity: 0.45; cursor: default; }
	.hint { margin: 16px 0 0; font-size: 12px; color: #7d7d83; }
	.daemon-code {
		display: grid;
		gap: 10px;
		margin: 0 0 4px;
		padding: 14px;
		border: 1px solid #2a2a2c;
		border-radius: 12px;
		background: #1b1b1d;
	}
	.daemon-code .label { color: #8d8d93; font-size: 12px; letter-spacing: 0.02em; }
	.daemon-code code {
		display: block;
		padding: 9px 12px;
		border: 1px solid #262628;
		border-radius: 8px;
		background: #121213;
		color: #e8d9a0;
		font-family: ui-monospace, monospace;
		font-size: 11.5px;
		letter-spacing: 0.02em;
		word-break: break-all;
		user-select: all;
	}
	[role="alert"] { margin: 14px 0 0; color: #ff9d94; }
	.identity { margin-top: 18px; border-top: 1px solid #262628; padding-top: 14px; }
	.identity summary { cursor: pointer; color: #c9c9cf; font-weight: 550; }
	.identity summary:hover { color: #fff; }
</style>
