<script lang="ts">
	/**
	 * UI PREVIEW ONLY - recurring tasks.
	 *
	 * Nothing here touches the engine, the DAG, or the relay. The rule and
	 * the mock occurrence history live in localStorage under
	 * `roostr.preview.repeat:<objectId>` so the mock survives reloads while
	 * we look at it. Delete the key (or clear the rule) to reset.
	 *
	 * The rule is deliberately small: every N <unit>, on <weekdays | day of
	 * month>, at <time of day>. The first occurrence is the next match from
	 * today (or the task's due date) at that time; each completion opens the
	 * next one on the fixed schedule.
	 */
	let {
		objectId,
		dueDate,
	}: {
		objectId: string;
		/** ms epoch of the task's dueDate relation, if set. Seeds the anchor. */
		dueDate?: number;
	} = $props();

	type Freq = "day" | "week" | "month" | "year";
	interface Rule {
		freq: Freq;
		interval: number;
		/** 0 = Sun … 6 = Sat. Weekly only. */
		weekdays: number[];
		/** Monthly only: same calendar day, or same "2nd Tuesday". */
		monthly: "date" | "weekday";
		/** ms epoch, start of the day the cadence is counted from (seeded, not edited). */
		start: number;
		/** Minutes after midnight, local time. */
		time: number;
	}
	interface Series {
		rule: Rule;
		/** ms epoch (start of day) of the occurrence that is currently open. */
		current: number;
		history: { at: number; state: "done" | "skipped"; on: number }[];
	}

	const KEY = $derived(`roostr.preview.repeat:${objectId}`);
	const DAY = 86_400_000;
	const MIN = 60_000;
	const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
	const ORD = ["", "1st", "2nd", "3rd", "4th", "last"];

	function load(key: string): Series | null {
		if (typeof localStorage === "undefined") return null;
		try {
			const raw = localStorage.getItem(key);
			const parsed = raw ? (JSON.parse(raw) as Series) : null;
			// Older previews stored rules without a time; they are not worth migrating.
			return parsed && typeof parsed.rule.time === "number" ? parsed : null;
		} catch {
			return null;
		}
	}
	let series = $state<Series | null>(null);
	let open = $state(false);
	let flash = $state("");
	// Navigating task -> task re-keys the mock; each object has its own series.
	$effect(() => {
		series = load(KEY);
		open = false;
		flash = "";
	});
	$effect(() => {
		if (typeof localStorage === "undefined") return;
		if (series) localStorage.setItem(KEY, JSON.stringify(series));
		else localStorage.removeItem(KEY);
	});

	// ── Dates ─────────────────────────────────────────────────────
	const sod = (ms: number) => {
		const d = new Date(ms);
		d.setHours(0, 0, 0, 0);
		return d.getTime();
	};
	const fmt = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
	const fmtLong = (ms: number) => new Date(ms).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
	const fmtTime = (minutes: number) => new Date(sod(Date.now()) + minutes * MIN).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
	const hhmm = (minutes: number) => `${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
	const addMonths = (ms: number, n: number, day: number) => {
		const d = new Date(ms);
		d.setDate(1);
		d.setMonth(d.getMonth() + n);
		const last = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
		d.setDate(Math.min(day, last));
		return sod(d.getTime());
	};
	/** "2nd Tuesday" of the month containing `ms`; ordinal 5 = last. */
	const nthWeekday = (ms: number, weekday: number, ordinal: number) => {
		const d = new Date(ms);
		if (ordinal === 5) {
			d.setMonth(d.getMonth() + 1, 0);
			while (d.getDay() !== weekday) d.setDate(d.getDate() - 1);
			return sod(d.getTime());
		}
		d.setDate(1);
		while (d.getDay() !== weekday) d.setDate(d.getDate() + 1);
		d.setDate(d.getDate() + 7 * (ordinal - 1));
		return sod(d.getTime());
	};
	const ordinalOf = (ms: number) => {
		const d = new Date(ms);
		const n = Math.ceil(d.getDate() / 7);
		const lastOfMonth = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
		return d.getDate() + 7 > lastOfMonth ? 5 : n;
	};

	/** Next occurrence day strictly after `from` (an occurrence day). */
	function stepFrom(rule: Rule, from: number): number {
		const start = new Date(rule.start);
		switch (rule.freq) {
			case "day":
				return from + rule.interval * DAY;
			case "week": {
				const days = rule.weekdays.length ? rule.weekdays : [start.getDay()];
				const weekStart = (ms: number) => sod(ms - new Date(ms).getDay() * DAY);
				const w0 = weekStart(rule.start);
				let d = from + DAY;
				for (let i = 0; i < 400; i++, d += DAY) {
					const weeks = Math.round((weekStart(d) - w0) / (7 * DAY));
					if (weeks % rule.interval === 0 && days.includes(new Date(d).getDay())) return d;
				}
				return d;
			}
			case "month": {
				if (rule.monthly === "weekday") {
					const m = new Date(from);
					m.setDate(1);
					m.setMonth(m.getMonth() + rule.interval);
					return nthWeekday(m.getTime(), start.getDay(), ordinalOf(rule.start));
				}
				return addMonths(from, rule.interval, start.getDate());
			}
			case "year": {
				const d = new Date(from);
				d.setFullYear(d.getFullYear() + rule.interval);
				return sod(d.getTime());
			}
		}
	}

	/** The start day itself when it fits the rule and its time is still ahead; otherwise the next match. */
	function firstOccurrence(rule: Rule): number {
		const fits = rule.freq !== "week" || rule.weekdays.includes(new Date(rule.start).getDay());
		if (fits && rule.start + rule.time * MIN > Date.now()) return rule.start;
		return stepFrom(rule, rule.start);
	}

	function upcoming(rule: Rule, from: number, count: number): number[] {
		const out: number[] = [];
		let cur = from;
		for (let i = 0; i < count; i++) {
			cur = stepFrom(rule, cur);
			out.push(cur);
		}
		return out;
	}

	function describe(rule: Rule): string {
		const every = rule.interval === 1 ? "Every" : `Every ${rule.interval}`;
		const unit = rule.interval === 1 ? rule.freq : `${rule.freq}s`;
		const start = new Date(rule.start);
		let s: string;
		if (rule.freq === "week") {
			const wd = [...rule.weekdays].sort();
			if (wd.length === 5 && wd.join() === "1,2,3,4,5" && rule.interval === 1) s = "Every weekday";
			else s = `${every} ${unit} on ${wd.length ? wd.map((d) => WD[d]).join(", ") : WD[start.getDay()]}`;
		} else if (rule.freq === "month") {
			s =
				rule.monthly === "weekday"
					? `${every} ${unit} on the ${ORD[ordinalOf(rule.start)]} ${WD[start.getDay()]}`
					: `${every} ${unit} on the ${start.getDate()}${suffix(start.getDate())}`;
		} else if (rule.freq === "year") {
			s = `${every} ${unit} on ${fmt(rule.start)}`;
		} else {
			s = `${every} ${unit}`;
		}
		return `${s} at ${fmtTime(rule.time)}`;
	}
	const suffix = (n: number) => (n % 10 === 1 && n !== 11 ? "st" : n % 10 === 2 && n !== 12 ? "nd" : n % 10 === 3 && n !== 13 ? "rd" : "th");

	// ── Editing ───────────────────────────────────────────────────
	const seed = (): Rule => {
		const start = sod(dueDate ?? Date.now());
		return { freq: "week", interval: 1, weekdays: [new Date(start).getDay()], monthly: "date", start, time: 9 * 60 };
	};
	let draft = $state<Rule>(seed());
	const draftFirst = $derived(firstOccurrence(draft));
	const preview = $derived([draftFirst, ...upcoming(draft, draftFirst, 4)]);

	function openEditor() {
		draft = series ? structuredClone($state.snapshot(series.rule)) : seed();
		open = true;
	}
	function toggleWeekday(d: number) {
		draft.weekdays = draft.weekdays.includes(d) ? draft.weekdays.filter((x) => x !== d) : [...draft.weekdays, d];
		if (!draft.weekdays.length) draft.weekdays = [d];
	}
	function setTime(value: string) {
		const [h, m] = value.split(":").map(Number);
		if (Number.isInteger(h) && Number.isInteger(m)) draft.time = h * 60 + m;
	}
	function save() {
		const keep = series && JSON.stringify(series.rule) === JSON.stringify(draft);
		if (!keep) series = { rule: structuredClone($state.snapshot(draft)), current: firstOccurrence(draft), history: [] };
		open = false;
	}
	function clear() {
		series = null;
		open = false;
		flash = "";
	}

	// ── Mock completion flow ──────────────────────────────────────
	function finish(state: "done" | "skipped") {
		if (!series) return;
		const s = series;
		const next = stepFrom(s.rule, s.current);
		s.history = [...s.history, { at: s.current, state, on: Date.now() }];
		s.current = next;
		flash = `${state === "done" ? "Done" : "Skipped"} ${fmt(s.history.at(-1)!.at)} · next opens ${fmtLong(next)} at ${fmtTime(s.rule.time)}.`;
	}
	function undo() {
		if (!series?.history.length) return;
		const last = series.history.at(-1)!;
		series.history = series.history.slice(0, -1);
		series.current = last.at;
		flash = "";
	}
	const next3 = $derived(series ? upcoming(series.rule, series.current, 3) : []);
	const overdue = $derived(!!series && series.current + series.rule.time * MIN < Date.now());
</script>

<div class="repeat" class:active={!!series}>
	<div class="row">
		<button class="cell" class:empty={!series} onclick={openEditor} title="Repeat">
			<span class="glyph">↻</span>
			{#if series}
				{describe(series.rule)}
				<span class="sep">·</span>
				<span class:overdue>{overdue ? "overdue since" : "next"} {fmtLong(series.current)}</span>
			{:else}
				Does not repeat
			{/if}
		</button>
		<span class="preview-tag" title="Mock only - nothing is saved to the DAG or relay">UI preview</span>
	</div>

	{#if series}
		<div class="strip">
			{#each series.history as h (h.at)}
				<span class="occ {h.state}" title="{h.state} on {fmtLong(h.on)}">{h.state === "done" ? "✓" : "→"} {fmt(h.at)}</span>
			{/each}
			<span class="occ current" class:overdue>● {fmt(series.current)}</span>
			{#each next3 as n (n)}
				<span class="occ future">○ {fmt(n)}</span>
			{/each}
			<span class="occ more">…</span>
		</div>
		<div class="actions">
			<button class="act primary" onclick={() => finish("done")}>✓ Complete this occurrence</button>
			<button class="act" onclick={() => finish("skipped")}>Skip</button>
			{#if series.history.length}<button class="act" onclick={undo}>Undo</button>{/if}
		</div>
		{#if flash}<p class="flash">{flash}</p>{/if}
	{/if}

	{#if open}
		<div class="pop">
			<div class="pop-head">
				<span class="pop-name">Repeat</span>
				{#if series}<button class="pop-rm" onclick={clear}>Clear</button>{/if}
			</div>

			<div class="field">
				<span class="lbl">Every</span>
				<input class="n" type="number" min="1" max="99" bind:value={draft.interval} oninput={() => (draft.interval = Math.max(1, Math.floor(draft.interval || 1)))} />
				<select bind:value={draft.freq}>
					<option value="day">{draft.interval === 1 ? "day" : "days"}</option>
					<option value="week">{draft.interval === 1 ? "week" : "weeks"}</option>
					<option value="month">{draft.interval === 1 ? "month" : "months"}</option>
					<option value="year">{draft.interval === 1 ? "year" : "years"}</option>
				</select>
			</div>

			{#if draft.freq === "week"}
				<div class="field">
					<span class="lbl">On</span>
					<div class="days">
						{#each WD as d, i (d)}
							<button class="day" class:on={draft.weekdays.includes(i)} onclick={() => toggleWeekday(i)}>{d[0]}</button>
						{/each}
					</div>
				</div>
			{/if}

			{#if draft.freq === "month"}
				<div class="field">
					<span class="lbl">On</span>
					<div class="seg">
						<button class:on={draft.monthly === "date"} onclick={() => (draft.monthly = "date")}>the {new Date(draft.start).getDate()}{suffix(new Date(draft.start).getDate())}</button>
						<button class:on={draft.monthly === "weekday"} onclick={() => (draft.monthly = "weekday")}>the {ORD[ordinalOf(draft.start)]} {WD[new Date(draft.start).getDay()]}</button>
					</div>
				</div>
			{/if}

			<div class="field">
				<span class="lbl">At</span>
				<input type="time" value={hhmm(draft.time)} onchange={(e) => setTime(e.currentTarget.value)} />
				{#if dueDate}<span class="hint">counted from the due date</span>{/if}
			</div>

			<div class="summary">
				<div class="desc">{describe(draft)}</div>
				<div class="next">
					{#each preview as p, i (p)}<span class:first={i === 0}>{fmtLong(p)}</span>{/each}
					<span>…</span>
				</div>
			</div>

			<div class="pop-foot">
				<button class="act" onclick={() => (open = false)}>Cancel</button>
				<button class="act primary" onclick={save}>{series ? "Update" : "Repeat"}</button>
			</div>
		</div>
		<button class="backdrop" aria-label="Close" onclick={() => (open = false)}></button>
	{/if}
</div>

<style>
	.repeat {
		position: relative;
		margin: -8px 0 14px 48px;
		font-size: 13px;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.cell {
		border: none;
		background: none;
		color: var(--fg);
		padding: 2px 4px;
		border-radius: 6px;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: 13px;
	}
	.cell:hover {
		background: var(--hover);
	}
	.cell.empty {
		color: var(--muted);
		opacity: 0.7;
	}
	.glyph {
		font-size: 14px;
		color: var(--accent);
	}
	.cell.empty .glyph {
		color: inherit;
	}
	.sep {
		color: var(--border);
	}
	.overdue {
		color: var(--red, #e5484d);
	}
	.preview-tag {
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--muted);
		border: 1px dashed var(--border);
		border-radius: 4px;
		padding: 1px 5px;
		opacity: 0.8;
	}
	.strip {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px;
		padding-left: 4px;
	}
	.occ {
		font-size: 12px;
		line-height: 20px;
		padding: 0 7px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.05);
		color: var(--muted);
		white-space: nowrap;
	}
	.occ.done {
		color: var(--fg);
		background: rgba(120, 220, 150, 0.14);
		text-decoration: line-through;
		text-decoration-color: rgba(255, 255, 255, 0.35);
	}
	.occ.skipped {
		opacity: 0.6;
		text-decoration: line-through;
	}
	.occ.current {
		color: var(--fg);
		background: rgba(120, 150, 255, 0.18);
		border: 1px solid rgba(120, 150, 255, 0.4);
	}
	.occ.current.overdue {
		background: rgba(229, 72, 77, 0.16);
		border-color: rgba(229, 72, 77, 0.45);
	}
	.occ.more {
		background: none;
	}
	.actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 6px;
		padding-left: 4px;
	}
	.act {
		border: 1px solid var(--border);
		background: none;
		color: var(--fg);
		font-size: 12px;
		padding: 3px 9px;
		border-radius: 6px;
		cursor: pointer;
	}
	.act:hover {
		background: var(--hover);
	}
	.act.primary {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}
	.hint {
		font-size: 11.5px;
		color: var(--muted);
		margin-left: 4px;
	}
	.flash {
		margin: 0;
		padding: 6px 10px;
		border-radius: 6px;
		background: rgba(120, 220, 150, 0.1);
		border: 1px solid rgba(120, 220, 150, 0.25);
		font-size: 12.5px;
	}

	/* Popover - same shell as FeaturedProps' .pop */
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
		gap: 10px;
	}
	.pop-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.pop-name {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.pop-rm {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 11px;
		cursor: pointer;
	}
	.pop-rm:hover {
		color: var(--red, #e5484d);
	}
	.field {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 6px;
	}
	.lbl {
		width: 52px;
		flex: none;
		font-size: 12px;
		color: var(--muted);
	}
	.n {
		width: 48px;
	}
	input,
	select {
		background: rgba(255, 255, 255, 0.05);
		color: var(--fg);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 3px 6px;
		font-size: 12.5px;
		font: inherit;
	}
	input[type="time"] {
		color-scheme: dark;
	}
	.days {
		display: flex;
		gap: 3px;
	}
	.day {
		width: 26px;
		height: 26px;
		border-radius: 50%;
		border: 1px solid var(--border);
		background: none;
		color: var(--muted);
		font-size: 11px;
		cursor: pointer;
	}
	.day.on {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}
	.seg {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 6px;
		overflow: hidden;
	}
	.seg button {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 12px;
		padding: 3px 9px;
		cursor: pointer;
	}
	.seg button + button {
		border-left: 1px solid var(--border);
	}
	.seg button.on {
		color: var(--fg);
		background: rgba(120, 150, 255, 0.18);
	}
	.summary {
		border-top: 1px solid var(--border);
		padding-top: 8px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.desc {
		font-size: 12.5px;
	}
	.next {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 10px;
		font-size: 11.5px;
		color: var(--muted);
	}
	.next .first {
		color: var(--fg);
	}
	.pop-foot {
		display: flex;
		justify-content: flex-end;
		gap: 6px;
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
		.repeat {
			margin: -4px 16px 10px;
		}
		.pop {
			width: min(360px, calc(100vw - 32px));
		}
	}
</style>
