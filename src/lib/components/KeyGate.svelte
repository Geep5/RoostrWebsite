<script lang="ts">
	/**
	 * Roostr Web sign-in: the key IS the account. Paste an nsec (or 64-char
	 * hex); everything on the relays is NIP-44 encrypted to it, so a wrong
	 * key simply decrypts nothing. The key never leaves this device.
	 */
	import { saveKey } from "$lib/engine/keys";

	let { onready }: { onready: () => void } = $props();

	let draft = $state("");
	let error = $state("");
	let busy = $state(false);

	function submit() {
		error = "";
		busy = true;
		try {
			saveKey(draft.trim());
			draft = "";
			onready();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="gate">
	<div class="card">
		<img class="logo" src="/logo.png" alt="Roostr" />
		<h1>Roostr</h1>
		<p class="sub">
			Sign in with your Nostr private key. Your notes sync end-to-end encrypted over relays —
			the key never leaves this device, and only it can decrypt your objects.
		</p>
		<form
			onsubmit={(e) => {
				e.preventDefault();
				if (draft.trim()) submit();
			}}
		>
			<input bind:value={draft} type="password" placeholder="nsec1… or 64-char hex" autocomplete="off" />
			<button type="submit" disabled={busy || !draft.trim()}>Open my vault</button>
		</form>
		{#if error}<p class="error">{error}</p>{/if}
		<p class="hint">
			On desktop Roostr: Settings → Reveal private key. New to Roostr? Any fresh key works —
			your vault starts empty and lives wherever your relays are.
		</p>
	</div>
</div>

<style>
	.gate {
		position: fixed;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg, #101216);
		z-index: 500;
	}
	.card {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
		max-width: 420px;
		padding: 36px 32px;
		text-align: center;
	}
	.logo {
		width: 72px;
		height: 72px;
		border-radius: 18px;
	}
	h1 {
		margin: 4px 0 0;
		font-size: 26px;
	}
	.sub {
		color: var(--muted, #9aa0ab);
		font-size: 14px;
		line-height: 1.55;
		margin: 0 0 8px;
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 10px;
		width: 100%;
	}
	input {
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.12);
		border-radius: 10px;
		color: var(--fg, #e8eaed);
		font-size: 14px;
		padding: 11px 14px;
		outline: none;
		text-align: center;
	}
	input:focus {
		border-color: var(--accent, #0a84ff);
	}
	button {
		background: var(--accent, #0a84ff);
		color: #fff;
		border: none;
		border-radius: 10px;
		padding: 11px;
		font-size: 14px;
		font-weight: 600;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.5;
	}
	.error {
		color: var(--red);
		font-size: 13px;
		margin: 0;
	}
	.hint {
		color: var(--muted, #9aa0ab);
		font-size: 12px;
		line-height: 1.5;
		margin: 6px 0 0;
	}
</style>
