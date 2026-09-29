<script lang="ts">
	/**
	 * A file object's page. The object carries only facts (name, hash,
	 * size, MIME, which computers hold the bytes); the bytes live in each
	 * computer's harness and move between them peer-to-peer. This tab asks
	 * its paired harness for them, which fetches from a peer first when this
	 * computer lacks them - that can take a few seconds.
	 */
	import { onMount, untrack } from "svelte";
	import { thisMachineId } from "$lib/capability-actions";
	import { isLocalBackend } from "$lib/client-backend";
	import { FILES_NEED_LOCAL, fetchFileBlob, humanSize } from "$lib/files";
	import { objectIcon } from "$lib/icons";
	import { onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let { object, onchanged }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	/** Text previews stop here; the download is always the whole file. */
	const TEXT_PREVIEW_BYTES = 200 * 1024;
	const MACHINE_ICON = objectIcon(undefined, "machine");

	let machines = $state<MachineRow[]>([]);
	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let url = $state("");
	let blobType = $state("");
	let text = $state<string | null>(null);
	let loading = $state(false);
	let fetchError = $state("");
	/** Bumped on every (re)load and on teardown: a slower earlier fetch never lands. */
	let generation = 0;

	const name = $derived(fieldStr(object.fields, "name"));
	const hash = $derived(fieldStr(object.fields, "file_hash").toLowerCase());
	const size = $derived(Number(object.fields["file_size"]?.intValue ?? 0));
	const mime = $derived(fieldStr(object.fields, "file_mime"));
	const objectError = $derived(fieldStr(object.fields, "error"));
	const availableOn = $derived((object.fields["available_on"]?.valuesValue?.items ?? []).map((v) => v.stringValue ?? "").filter(Boolean));
	const holders = $derived(availableOn.map((machineId) => ({ machineId, row: machines.find((m) => m.machineId === machineId), name: machineName(machines, machineId) })));
	/** The bytes come from a peer when this computer is not among the holders. */
	const fetchingFrom = $derived(thisMachine && !availableOn.includes(thisMachine) && holders.length > 0 ? holders.map((h) => h.name).join(" or ") : "");
	const shownType = $derived(mime || blobType);
	const kind = $derived(previewKind(shownType));

	function previewKind(type: string): "image" | "video" | "audio" | "pdf" | "text" | "none" {
		const t = type.toLowerCase().split(";")[0].trim();
		if (t.startsWith("image/")) return "image";
		if (t.startsWith("video/")) return "video";
		if (t.startsWith("audio/")) return "audio";
		if (t === "application/pdf") return "pdf";
		if (t.startsWith("text/") || t === "application/json" || t.endsWith("+json")) return "text";
		return "none";
	}

	function release() {
		if (url) URL.revokeObjectURL(url);
		url = "";
		blobType = "";
		text = null;
	}

	async function load() {
		const mine = ++generation;
		release();
		fetchError = "";
		if (!isLocalBackend || !paired || !hash) {
			loading = false;
			return;
		}
		loading = true;
		try {
			const fetched = await fetchFileBlob(hash);
			// The object's MIME wins over whatever the transfer labelled it,
			// so the browser renders the preview (a PDF iframe needs it).
			const blob = mime && fetched.type !== mime ? new Blob([fetched], { type: mime }) : fetched;
			const preview = previewKind(mime || blob.type) === "text" ? await blob.slice(0, TEXT_PREVIEW_BYTES).text() : null;
			if (mine !== generation) return;
			url = URL.createObjectURL(blob);
			blobType = blob.type;
			text = preview;
		} catch (e) {
			if (mine === generation) fetchError = e instanceof Error ? e.message : String(e);
		} finally {
			if (mine === generation) loading = false;
		}
	}

	/** Pick up what the harness wrote since (error, holders), then try the bytes again. */
	async function retry() {
		await onchanged().catch(() => {});
		await load();
	}

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
	}

	// New bytes (another file, or a pairing that just arrived): fetch them;
	// the previous object URL is revoked on change and on teardown.
	$effect(() => {
		void hash;
		void paired;
		untrack(() => void load());
		return () => {
			generation++;
			release();
		};
	});

	onMount(() => {
		if (!isLocalBackend) return;
		void fetchMachines().then(({ machines: roster }) => (machines = roster)).catch(() => {});
		void loadPairing();
		return onPairingChange(() => void loadPairing());
	});
</script>

