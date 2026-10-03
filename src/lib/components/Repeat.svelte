<script lang="ts">
	/**
	 * Recurring objects. The engine owns the rule (`object.fields.repeat`)
	 * and the occurrence math; this component renders when it next runs and
	 * lets a person edit the cadence or turn repeating off. Advancing an
	 * occurrence is the agent's job (`occurrence_complete`), never a button.
	 *
	 * The rule is deliberately small: every N minutes or hours across the
	 * day (or only between two times, optionally only on some weekdays), or
	 * every N days/weeks/months/years on <weekdays | day of month> at one or
	 * more times of day, counted from an anchor day.
	 */
	import type { ObjectJSON, RepeatFreq } from "$lib/types";
	import { DAY, DAY_MINUTES, MIN, ORD, WD, describeDraft as describe, isSubDaily, manyADay, ordinalOf, sod, sortedTimes, suffix, toDraft, type RepeatDraft as Draft } from "$lib/repeat";
	import { repeatOf, guestAgents } from "$lib/types";
	import { note, repeat } from "$lib/api";
	import { store } from "$lib/data.svelte";
	import { isIOSBackend } from "$lib/client-backend";
	import { machineName, resolveServing, servingCopy } from "$lib/serving";

	let {
		object,
		onchanged,
	}: {
		object: ObjectJSON;
		onchanged: () => Promise<void>;
	} = $props();


	const rule = $derived(repeatOf(object.fields));
	let open = $state(false);

	// The agent that runs each occurrence is the object's `assignee`, chosen
	// from its guest list (`agent` property). An empty guest list means there
	// is nobody to run it, so the editor explains that instead of offering a
	// rule that can never fire.
	const guests = $derived(guestAgents(object.fields).map((id) => store.agents.find((a) => a.id === id)).filter((a): a is NonNullable<typeof a> => !!a));
	const hasGuests = $derived(guestAgents(object.fields).length > 0);
	let assignee = $state<string>("");
	const assigneeName = $derived(guests.find((a) => a.id === assignee)?.name ?? "");

	// Agent-owned occurrences run on the machine the engine resolves for
	// this object (`$lib/serving`), never on a phone; the cell says so and
	// names the machine when it can, plus the warning when nobody can.
	const agentOwned = $derived(!!(object.fields["assignee"] ?? object.fields["agent"]));
	let servingName = $state("");
	let servingWarning = $state("");
	$effect(() => {
		const current = object;
		if (!agentOwned) {
			servingName = "";
			servingWarning = "";
			return;
		}
		void (async () => {
			try {
				const { serving, machines } = await resolveServing(current);
				servingName = machineName(machines, serving.machineId);
				const copy = servingCopy(serving, machines);
				servingWarning = copy.warning ? copy.text : "";
			} catch {
				servingName = "";
				servingWarning = "";
			}
		})();
	});
	let busy = $state(false);
	let error = $state("");
	// Navigating object -> object closes a stale editor.
	$effect(() => {
		void object.id;
		open = false;
		error = "";
	});

	// ── Dates ─────────────────────────────────────────────────────
	const fmtDay = (ms: number) => new Date(ms).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
	const fmtLong = (ms: number) => new Date(ms).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
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

	// ── Preview only ──────────────────────────────────────────────
	// The engine computes the real `next` (core/repeat.odin). These mirror
	// its stepping so the popover can show the upcoming occurrences while
	// the user edits; nothing here is written back.

	/** Whether the rule runs on the local day starting at `ms` (the engine's repeat_day_fits). */
	function dayFits(d: Draft, ms: number): boolean {
		if (isSubDaily(d.freq)) return d.weekdays.length === 0 || d.weekdays.includes(new Date(ms).getDay());
		return d.freq !== "week" || d.weekdays.includes(new Date(ms).getDay());
	}

	/** Local midnight `n` days after local midnight `ms`: the half day of slack absorbs a DST hour either way. */
	const addDays = (ms: number, n: number) => sod(ms + n * DAY + DAY / 2);

	/** Next occurrence day strictly after `from` (an occurrence day). */
	function stepFrom(d: Draft, from: number): number {
		const anchor = new Date(d.anchor);
		switch (d.freq) {
			case "minute":
			case "hour": {
				// The interval spaces the times within a day; every allowed day runs.
				let cur = addDays(from, 1);
				for (let i = 0; i < 7 && !dayFits(d, cur); i++) cur = addDays(cur, 1);
				return cur;
			}
			case "day":
				return addDays(from, d.interval);
			case "week": {
				const days = d.weekdays.length ? d.weekdays : [anchor.getDay()];
				const weekStart = (ms: number) => addDays(ms, -new Date(ms).getDay());
				const w0 = weekStart(d.anchor);
				let cur = addDays(from, 1);
				for (let i = 0; i < 400; i++, cur = addDays(cur, 1)) {
					const weeks = Math.round((weekStart(cur) - w0) / (7 * DAY));
					if (weeks % d.interval === 0 && days.includes(new Date(cur).getDay())) return cur;
				}
				return cur;
			}
			case "month": {
				if (d.monthly === "weekday") {
					const m = new Date(from);
					m.setDate(1);
					m.setMonth(m.getMonth() + d.interval);
					return nthWeekday(m.getTime(), anchor.getDay(), ordinalOf(d.anchor));
				}
				return addMonths(from, d.interval, anchor.getDate());
			}
			case "year": {
				const y = new Date(from);
				y.setFullYear(y.getFullYear() + d.interval);
				return sod(y.getTime());
			}
		}
	}

	/** The occurrence times (minutes after local midnight) on a day the rule runs: the stored times, or the minute/hour grid. */
	function dayTimes(d: Draft): number[] {
		if (!isSubDaily(d.freq)) return sortedTimes(d.times);
		const [from, until] = d.window ?? [0, DAY_MINUTES - 1];
		const period = d.interval * (d.freq === "hour" ? 60 : 1);
		const out: number[] = [];
		for (let t = from; t <= until; t += period) out.push(t);
		return out;
	}

	const view = $derived(rule ? toDraft(rule) : null);
	const overdue = $derived(!!rule && rule.next < Date.now());

	// ── Editing ───────────────────────────────────────────────────
	const FREQS: RepeatFreq[] = ["minute", "hour", "day", "week", "month", "year"];
	const maxInterval = (freq: RepeatFreq) => (freq === "hour" ? 23 : 999);
	const seed = (): Draft => {
		const anchor = sod(Date.now());
		return { freq: "week", interval: 1, weekdays: [new Date(anchor).getDay()], monthly: "date", anchor, times: [9 * 60], window: null };
	};
	let draft = $state<Draft>(seed());
	/** The next five occurrences from now: the first fitting day from the anchor, then each step. */
	const preview = $derived.by(() => {
		const times = dayTimes(draft);
		const now = Date.now();
		const out: number[] = [];
		let day = dayFits(draft, draft.anchor) ? draft.anchor : stepFrom(draft, draft.anchor);
		for (let i = 0; i < 400 && out.length < 5; i++, day = stepFrom(draft, day)) {
			for (const t of times) if (out.length < 5 && day + t * MIN > now) out.push(day + t * MIN);
		}
		return out;
	});
	/** The engine refuses a window that ends before it starts. */
	const invalid = $derived(isSubDaily(draft.freq) && draft.window && draft.window[0] > draft.window[1] ? "The window ends before it starts." : "");

	function openEditor() {
		draft = rule ? toDraft(rule) : seed();
		// Seed the assignee from the object's current guest list every open:
		// an agent added after the last refresh must be pickable now.
		const g = guestAgents(object.fields);
		assignee = g.includes(object.fields["assignee"]?.stringValue ?? "") ? (object.fields["assignee"]?.stringValue ?? "") : (g[0] ?? "");
		open = true;
	}
	function setFreq(value: string) {
		const freq = FREQS.find((f) => f === value);
		if (!freq) return;
		// Minute/hour weekdays filter the days (none = every day); a week's
		// weekdays are the days it runs. Neither carries over to the other.
		if (isSubDaily(freq) !== isSubDaily(draft.freq)) draft.weekdays = isSubDaily(freq) ? [] : [new Date(draft.anchor).getDay()];
		draft.freq = freq;
		draft.interval = Math.min(draft.interval, maxInterval(freq));
	}
	/** A local day as the date input's "YYYY-MM-DD". */
	function ymd(ms: number): string {
		const d = new Date(ms);
		return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
	}
	/** Start the cadence on another day. A week still on its default day (the old start's weekday) follows it. */
	function setAnchor(value: string) {
		const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
		if (!m) return;
		const next = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime();
		const oldDay = new Date(draft.anchor).getDay();
		if (draft.freq === "week" && draft.weekdays.length === 1 && draft.weekdays[0] === oldDay) draft.weekdays = [new Date(next).getDay()];
		draft.anchor = next;
	}
	function toggleWeekday(d: number) {
		draft.weekdays = draft.weekdays.includes(d) ? draft.weekdays.filter((x) => x !== d) : [...draft.weekdays, d];
		// A week needs a day to run on; minute/hour with none run every day.
		if (!draft.weekdays.length && draft.freq === "week") draft.weekdays = [d];
	}
	/** "HH:MM" from a time input as minutes after midnight, or null while it is incomplete. */
	function minutesOf(value: string): number | null {
		const [h, m] = value.split(":").map(Number);
		return Number.isInteger(h) && Number.isInteger(m) ? h * 60 + m : null;
	}
	function setTime(i: number, value: string) {
		const t = minutesOf(value);
		if (t !== null) draft.times[i] = t;
	}
	/** A new time an hour after the last, skipping ones already listed. */
	function addTime() {
		let t = ((draft.times[draft.times.length - 1] ?? 9 * 60) + 60) % DAY_MINUTES;
		for (let i = 0; i < 24 && draft.times.includes(t); i++) t = (t + 60) % DAY_MINUTES;
		draft.times = [...draft.times, t];
	}
	function setWindow(edge: 0 | 1, value: string) {
		const t = minutesOf(value);
		if (t === null || !draft.window) return;
		draft.window = edge === 0 ? [t, draft.window[1]] : [draft.window[0], t];
	}

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
	function sameRule(a: Draft, b: Draft): boolean {
		return (
			a.freq === b.freq &&
			a.interval === b.interval &&
			a.monthly === b.monthly &&
			a.anchor === b.anchor &&
			sortedTimes(a.times).join() === sortedTimes(b.times).join() &&
			(a.window?.join() ?? "") === (b.window?.join() ?? "") &&
			[...a.weekdays].sort().join() === [...b.weekdays].sort().join()
		);
	}
	async function save() {
		if (invalid) return;
		open = false;
		// Re-saving an identical rule would recompute `next` from the anchor
		// and could resurrect an occurrence that was already completed.
		if (view && sameRule(view, draft)) {
			// The rule is unchanged; the assignee may still have moved.
			await saveAssignee();
			return;
		}
		const d = $state.snapshot(draft);
		const sub = isSubDaily(d.freq);
		await run(async () => {
			await saveAssignee();
			await repeat.set(object.id, {
				freq: d.freq,
				interval: d.interval,
				weekdays: d.freq === "week" || sub ? d.weekdays : [],
				monthly: d.monthly,
				times: sub ? [] : sortedTimes(d.times),
				// No window = the whole day.
				...(sub && d.window ? { window: d.window } : {}),
				tz: Intl.DateTimeFormat().resolvedOptions().timeZone,
				// Noon, so the engine lands on the same wall-clock day even when
				// today's UTC offset differs from the anchor day's (DST).
				anchor_ms: d.anchor + DAY / 2,
			});
		});
	}
	/** Who runs each occurrence: the chosen guest, or clear the field. */
	async function saveAssignee() {
		const current = object.fields["assignee"]?.stringValue ?? "";
		if (assignee === current) return;
		if (assignee) await note.setField(object.id, "assignee", { stringValue: assignee });
		else await note.deleteField(object.id, "assignee");
	}
	async function clear() {
		open = false;
		await run(() => repeat.clear(object.id));
	}
