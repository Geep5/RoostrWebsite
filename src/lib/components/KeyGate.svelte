<script lang="ts">
	/**
	 * Roostr Web sign-in: the key IS the account. Paste an nsec (or 64-char
	 * hex), or create a new account: a key made here, shown once to save -
	 * there is no reset, since everything on the relays is NIP-44 encrypted
	 * to it and a wrong key simply decrypts nothing. The key never leaves
	 * this device.
	 */
	import { generateSecretKey, nip19 } from "nostr-tools";
	import { saveKey } from "$lib/engine/keys";

	let { onready }: { onready: () => void } = $props();

	let draft = $state("");
	let error = $state("");
	let busy = $state(false);
	/** The new account's key while it is being saved; null on the sign-in form. */
	let created = $state<string | null>(null);
	let saved = $state(false);
	let copied = $state(false);

	function open(key: string) {
		error = "";
		busy = true;
		try {
			saveKey(key);
			draft = "";
			created = null;
			onready();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}

	function create() {
		error = "";
		saved = false;
		copied = false;
		created = nip19.nsecEncode(generateSecretKey());
	}

	async function copy() {
		if (!created) return;
		try {
			await navigator.clipboard.writeText(created);
			copied = true;
		} catch {
			error = "Couldn't copy - select the key and copy it yourself.";
		}
	}
</script>

<div class="gate">
	<div class="card">
		<img class="logo" src="/logo.png" alt="Roostr" />
		<h1>Roostr</h1>
		{#if created}
			<p class="sub">
				This key is your new account. Save it somewhere safe, like a password manager - it's the
				only way back into your vault on another device, and nobody can reset it.
			</p>
			<code class="key">{created}</code>
			<button type="button" class="ghost" onclick={() => void copy()}>{copied ? "Copied" : "Copy key"}</button>
			<label class="check">
				<input type="checkbox" bind:checked={saved} />
				I saved my key
			</label>
			<button type="button" disabled={busy || !saved} onclick={() => created && open(created)}>Open my vault</button>
			<button type="button" class="link" onclick={() => (created = null)}>Back</button>
		{:else}
			<p class="sub">
				Sign in with your Nostr private key. Your notes sync end-to-end encrypted over relays —
				the key never leaves this device, and only it can decrypt your objects.
			</p>
			<form
				onsubmit={(e) => {
					e.preventDefault();
					if (draft.trim()) open(draft.trim());
				}}
			>
				<input bind:value={draft} type="password" placeholder="nsec1… or 64-char hex" autocomplete="off" />
				<button type="submit" disabled={busy || !draft.trim()}>Open my vault</button>
			</form>
			<div class="or">or</div>
			<button type="button" class="ghost" onclick={create}>Create a new account</button>
			<p class="hint">Have desktop Roostr? Settings → Reveal private key, and paste it above.</p>
		{/if}
		{#if error}<p class="error">{error}</p>{/if}
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
	button.ghost {
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.14);
		color: var(--fg, #e8eaed);
	}
	button.link {
		background: none;
		color: var(--muted, #9aa0ab);
		font-weight: 400;
		padding: 4px;
	}
	.card > button:not(.link) {
		width: 100%;
	}
	.or {
		color: var(--muted, #9aa0ab);
		font-size: 12px;
	}
	.key {
		width: 100%;
		box-sizing: border-box;
		padding: 12px 14px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.12);
		font-size: 13px;
		line-height: 1.5;
		word-break: break-all;
		user-select: all;
	}
	.check {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14px;
		cursor: pointer;
	}
	.check input {
		margin: 0;
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
