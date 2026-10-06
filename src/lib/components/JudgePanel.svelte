<script lang="ts">
	/**
	 * A Judge's own panel: how its page is read, which computer asks Jev
	 * (its Served by, else the computer keeping its TypeSafe credential),
	 * and every object that lists it in Judges, with the answer it gave.
	 */
	import { fetchAllQuery, fetchObject, type QueryResultRow } from "$lib/api";
	import { fetchMachines, machineName, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON, type ValueJSON } from "$lib/types";

	let { object }: { object: ObjectJSON } = $props();

	const answer = $derived(fieldStr(object.fields, "judge_answer"));
	const propertyKey = $derived(fieldStr(object.fields, "judge_property"));
	const linkIds = (v: ValueJSON | undefined): string[] =>
		(v?.valuesValue?.items ?? (v ? [v] : [])).map((i) => i.linkValue?.targetId ?? i.stringValue ?? "").filter(Boolean);
	const hasPrompt = $derived(linkIds(object.fields["prompt"]).length > 0);

	const HOW: Record<string, string> = {
		Score: "On the Prompt's page: the question, then a numbered list of 2–10 levels, lowest first. The answer is a number on that list (1 = the first level).",
		Choice: "On the Prompt's page: the question, then a bulleted list of options - “Name: what it means”, or just the name.",
		"Yes or no": "On the Prompt's page: a yes/no question. Optional lines “Yes: …” and “No: …” say what each means.",
	};


	let scored = $state<QueryResultRow[] | null>(null);
	let computer = $state("");
	$effect(() => {
		const judge = object;
		void (async () => {
			try {
				const [rows, { machines }] = await Promise.all([fetchAllQuery({ filters: [{ key: "judges", condition: "exists" }] }), fetchMachines()]);
				scored = rows.filter((r) => linkIds(r.fields["judges"]).includes(judge.id));
				computer = await runsOn(judge, machines);
			} catch {
				scored = [];
			}
		})();
	});

	/** The computer that asks Jev for this Judge, by name; "" when none is set. */
	async function runsOn(judge: ObjectJSON, machines: MachineRow[]): Promise<string> {
		let id = fieldStr(judge.fields, "served_by");
		if (!id) {
			for (const credId of linkIds(judge.fields["credentials"])) {
				const cred = await fetchObject(credId).catch(() => null);
				if (cred && fieldStr(cred.fields, "service") === "typesafe") {
					id = fieldStr(cred.fields, "served_by");
					break;
				}
			}
		}
		return id ? machineName(machines, id) : "";
	}

	/** The answer this Judge gave an object, and how sure: "9 · 97% sure". */
	function verdict(row: QueryResultRow): string {
		const v = propertyKey ? row.fields[propertyKey] : undefined;
		if (!v) return "not scored yet";
		const value = v.floatValue ?? v.intValue ?? (v.boolValue !== undefined ? (v.boolValue ? "Yes" : "No") : v.stringValue ?? "");
		const note = row.fields["judged"]?.mapValue?.entries?.[propertyKey]?.mapValue?.entries;
		const p = note?.["probability"]?.floatValue;
		const c = note?.["confidence"]?.floatValue;
		const sure = p !== undefined ? `${Math.round(p * 100)}% yes` : c !== undefined ? `${Math.round(c * 100)}% sure` : "";
		return sure ? `${value} · ${sure}` : String(value);
	}
</script>

<div class="judge">
	{#if !hasPrompt}
		<p class="warn">Pick a <b>Prompt</b> (or “+ New prompt” in its picker): that System prompt's page is the question this Judge asks.</p>
	{/if}
	<p class="how">{HOW[answer] ?? "Pick an Answer: Score, Choice, or Yes or no."}</p>
	<p class="how">Edit the Prompt's page to change the question - everything this Judge scores is scored again. Its name is the property it fills in; add it to any object's <b>Judges</b> to score that object.</p>
	{#if computer}
		<p class="hint">Asks Jev from {computer}.</p>
	{:else}
		<p class="warn">Add a TypeSafe credential to Credentials (or a computer in Served by) - until then nothing scores.</p>
	{/if}
	{#if scored && scored.length > 0}
		<span class="label">Scores {scored.length} object{scored.length === 1 ? "" : "s"}</span>
		<ul class="results">
			{#each scored as r (r.id)}
				<li>
					<a href="/app/object/{r.id}">{fieldStr(r.fields, "name") || "Untitled"}</a>
					<span class="answer">{verdict(r)}</span>
				</li>
			{/each}
		</ul>
	{:else if scored}
		<p class="hint">No object lists this Judge yet.</p>
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
	.hint,
	.warn {
		margin: 0;
		font-size: 12px;
		line-height: 1.45;
		color: var(--muted);
	}
	.warn {
		color: var(--yellow, #e0b341);
	}
	.label {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
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
		color: var(--muted);
		white-space: nowrap;
	}
</style>
