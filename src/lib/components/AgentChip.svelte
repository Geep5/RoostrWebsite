<script lang="ts">
	/**
	 * Which agent answers this object. The object's own `agent` field names
	 * it; empty means the space's default agent (the one whose
	 * `space_default` is this object's channel). The chip states the answer
	 * and opens a picker over the agents of the same space - each choice is
	 * a normal field write on the object, never on an agent.
	 */
	import type { ObjectJSON } from "$lib/types";
	import { fetchQuery, note } from "$lib/api";
	import { store } from "$lib/data.svelte";
	import { AGENTLESS_TYPES } from "$lib/agent-field";

	let {
		object,
		onchanged,
	}: {
		object: ObjectJSON;
		onchanged: () => Promise<void>;
	} = $props();

	interface AgentOption {
		id: string;
		name: string;
		icon: string;
		/** Answers everything in this space that names no agent. */
		isDefault: boolean;
	}

	let agents = $state<AgentOption[]>([]);
	let open = $state(false);
	let busy = $state(false);
	let error = $state("");

	const hidden = $derived(!!AGENTLESS_TYPES[object.typeKey]);
	/** Objects of the first space carry no channel stamp; agents there do. */
	const channelId = $derived(object.fields["channel"]?.stringValue || store.channels[0]?.id || "");
	const assigned = $derived(object.fields["agent"]?.stringValue ?? "");
	const current = $derived(agents.find((a) => a.id === assigned));
	const spaceDefault = $derived(agents.find((a) => a.isDefault));

	// Every refresh hands down a new object; the roster follows its space.
	$effect(() => {
		const space = channelId;
		if (hidden || !space) return;
		void (async () => {
			try {
				const res = await fetchQuery({ type: "agent", filters: [{ key: "channel", condition: "equal", value: space }], limit: 200 });
				agents = res.records
					.filter((r) => !r.fields["spawn_parent"]?.stringValue) // subagents are ephemeral
					.map((r) => ({
						id: r.id,
						name: r.fields["name"]?.stringValue || "Agent",
						icon: r.fields["iconEmoji"]?.stringValue ?? "",
						isDefault: r.fields["space_default"]?.stringValue === space,
					}))
					.sort((a, b) => Number(b.isDefault) - Number(a.isDefault) || a.name.localeCompare(b.name));
				error = "";
			} catch (e) {
				agents = [];
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
	function choose(agentId: string) {
		if (agentId === assigned) return;
		void run(() => (agentId ? note.setField(object.id, "agent", { stringValue: agentId }) : note.deleteField(object.id, "agent")));
	}
</script>

{#if !hidden}
	<div class="agent">
		<button class="chip" data-testid="agent-chip" onclick={() => (open = !open)} title="Which agent answers this object">
			<span class="glyph">{current?.icon || "🛰️"}</span>
			{#if current}
				{current.name}
			{:else if assigned}
				Agent {assigned.slice(0, 8)}…
			{:else}
				Space agent{spaceDefault ? ` · ${spaceDefault.name}` : ""}
			{/if}
		</button>
		{#if error}<p class="err">{error}</p>{/if}

		{#if open}
			<div class="pop">
				<div class="pop-head">
					<span class="pop-name">Answered by</span>
				</div>
				<div class="opts">
					<label class="opt" data-testid="agent-chip-default">
						<input type="radio" name="answered-by" disabled={busy} checked={!assigned} onchange={() => choose("")} />
						<span class="opt-name">Use space default</span>
						<span class="opt-sub">{spaceDefault ? `currently ${spaceDefault.name}` : "no space agent yet"}</span>
					</label>
					{#each agents as a (a.id)}
						<label class="opt" class:current={a.id === assigned} data-testid="agent-chip-option-{a.id}">
							<input type="radio" name="answered-by" disabled={busy} checked={a.id === assigned} onchange={() => choose(a.id)} />
							<span class="opt-name">{a.icon ? `${a.icon} ` : ""}{a.name}</span>
							<span class="opt-sub">{a.isDefault ? "space default" : ""}</span>
						</label>
					{/each}
					{#if agents.length === 0}
						<p class="none">No agent in this space yet.</p>
					{/if}
				</div>
			</div>
			<button class="backdrop" aria-label="Close" onclick={() => (open = false)}></button>
		{/if}
	</div>
{/if}

<style>
	.agent {
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
	.glyph {
		font-size: 12px;
	}
	.err {
		margin: 0;
		padding: 6px 10px;
		border-radius: 6px;
		background: rgba(229, 72, 77, 0.1);
		border: 1px solid rgba(229, 72, 77, 0.3);
		font-size: 12.5px;
	}

	/* Popover - same shell as ServingChip's .pop */
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
	.none {
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
		.agent {
			margin: -4px 16px 10px;
		}
		.pop {
			width: min(360px, calc(100vw - 32px));
		}
	}
</style>
