<script lang="ts">
	/**
	 * A Judge's own panel: how its page is read, whether it can run on its
	 * Repeat, and Try it - Jev's answers for the newest objects it runs on,
	 * from the paired harness, written nowhere.
	 */
	import { harnessFetch } from "$lib/local-transport";
	import { fieldStr, type ObjectJSON } from "$lib/types";

	let { object }: { object: ObjectJSON } = $props();

	interface TrialRow { id: string; name: string; text: string; confidence?: number; probability?: number; error?: string }

	let busy = $state(false);
	let error = $state("");
	let rows = $state<TrialRow[] | null>(null);

	const answer = $derived(fieldStr(object.fields, "judge_answer"));
	const hasRepeat = $derived(!!object.fields["repeat"]);
	const hasComputer = $derived(!!fieldStr(object.fields, "served_by"));

	const HOW: Record<string, string> = {
		Score: "Write the question, then a numbered list of 2–10 levels, lowest first. The answer is a number on that list (1 = the first level).",
		Choice: "Write the question, then a bulleted list of options - “Name: what it means”, or just the name.",
		"Yes or no": "Write a yes/no question. Optional lines “Yes: …” and “No: …” say what each means.",
	};

	/** "94% sure", or for Yes/No "87% yes". */
	function sure(r: TrialRow): string {
		if (r.probability !== undefined) return `${Math.round(r.probability * 100)}% yes`;
		if (r.confidence !== undefined) return `${Math.round(r.confidence * 100)}% sure`;
		return "";
	}

	async function tryIt() {
		busy = true;
		error = "";
		rows = null;
		try {
			const res = await harnessFetch("/judges/try", { method: "POST", body: JSON.stringify({ id: object.id }) });
			const body = (await res.json().catch(() => ({}))) as { rows?: TrialRow[]; error?: string };
			if (!res.ok || !body.rows) throw new Error(body.error ?? `HTTP ${res.status}`);
			rows = body.rows;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="judge">
	<p class="how">{HOW[answer] ?? "Pick an Answer: Score, Choice, or Yes or no. The page is the question Jev answers."}</p>
	{#if !hasRepeat || !hasComputer}
		<p class="hint">To run on its own, give it a Repeat and a computer in Served by. Each run judges what's new or changed in Runs on.</p>
	{/if}
	<button class="try" disabled={busy} onclick={() => void tryIt()}>{busy ? "Asking Jev…" : "Try it"}</button>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	{#if rows}
		{#if rows.length === 0}
			<p class="hint">Runs on has nothing in it yet.</p>
		{:else}
			<ul class="results">
				{#each rows as r (r.id)}
					<li>
						<a href="/app/object/{r.id}">{r.name}</a>
						{#if r.error}
							<span class="error">{r.error}</span>
						{:else}
							<span class="answer">{r.text}</span><span class="sure">{sure(r)}</span>
						{/if}
					</li>
				{/each}
			</ul>
			<p class="hint">Not saved - the Judge writes answers when its Repeat runs.</p>
		{/if}
	{/if}
</div>

<style>
	.judge {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 8px 4px 4px;
		border-top: 1px solid var(--border);
		margin-top: 4px;
	}
	.how,
	.hint {
		margin: 0;
		font-size: 12px;
		line-height: 1.45;
		color: var(--muted);
	}
	.try {
		align-self: flex-start;
		background: var(--hover);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--fg);
		font-size: 12.5px;
		padding: 5px 12px;
		cursor: pointer;
	}
	.try:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.error {
		margin: 0;
		font-size: 12px;
		color: var(--red);
	}
	.results {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-size: 12.5px;
	}
	.results li {
		display: flex;
		gap: 8px;
		align-items: baseline;
		min-width: 0;
	}
	.results a {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--fg);
		text-decoration: none;
	}
	.results a:hover {
		text-decoration: underline;
	}
	.answer {
		font-weight: 600;
		white-space: nowrap;
	}
	.sure {
		color: var(--muted);
		white-space: nowrap;
	}
</style>
