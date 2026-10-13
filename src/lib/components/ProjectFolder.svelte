<script lang="ts">
	/**
	 * Under an agent's Project folder (`repo_path`): Choose… opens the macOS
	 * folder window on the computer this tab is paired with - offered only
	 * when that is the agent's Served by, since the folder must exist there -
	 * and the folder as that computer sees it (a git repo, a plain folder,
	 * or not there). On another computer the agent's own computer checks it
	 * and says so on the agent's Error.
	 */
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import { harnessFetch } from "$lib/local-transport";
	import { thisMachineId } from "$lib/capability-actions";

	let { object, onsave }: { object: ObjectJSON; onsave: (path: string) => void | Promise<void> } = $props();

	const path = $derived(fieldStr(object.fields, "repo_path").trim());
	const servedBy = $derived(fieldStr(object.fields, "served_by"));
	let here = $state(""); // this tab's computer
	let picking = $state(false);
	let status = $state<{ ok: boolean; text: string } | null>(null);

	$effect(() => {
		void thisMachineId().then((id) => (here = id));
	});
	const local = $derived(!!here && here === servedBy);

	$effect(() => {
		const p = path;
		status = null;
		if (!p || !local) return;
		let live = true;
		void harnessFetch("/workspace/check", { method: "POST", body: JSON.stringify({ path: p }) })
			.then((r) => r.json() as Promise<{ ok: boolean; summary?: string; error?: string }>)
			.then((r) => {
				if (live) status = r.ok ? { ok: true, text: r.summary ?? "" } : { ok: false, text: r.error ?? "Not found" };
			})
			.catch(() => {});
		return () => {
			live = false;
		};
	});

	async function choose() {
		picking = true;
		try {
			const r = (await (await harnessFetch("/workspace/pick", { method: "POST" })).json()) as { path?: string; cancelled?: boolean; error?: string };
			if (r.path) await onsave(r.path);
			else if (r.error) status = { ok: false, text: r.error };
		} finally {
			picking = false;
		}
	}
</script>

<div class="folder">
	{#if local}
		<button class="choose" disabled={picking} onclick={() => void choose()}>{picking ? "Choose in the window that opened…" : "Choose…"}</button>
	{:else if servedBy}
		<p class="note">The folder must be on this agent's computer; it checks it there, and a problem shows on its Error.</p>
	{:else}
		<p class="note">Set Served by first - the folder must be on that computer.</p>
	{/if}
	{#if status}<p class={status.ok ? "ok" : "bad"}>{status.ok ? "✓" : "✗"} {status.text}</p>{/if}
	<p class="note">Optional. When set, the agent works there: it's told the folder is its project (and its git repo), reads its AGENTS.md, and its shell starts there.</p>
</div>

<style>
	.folder {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: 8px;
	}
	.choose {
		align-self: flex-start;
		padding: 4px 10px;
		border-radius: 6px;
		border: 1px solid var(--border, #444);
		background: transparent;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}
	.choose:disabled {
		opacity: 0.6;
		cursor: default;
	}
	p {
		margin: 0;
		font-size: 12px;
		line-height: 1.4;
	}
	.note {
		opacity: 0.6;
	}
	.ok {
		color: #4caf50;
	}
	.bad {
		color: #f55522;
	}
</style>