<section class="file" data-testid="file">
	<div class="facts">
		{#if size || hash}<span data-testid="file-size">{humanSize(size)}</span>{/if}
		{#if shownType}<span class="dot">·</span><span data-testid="file-mime">{shownType}</span>{/if}
		{#if hash}<span class="dot">·</span><code class="hash" title={hash} data-testid="file-hash">{hash.slice(0, 12)}</code>{/if}
	</div>

	{#if objectError}<p class="error" role="alert" data-testid="file-error">{objectError}</p>{/if}

	{#if !isLocalBackend}
		<p class="muted" data-testid="file-local-only">{FILES_NEED_LOCAL}</p>
	{:else if !paired}
		<PairGate compact onready={() => void loadPairing()} />
	{:else if !hash}
		<p class="muted">This file has no bytes yet.</p>
	{:else}
		<div class="sec">
			{#if loading}
				<p class="muted" role="status" data-testid="file-loading">{fetchingFrom ? `Fetching from ${fetchingFrom}…` : "Loading…"}</p>
			{:else if fetchError}
				<p class="error" role="alert" data-testid="file-fetch-error">{fetchError}</p>
			{:else if url}
				{#if kind === "image"}
					<img class="preview" src={url} alt={name} />
				{:else if kind === "video"}
					<!-- svelte-ignore a11y_media_has_caption -->
					<video class="preview" src={url} controls></video>
				{:else if kind === "audio"}
					<audio src={url} controls></audio>
				{:else if kind === "pdf"}
					<iframe class="preview pdf" src={url} title={name}></iframe>
				{:else if kind === "text" && text !== null}
					<pre class="preview text">{text}</pre>
					{#if size > TEXT_PREVIEW_BYTES}<p class="muted">Showing the first {humanSize(TEXT_PREVIEW_BYTES)} - download for the rest.</p>{/if}
				{/if}
			{/if}
			<div class="actions">
				{#if url}
					<a class="subtle-btn" href={url} download={name || "file"} data-testid="file-download">Download</a>
				{/if}
				{#if fetchError || objectError}
					<button class="subtle-btn" disabled={loading} data-testid="file-retry" onclick={() => void retry()}>Retry</button>
				{/if}
			</div>
		</div>
	{/if}

	<div class="sec">
		<div class="sec-name">Stored on</div>
		{#if holders.length === 0}
			<p class="muted" data-testid="file-holders-empty">No computer holds this file yet</p>
		{:else}
			<ul class="holders" data-testid="file-holders">
				{#each holders as h (h.machineId)}
					<li>
						<span>{MACHINE_ICON}</span>
						{#if h.row}<a href="/app/object/{h.row.id}">{h.name}</a>{:else}<span>{h.name}</span>{/if}
						{#if h.machineId === thisMachine}<span class="muted">(this computer)</span>{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</div>
</section>

<style>
	.file { display: flex; flex-direction: column; gap: 20px; margin: 16px 0 0 48px; max-width: 720px; }
	@media (max-width: 720px) { .file { margin: 16px 16px 0; } }
	.facts { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px; color: var(--muted); font-size: 13px; }
	.dot { opacity: 0.6; }
	.hash { font-size: 12px; cursor: help; }
	.sec { display: flex; flex-direction: column; gap: 8px; }
	.sec-name { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
	.preview { max-width: 100%; border: 1px solid var(--border); border-radius: 10px; background: var(--bg); }
	img.preview, video.preview { max-height: 70vh; object-fit: contain; align-self: flex-start; }
	.pdf { width: 100%; height: 70vh; }
	.text { margin: 0; padding: 12px; max-height: 60vh; overflow: auto; font-size: 12.5px; line-height: 1.5; color: var(--fg); white-space: pre-wrap; word-break: break-word; }
	audio { width: 100%; }
	.holders { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 4px; font-size: 14px; color: var(--fg); }
	.holders li { display: flex; gap: 6px; align-items: baseline; }
	a { color: var(--fg); }
	a:hover { color: var(--accent); }
	.muted { margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; }
	.error { margin: 0; color: var(--red, #ff6961); font-size: 13px; line-height: 1.5; white-space: pre-wrap; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.subtle-btn { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--muted); text-decoration: none; }
	.subtle-btn:hover:not(:disabled) { border-color: var(--accent); color: var(--fg); }
	.subtle-btn:disabled { opacity: 0.5; cursor: default; }
</style>
