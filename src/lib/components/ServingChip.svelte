<script lang="ts">
	/**
	 * Which machine serves this object. The engine resolves it
	 * (`$lib/serving`); the chip states the answer and opens a picker that
	 * edits its inputs - a `served_by` pin and the `requires` capability
	 * list - each a normal field write. Warning reasons (a pin the machine
	 * cannot honour, a need nobody has) render in the warning colour.
	 */
	import type { ObjectJSON } from "$lib/types";
	import { note } from "$lib/api";
	import { capabilityLabel, machineName, resolveServing, servingCopy, type MachineRow, type Serving } from "$lib/serving";
	import { capabilityCards, loadCards, type Card } from "$lib/cards";

	let {
		object,
		onchanged,
	}: {
		object: ObjectJSON;
		onchanged: () => Promise<void>;
	} = $props();

	let serving = $state<Serving | null>(null);
	let machines = $state<MachineRow[]>([]);
	/** What a machine can be asked for: the cards published in this vault. */
	let capabilities = $state<Card[]>(capabilityCards());
	let open = $state(false);
	let busy = $state(false);
	let error = $state("");

	const pinned = $derived(object.fields["served_by"]?.stringValue ?? "");
	const copy = $derived(serving ? servingCopy(serving, machines) : null);

	// Every refresh hands down a new object; the resolution follows it.
	$effect(() => {
		const current = object;
		void (async () => {
			try {
				const out = await resolveServing(current);
				serving = out.serving;
				machines = out.machines;
				await loadCards();
				capabilities = capabilityCards();
				error = "";
			} catch (e) {
				serving = null;
				error = e instanceof Error ? e.message : String(e);
			}
		})();
	});
	// Navigating object -> object closes a stale picker.
	$effect(() => {
		void object.id;
		open = false;
	});

	async function run(op: () => Promise<unknown>) {
		busy = true;
		error = "";
		try {
			await op();
			await onchanged();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
	function pin(machineId: string) {
		if (machineId === pinned) return;
		void run(() => (machineId ? note.setField(object.id, "served_by", { stringValue: machineId }) : note.deleteField(object.id, "served_by")));
	}
	function toggleRequire(key: string) {
		const have = serving?.requires ?? [];
		const next = have.includes(key) ? have.filter((k) => k !== key) : [...have, key];
		void run(() =>
			next.length ? note.setField(object.id, "requires", { valuesValue: { items: next.map((k) => ({ stringValue: k })) } }) : note.deleteField(object.id, "requires"),
		);
	}
</script>

<div class="serving">
	<button class="chip" class:overdue={copy?.warning} disabled={!serving} onclick={() => (open = !open)} title="Which machine serves this object">
		<span class="glyph">🖥️</span>
		{#if copy}
			{copy.text}
			{#if serving?.requires.length && serving.reason !== "capability" && serving.reason !== "pinned-uncapable" && serving.reason !== "unsatisfied"}
				· needs {serving.requires.map(capabilityLabel).join(", ")}
			{/if}
		{:else}
			resolving…
		{/if}
	</button>
	{#if error}<p class="err">{error}</p>{/if}

	{#if open && serving}
		<div class="pop">
			<div class="pop-head">
				<span class="pop-name">Served by</span>
			</div>
			<div class="opts">
				<label class="opt">
					<input type="radio" name="served-by" disabled={busy} checked={!pinned} onchange={() => pin("")} />
					<span class="opt-name">Automatic</span>
					<span class="opt-sub">{pinned ? "clear the pin" : `currently ${machineName(machines, serving.machineId)}`}</span>
				</label>
				{#each machines as m (m.id)}
					<label class="opt" class:current={m.machineId === serving.machineId}>
						<input type="radio" name="served-by" disabled={busy} checked={pinned === m.machineId} onchange={() => pin(m.machineId)} />
						<span class="opt-name">{m.name || `${m.machineId.slice(0, 8)}…`}</span>
						<span class="opt-sub">
							{#if m.machineId === serving.machineId}serving now · {/if}{m.capabilities.length ? m.capabilities.map(capabilityLabel).join(", ") : "no capabilities"}
						</span>
					</label>
				{/each}
				{#if machines.length === 0}
					<p class="none">No machine has published itself yet.</p>
				{/if}
			</div>

			<div class="pop-head">
				<span class="pop-name">Needs</span>
			</div>
			<div class="opts">
				{#each capabilities as c (c.key)}
					<label class="opt">
						<input type="checkbox" disabled={busy} checked={serving.requires.includes(c.key)} onchange={() => toggleRequire(c.key)} />
						<span class="opt-name">{c.name}</span>
						<span class="opt-sub">{c.key}</span>
					</label>
				{/each}
				{#if capabilities.length === 0}
					<p class="none">No machine has published what it can do yet.</p>
				{/if}
			</div>
			{#if copy?.warning}
				<p class="warn overdue">{copy.text}</p>
			{/if}
		</div>
		<button class="backdrop" aria-label="Close" onclick={() => (open = false)}></button>
	{/if}
</div>

<style>
	.serving {
		position: relative;
		margin: -8px 0 14px 48px;
		font-size: 13px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.chip {
		border: none;
		background: none;
		color: var(--muted);
		padding: 2px 4px;
		border-radius: 6px;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 11.5px;
		align-self: flex-start;
		text-align: left;
	}
	.chip:hover {
		background: var(--hover);
	}
	.chip:disabled {
		cursor: default;
		opacity: 0.7;
	}
	.glyph {
		font-size: 12px;
	}
	.overdue {
		color: var(--red, #e5484d);
	}
	.err {
		margin: 0;
		padding: 6px 10px;
		border-radius: 6px;
		background: rgba(229, 72, 77, 0.1);
		border: 1px solid rgba(229, 72, 77, 0.3);
		font-size: 12.5px;
	}

	/* Popover - same shell as Repeat's .pop */
	.pop {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 90;
		width: 360px;
		background: var(--panel, #1a1d23);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px 12px 12px;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.pop-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.pop-head + .opts {
		margin-top: -4px;
	}
	.pop-name {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.opts {
		display: flex;
		flex-direction: column;
	}
	.opts + .pop-head {
		border-top: 1px solid var(--border);
		padding-top: 8px;
	}
	.opt {
		display: grid;
		grid-template-columns: auto 1fr;
		column-gap: 8px;
		align-items: center;
		padding: 4px 4px;
		border-radius: 6px;
		cursor: pointer;
		font-size: 12.5px;
	}
	.opt:hover {
		background: var(--hover);
	}
	.opt input {
		margin: 0;
		grid-row: 1 / span 2;
		accent-color: var(--accent);
	}
	.opt-name {
		color: var(--fg);
	}
	.opt.current .opt-name {
		font-weight: 600;
	}
	.opt-sub {
		grid-column: 2;
		font-size: 11px;
		color: var(--muted);
	}
	.opt-sub:empty {
		display: none;
	}
	.none,
	.warn {
		margin: 0;
		font-size: 12px;
		color: var(--muted);
	}
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 80;
		background: none;
		border: none;
		cursor: default;
	}
	@media (max-width: 720px) {
		.serving {
			margin: -4px 16px 10px;
		}
		.pop {
			width: min(360px, calc(100vw - 32px));
		}
	}
</style>
