<script lang="ts">
	/**
	 * Recurring objects. The engine owns the rule (`object.fields.repeat`)
	 * and the occurrence math; this component renders when it next runs and
	 * lets a person edit the cadence or turn repeating off. Advancing an
	 * occurrence is the agent's job (`occurrence_complete`), never a button.
	 *
	 * The rule is deliberately small: every N <unit>, on <weekdays | day of
	 * month>, at <time of day>, counted from an anchor day.
	 */
	import type { ObjectJSON, RepeatFreq, RepeatJSON } from "$lib/types";
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

	/** The editable shape: the stored rule with `anchor` as local start-of-day ms instead of a day index. */
	interface Draft {
		freq: RepeatFreq;
		interval: number;
		weekdays: number[];
		monthly: "date" | "weekday";
		anchor: number;
		time: number;
	}

	const DAY = 86_400_000;
	const MIN = 60_000;
	const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
	const ORD = ["", "1st", "2nd", "3rd", "4th", "last"];

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
	const sod = (ms: number) => {
		const d = new Date(ms);
		d.setHours(0, 0, 0, 0);
		return d.getTime();
	};
	/** Local midnight of a wall-clock day index (the engine's `anchor`). */
	const dayToLocal = (day: number) => new Date(1970, 0, 1 + day).getTime();
	const fmt = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
	const fmtLong = (ms: number) => new Date(ms).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
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
	const suffix = (n: number) => (n % 10 === 1 && n !== 11 ? "st" : n % 10 === 2 && n !== 12 ? "nd" : n % 10 === 3 && n !== 13 ? "rd" : "th");

	// ── Preview only ──────────────────────────────────────────────
	// The engine computes the real `next` (core/repeat.odin). These mirror
	// its stepping so the popover can show the upcoming dates while the
	// user edits; nothing here is written back.

	/** Next occurrence day strictly after `from` (an occurrence day). */
	function stepFrom(d: Draft, from: number): number {
		const anchor = new Date(d.anchor);
		switch (d.freq) {
			case "day":
				return from + d.interval * DAY;
			case "week": {
				const days = d.weekdays.length ? d.weekdays : [anchor.getDay()];
				const weekStart = (ms: number) => sod(ms - new Date(ms).getDay() * DAY);
				const w0 = weekStart(d.anchor);
				let cur = from + DAY;
				for (let i = 0; i < 400; i++, cur += DAY) {
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

	/** The anchor day itself when it fits the rule and its time is still ahead; otherwise the next match. */
	function firstOccurrence(d: Draft): number {
		const fits = d.freq !== "week" || d.weekdays.includes(new Date(d.anchor).getDay());
		if (fits && d.anchor + d.time * MIN > Date.now()) return d.anchor;
		return stepFrom(d, d.anchor);
	}

	function describe(d: Draft): string {
		const every = d.interval === 1 ? "Every" : `Every ${d.interval}`;
		const unit = d.interval === 1 ? d.freq : `${d.freq}s`;
		const anchor = new Date(d.anchor);
		let s: string;
		if (d.freq === "week") {
			const wd = [...d.weekdays].sort();
			if (wd.length === 5 && wd.join() === "1,2,3,4,5" && d.interval === 1) s = "Every weekday";
			else s = `${every} ${unit} on ${wd.length ? wd.map((x) => WD[x]).join(", ") : WD[anchor.getDay()]}`;
		} else if (d.freq === "month") {
			s =
				d.monthly === "weekday"
					? `${every} ${unit} on the ${ORD[ordinalOf(d.anchor)]} ${WD[anchor.getDay()]}`
					: `${every} ${unit} on the ${anchor.getDate()}${suffix(anchor.getDate())}`;
		} else if (d.freq === "year") {
			s = `${every} ${unit} on ${fmt(d.anchor)}`;
		} else {
			s = `${every} ${unit}`;
		}
		return `${s} at ${fmtTime(d.time)}`;
	}

	const toDraft = (r: RepeatJSON): Draft => ({ freq: r.freq, interval: r.interval, weekdays: [...r.weekdays], monthly: r.monthly, anchor: dayToLocal(r.anchor), time: r.time });
	const view = $derived(rule ? toDraft(rule) : null);
	const overdue = $derived(!!rule && rule.next < Date.now());

	// ── Editing ───────────────────────────────────────────────────
	const seed = (): Draft => {
		const anchor = sod(Date.now());
		return { freq: "week", interval: 1, weekdays: [new Date(anchor).getDay()], monthly: "date", anchor, time: 9 * 60 };
	};
	let draft = $state<Draft>(seed());
	const preview = $derived.by(() => {
		const out = [firstOccurrence(draft)];
		while (out.length < 5) out.push(stepFrom(draft, out[out.length - 1]));
		return out.map((day) => day + draft.time * MIN);
	});

	function openEditor() {
		draft = rule ? toDraft(rule) : seed();
		// Seed the assignee from the object's current guest list every open:
		// an agent added after the last refresh must be pickable now.
		const g = guestAgents(object.fields);
		assignee = g.includes(object.fields["assignee"]?.stringValue ?? "") ? (object.fields["assignee"]?.stringValue ?? "") : (g[0] ?? "");
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
		return a.freq === b.freq && a.interval === b.interval && a.monthly === b.monthly && a.time === b.time && a.anchor === b.anchor && [...a.weekdays].sort().join() === [...b.weekdays].sort().join();
	}
	async function save() {
		open = false;
		// Re-saving an identical rule would recompute `next` from the anchor
		// and could resurrect an occurrence that was already completed.
		if (view && sameRule(view, draft)) {
			// The rule is unchanged; the assignee may still have moved.
			await saveAssignee();
			return;
		}
		const d = $state.snapshot(draft);
		await run(async () => {
			await saveAssignee();
			await repeat.set(object.id, {
				freq: d.freq,
				interval: d.interval,
				weekdays: d.freq === "week" ? d.weekdays : [],
				monthly: d.monthly,
				time: d.time,
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
	<div class="row">
		<button class="cell" class:empty={!rule} onclick={openEditor} title="Repeat">
			<span class="glyph">↻</span>
			{#if rule && view}
				{describe(view)}
				<span class="sep">·</span>
				<span class:overdue>{overdue ? "overdue since" : "next"} {fmtLong(rule.next)}</span>
			{:else}
				Does not repeat
			{/if}
		</button>
	</div>

	{#if rule}
		{#if agentOwned}
			<p class="meta">
				<span>{#if assigneeName}runs as {assigneeName} on {servingName || "its machine"}{:else}runs on {servingName || "the machine serving this space"}{/if}{isIOSBackend ? " · not on this device" : ""}{#if servingWarning}<span class="overdue"> · {servingWarning}</span>{/if}</span>
			</p>
		{/if}
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
							<button class:on={draft.monthly === "date"} onclick={() => (draft.monthly = "date")}>the {new Date(draft.anchor).getDate()}{suffix(new Date(draft.anchor).getDate())}</button>
							<button class:on={draft.monthly === "weekday"} onclick={() => (draft.monthly = "weekday")}>the {ORD[ordinalOf(draft.anchor)]} {WD[new Date(draft.anchor).getDay()]}</button>
						</div>
					</div>
				{/if}

				<div class="field">
					<span class="lbl">At</span>
					<input type="time" value={hhmm(draft.time)} onchange={(e) => setTime(e.currentTarget.value)} />
				</div>

				<div class="summary">
					<div class="desc">{describe(draft)}</div>
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
				<button class="act primary" disabled={busy || !hasGuests} onclick={() => void save()}>{rule ? "Update" : "Repeat"}</button>
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
	.meta {
		margin: 0;
		padding-left: 4px;
		display: flex;
		flex-wrap: wrap;
		gap: 2px 12px;
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
