<script lang="ts">
	import { onMount } from "svelte";
	import { pairLocal, pairedSession, onPairingChange, unpairLocal, LOCAL_API } from "$lib/local-transport";
	let { onready, compact = false }: { onready: () => void; compact?: boolean } = $props();
	let code = $state("");
	let busy = $state(false);
	let error = $state("");
	let session = $state<ReturnType<typeof pairedSession>>(null);
	let daemonCode = $state<string | null>(null);
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
</script>

<section class:compact aria-label="Local daemon pairing">
	<h2>{session ? "Paired with this machine" : "Pair with your local daemon"}</h2>
	{#if session}
		<p>This tab is paired until {new Date(session.expiresAt).toLocaleString()}. The daemon’s private key stays on this machine.</p>
		<button onclick={() => onready()}>Reconnect</button>
		<button onclick={() => unpairLocal()}>Unpair this tab</button>
	{:else}
		<p>Pairing explicitly allows this browser origin to access local data and machine controls for 24 hours. Codes are one-use and expire after five minutes.</p>
		{#if daemonCode}
			<div class="daemon-code">
				<span>This daemon&rsquo;s current code</span>
				<code>{daemonCode}</code>
				<button type="button" disabled={busy} onclick={() => { code = daemonCode!; void pair(); }}>{busy ? "Pairing…" : "Pair with this code"}</button>
			</div>
		{/if}
		<form onsubmit={(event) => { event.preventDefault(); void pair(); }}>
			<label for="local-pair-code">Or paste a terminal code</label>
			<input id="local-pair-code" type="password" bind:value={code} autocomplete="off" spellcheck="false" required disabled={busy} />
			<button type="submit" disabled={busy || !code.trim()}>{busy ? "Pairing…" : "Pair this tab"}</button>
		</form>
		<p class="hint">No daemon? Open the browser-mode website to work offline with a browser-owned identity. Pairing never uploads or replaces that identity.</p>
	{/if}
	{#if error}<p role="alert">{error}</p>{/if}
</section>

<style>
	section { box-sizing: border-box; max-width: 520px; margin: 12vh auto; padding: 28px; border: 1px solid #383838; border-radius: 16px; background: #171717; color: #eee; font: 14px/1.6 system-ui, sans-serif; }
	section.compact { margin: 12px 0; max-width: none; padding: 18px; }
	h2 { margin: 0 0 12px; font-size: 20px; }
	p { color: #bbb; }
	form { display: grid; gap: 10px; }
	input { min-width: 0; padding: 10px; background: #222; border: 1px solid #555; border-radius: 6px; color: inherit; font: inherit; }
	button { padding: 9px 14px; border: 1px solid #666; border-radius: 6px; background: #292929; color: inherit; cursor: pointer; }
	button:disabled { opacity: .5; cursor: default; }
	.hint { font-size: 12px; }
	.daemon-code { display: grid; gap: 8px; margin: 0 0 14px; padding: 12px; border: 1px solid #3d3d3d; border-radius: 10px; background: #1d1d1d; }
	.daemon-code span { color: #999; font-size: 12px; }
	.daemon-code code { display: block; padding: 8px 10px; border-radius: 6px; background: #111; color: #e8d9a0; font-size: 12px; word-break: break-all; user-select: all; }
	[role="alert"] { color: #ffaca5; }
</style>
