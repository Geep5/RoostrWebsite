<script lang="ts">
	/**
	 * A File object shown inside a page: images inline, PDFs in the
	 * browser's own viewer, anything else as a card. The block only names
	 * the File (`fileId`, `hash`); the bytes come from this computer's
	 * harness, which fetches them peer-to-peer when it lacks them.
	 */
	import { isLocalBackend } from "$lib/client-backend";
	import { FILES_NEED_LOCAL, fetchFileBlob } from "$lib/files";
	import { store } from "$lib/data.svelte";

	let { meta }: { meta: Record<string, string> } = $props();

	const mime = $derived(meta["mime"] ?? "");
	const name = $derived(store.summaries.find((s) => s.id === meta["fileId"])?.name || meta["name"] || "File");
	const inline = $derived(mime.startsWith("image/") || mime === "application/pdf");

	let url = $state("");
	let error = $state("");
	let attempt = $state(0);

	$effect(() => {
		const hash = meta["hash"] ?? "";
		void attempt;
		if (!isLocalBackend || !inline || !hash) return;
		let cancelled = false;
		let made = "";
		error = "";
		url = "";
		fetchFileBlob(hash)
			.then((blob) => {
				if (cancelled) return;
				made = URL.createObjectURL(blob);
				url = made;
			})
			.catch((err) => {
				if (!cancelled) error = err instanceof Error ? err.message : String(err);
			});
		return () => {
			cancelled = true;
			if (made) URL.revokeObjectURL(made);
		};
	});
</script>

<div class="file-block">
	{#if !isLocalBackend}
		<p class="note">{FILES_NEED_LOCAL}</p>
	{:else if inline && error}
		<p class="err">{error} <button onclick={() => attempt++}>Retry</button></p>
	{:else if inline && !url}
		<p class="note">Loading {name}…</p>
	{:else if mime.startsWith("image/")}
		<img src={url} alt={name} />
	{:else if mime === "application/pdf"}
		<iframe src={url} title={name}></iframe>
	{/if}
	<a class="caption" href="/app/object/{meta['fileId']}" onclick={(e) => e.stopPropagation()}>📎 {name}</a>
</div>

<style>
	.file-block {
		display: flex;
		flex-direction: column;
		gap: 6px;
		width: 100%;
		min-width: 0;
	}
	img {
		max-width: 100%;
		max-height: 70vh;
		border-radius: 8px;
		object-fit: contain;
		align-self: flex-start;
	}
	iframe {
		width: 100%;
		height: 70vh;
		border: 1px solid var(--border);
		border-radius: 8px;
		background: #fff;
	}
	.caption {
		color: var(--muted);
		font-size: 12px;
		text-decoration: none;
		align-self: flex-start;
	}
	.caption:hover {
		color: var(--fg);
	}
	.note {
		color: var(--muted);
		font-size: 13px;
		margin: 0;
	}
	.err {
		color: var(--red);
		font-size: 13px;
		margin: 0;
	}
</style>