</script>

<div class="repeat" class:active={!!rule}>
	<div class="row-wrap">
		<button class="row" class:empty={!rule} onclick={openEditor} title="Repeat">
			<span class="row-label">
				<span class="glyph">↻</span>
				<span class="row-name">Repeat</span>
			</span>
			<span class="row-value">
				{#if rule && view}
					<span class="summary-line">{describe(view)}</span>
				{:else}
					<span class="placeholder">Off</span>
				{/if}
			</span>
		</button>
	</div>

	{#if rule}
		<!-- One quiet line under the rule: when next, and who runs it where. -->
		<p class="meta">
			<span class:overdue>{overdue ? `Overdue since ${fmtLong(rule.next)}` : `Next ${manyADay(rule) ? fmtLong(rule.next) : fmtDay(rule.next)}`}</span>
			{#if agentOwned}
				<span>{#if assigneeName}{assigneeName} on {servingName || "its machine"}{:else}on {servingName || "its agent's machine"}{/if}{isIOSBackend ? " · not on this device" : ""}{#if servingWarning}<span class="overdue"> · {servingWarning}</span>{/if}</span>
			{/if}
		</p>
		{#if rule.fired_at !== undefined || rule.last_run}
			<p class="meta">
				{#if rule.fired_at !== undefined}
					<span>fired {fmtLong(rule.fired_at)}{rule.fired_by ? ` by ${rule.fired_by}` : ""}</span>
				{/if}
				{#if rule.last_run}
					<span>
						last run {fmtLong(rule.last_run.at)}{rule.last_run.machine ? ` on ${rule.last_run.machine}` : ""}{#if rule.last_run.error}<span class="overdue"> · {rule.last_run.error}</span>{/if}
					</span>
				{/if}
			</p>
		{/if}
	{/if}
	{#if error}<p class="err">{error}</p>{/if}

	{#if open}
		<div class="pop">
			<div class="pop-head">
				<span class="pop-name">Repeat</span>
			</div>

			{#if !hasGuests}
				<p class="hint">
					Add an agent to this object's <strong>Agent</strong> property first.
					A repeat needs an agent on the guest list to run each occurrence.
				</p>
			{:else}
				<div class="field">
					<span class="lbl">Agent</span>
					<select bind:value={assignee}>
						{#each guests as a (a.id)}
							<option value={a.id}>{a.icon ? `${a.icon} ` : ""}{a.name || "Agent"}</option>
						{/each}
					</select>
				</div>

				<div class="field">
					<span class="lbl">Every</span>
					<input
						class="n"
						type="number"
						min="1"
						max={maxInterval(draft.freq)}
						bind:value={draft.interval}
						oninput={() => (draft.interval = Math.min(maxInterval(draft.freq), Math.max(1, Math.floor(draft.interval || 1))))}
					/>
					<select value={draft.freq} onchange={(e) => setFreq(e.currentTarget.value)}>
						<option value="minute">{draft.interval === 1 ? "minute" : "minutes"}</option>
						<option value="hour">{draft.interval === 1 ? "hour" : "hours"}</option>
						<option value="day">{draft.interval === 1 ? "day" : "days"}</option>
						<option value="week">{draft.interval === 1 ? "week" : "weeks"}</option>
						<option value="month">{draft.interval === 1 ? "month" : "months"}</option>
						<option value="year">{draft.interval === 1 ? "year" : "years"}</option>
					</select>
				</div>

				{#if !isSubDaily(draft.freq)}
					<!-- The day the cadence counts from: it decides "the 1st" vs "the 3rd",
					     a yearly date, and which weeks an every-2-weeks lands on. It
					     used to be fixed to the day the repeat was made. -->
					<div class="field">
						<span class="lbl">Starts</span>
						<input type="date" value={ymd(draft.anchor)} onchange={(e) => setAnchor(e.currentTarget.value)} />
					</div>
				{/if}

				{#if isSubDaily(draft.freq)}
					<div class="field">
						<span class="lbl">Hours</span>
						<div class="seg">
							<button class:on={!draft.window} onclick={() => (draft.window = null)}>All day</button>
							<button class:on={!!draft.window} onclick={() => (draft.window ??= [8 * 60, 20 * 60])}>Only between</button>
						</div>
					</div>
					{#if draft.window}
						<div class="field">
							<span class="lbl"></span>
							<input type="time" value={hhmm(draft.window[0])} onchange={(e) => setWindow(0, e.currentTarget.value)} aria-label="From" />
							<span class="to">–</span>
							<input type="time" value={hhmm(draft.window[1])} onchange={(e) => setWindow(1, e.currentTarget.value)} aria-label="Until" />
						</div>
					{/if}
				{/if}

				{#if draft.freq === "week" || isSubDaily(draft.freq)}
					<div class="field">
						<span class="lbl">On</span>
						<div class="days">
							{#each WD as d, i (d)}
								<button class="day" class:on={draft.weekdays.includes(i)} onclick={() => toggleWeekday(i)}>{d[0]}</button>
							{/each}
						</div>
						{#if !draft.weekdays.length}<span class="aside">every day</span>{/if}
					</div>
				{/if}

				{#if draft.freq === "month"}
					<div class="field">
						<span class="lbl">On</span>
						<div class="seg">
							<button class:on={draft.monthly === "date"} onclick={() => (draft.monthly = "date")}>the {new Date(draft.anchor).getDate()}{suffix(new Date(draft.anchor).getDate())}</button>
							<button class:on={draft.monthly === "weekday"} onclick={() => (draft.monthly = "weekday")}>the {ORD[ordinalOf(draft.anchor)]} {WD[new Date(draft.anchor).getDay()]}</button>
						</div>
					</div>
				{/if}

				{#if !isSubDaily(draft.freq)}
					<div class="field top">
						<span class="lbl">At</span>
						<div class="time-list">
							{#each draft.times as t, i (i)}
								<span class="time">
									<input type="time" value={hhmm(t)} onchange={(e) => setTime(i, e.currentTarget.value)} />
									{#if draft.times.length > 1}
										<button class="time-rm" aria-label="Remove time" title="Remove time" onclick={() => (draft.times = draft.times.filter((_, j) => j !== i))}>×</button>
									{/if}
								</span>
							{/each}
							<button class="add-time" onclick={addTime}>+ Add time</button>
						</div>
					</div>
				{/if}

				<div class="summary">
					<div class="desc" class:overdue={!!invalid}>{invalid || describe(draft)}</div>
					<div class="next">
						{#each preview as p, i (p)}<span class:first={i === 0}>{fmtLong(p)}</span>{/each}
						<span>…</span>
					</div>
				</div>
			{/if}

			<div class="pop-foot">
				{#if rule}<button class="pop-rm" disabled={busy} onclick={() => void clear()}>Turn off repeating</button>{/if}
				<span class="spacer"></span>
				<button class="act" onclick={() => (open = false)}>Cancel</button>
				<button class="act primary" disabled={busy || !hasGuests || !!invalid} onclick={() => void save()}>{rule ? "Update" : "Repeat"}</button>
			</div>
		</div>
		<button class="backdrop" aria-label="Close" onclick={() => (open = false)}></button>
	{/if}
</div>

<style>
	.repeat {
		position: relative;
		font-size: 13px;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		min-height: 28px;
		padding: 3px 22px 3px 4px;
		border: none;
		background: none;
		border-radius: 6px;
		color: var(--fg);
		font-size: 13px;
		text-align: left;
		cursor: pointer;
	}
	.row:hover {
		background: var(--hover);
	}
	.row-label {
		display: flex;
		align-items: center;
		gap: 7px;
		flex: none;
		min-width: 0;
		color: var(--muted);
	}
	.row-name {
		font-size: 13px;
		white-space: nowrap;
	}
	.row-value {
		margin-left: auto;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 4px;
		min-width: 0;
		text-align: right;
	}
	.placeholder {
		color: var(--muted);
		opacity: 0.65;
	}
	.glyph {
		font-size: 14px;
		color: var(--accent);
	}
	.row.empty .glyph {
		color: inherit;
	}
	.summary-line {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.overdue {
		color: var(--red, #e5484d);
	}
	.meta > span + span::before {
		content: "·";
		margin-right: 6px;
		color: var(--muted);
	}
	.meta {
		margin: 0;
		padding-left: 25px;
		display: flex;
		flex-wrap: wrap;
		gap: 2px 6px;
		font-size: 11.5px;
		color: var(--muted);
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
	.act:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.err {
		margin: 0;
		padding: 6px 10px;
		border-radius: 6px;
		background: rgba(229, 72, 77, 0.1);
		border: 1px solid rgba(229, 72, 77, 0.3);
		font-size: 12.5px;
	}

	/* Popover - same shell as PropertiesPane's .pop */
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
	.field.top {
		align-items: flex-start;
	}
	.field.top .lbl {
		padding-top: 5px;
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
	input[type="time"],
	input[type="date"] {
		color-scheme: dark;
	}
	.to,
	.aside {
		font-size: 12px;
		color: var(--muted);
	}
	.time-list {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
	}
	.time {
		display: inline-flex;
		align-items: center;
		gap: 2px;
	}
	.time-rm {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 14px;
		line-height: 1;
		padding: 0 3px;
		cursor: pointer;
	}
	.time-rm:hover {
		color: var(--red, #e5484d);
	}
	.add-time {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 12px;
		padding: 3px 4px;
		cursor: pointer;
	}
	.add-time:hover {
		color: var(--fg);
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
		align-items: center;
		gap: 6px;
	}
	.spacer {
		flex: 1;
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
