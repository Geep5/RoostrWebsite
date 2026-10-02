<script lang="ts">
	/**
	 * A File object shown inside a page: images inline, PDFs in the
	 * browser's own viewer, anything else as a card. The block only names
	 * the File (`fileId`, `hash`); the bytes come from this computer's
	 * harness, which fetches them peer-to-peer when it lacks them.
	 *
	 * Images never grow past the page column, and can be resized by
	 * dragging their bottom-right corner; the width (percent of the column)
	 * is kept on the block as `width`.
	 */
	import { isLocalBackend } from "$lib/client-backend";
	import { FILES_NEED_LOCAL, fetchFileBlob } from "$lib/files";
	import { store } from "$lib/data.svelte";
	import { note } from "$lib/api";

	let {
		meta,
		objectId,
		blockId,
		readonly,
		onrefresh,
	}: {
		meta: Record<string, string>;
		objectId: string;
		blockId: string;
		readonly: boolean;
		onrefresh: () => void | Promise<void>;
	} = $props();

	const MIN_WIDTH = 10;

	const mime = $derived(meta["mime"] ?? "");
	const name = $derived(store.summaries.find((s) => s.id === meta["fileId"])?.name || meta["name"] || "File");
	const inline = $derived(mime.startsWith("image/") || mime === "application/pdf");
	/** Percent of the column; null = never resized (natural size, never wider than the column). */
	const savedWidth = $derived(Number(meta["width"]) > 0 ? Math.min(100, Math.max(MIN_WIDTH, Number(meta["width"]))) : null);

	let url = $state("");
	let error = $state("");
	let attempt = $state(0);
	/** The width while a drag is in progress; null = show the saved one. */
	let dragWidth = $state<number | null>(null);
	let frame = $state<HTMLDivElement>();

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

	function startResize(e: PointerEvent) {
		const column = frame?.parentElement;
		if (!frame || !column) return;
		e.preventDefault();
		e.stopPropagation();
		const handle = e.currentTarget;
		if (handle instanceof HTMLElement) handle.setPointerCapture(e.pointerId);
		const left = frame.getBoundingClientRect().left;
		const columnWidth = column.getBoundingClientRect().width;
		const move = (ev: PointerEvent) => {
			dragWidth = Math.round(Math.min(100, Math.max(MIN_WIDTH, ((ev.clientX - left) / columnWidth) * 100)));
		};
		const end = async () => {
			window.removeEventListener("pointermove", move);
			window.removeEventListener("pointerup", end);
			const width = dragWidth;
			if (width === null || width === savedWidth) {
				dragWidth = null;
				return;
			}
			await note.blockUpdate(objectId, blockId, { custom: { contentType: "file", meta: { ...meta, width: String(width) } } });
			await onrefresh();
			dragWidth = null;
		};
		window.addEventListener("pointermove", move);
		window.addEventListener("pointerup", end);
	}
</script>

<div class="file-block">
	{#if !isLocalBackend}
		<p class="note">{FILES_NEED_LOCAL}</p>
	{:else if inline && error}
		<p class="err">{error} <button onclick={() => attempt++}>Retry</button></p>
	{:else if inline && !url}
		<p class="note">Loading {name}…</p>
	{:else if mime.startsWith("image/")}
		<div class="img-frame" class:resizing={dragWidth !== null} class:sized={(dragWidth ?? savedWidth) !== null} bind:this={frame} style={(dragWidth ?? savedWidth) !== null ? `width:${dragWidth ?? savedWidth}%` : ""}>
			<img src={url} alt={name} draggable="false" />
			{#if !readonly}
				<button class="resize" title="Drag to resize" aria-label="Resize image" onpointerdown={startResize}></button>
			{/if}
		</div>
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
	.img-frame {
		position: relative;
		max-width: 100%;
		align-self: flex-start;
	}
	img {
		display: block;
		max-width: 100%;
		max-height: 70vh;
		height: auto;
		border-radius: 8px;
	}
	.img-frame.sized img {
		width: 100%;
		max-height: none;
	}
	.resize {
		position: absolute;
		right: -5px;
		bottom: -5px;
		width: 14px;
		height: 14px;
		padding: 0;
		border-radius: 50%;
		border: 2px solid var(--bg, #fff);
		background: var(--accent);
		cursor: nwse-resize;
		opacity: 0;
		transition: opacity 0.12s;
		touch-action: none;
	}
	.img-frame:hover .resize,
	.img-frame.resizing .resize,
	.resize:focus-visible {
		opacity: 1;
	}
	.img-frame.resizing {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
		border-radius: 8px;
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
