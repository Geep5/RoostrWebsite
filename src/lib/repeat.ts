/**
 * Words for a recurring object's rule, shared by the Repeat editor and the
 * table's Repeat column so both say "Every week on Wed at 9:00 AM" the same
 * way. The engine owns the rule and the occurrence math (core/repeat.odin).
 */
import type { RepeatFreq, RepeatJSON } from "$lib/types";

export const DAY = 86_400_000;
export const MIN = 60_000;
export const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
export const ORD = ["", "1st", "2nd", "3rd", "4th", "last"];

/** The editable shape: the stored rule with `anchor` as local start-of-day ms instead of a day index. */
export interface RepeatDraft {
	freq: RepeatFreq;
	interval: number;
	weekdays: number[];
	monthly: "date" | "weekday";
	anchor: number;
	/** Day/week/month/year: the day's occurrence times (minutes after local midnight). */
	times: number[];
	/** Minute/hour: the hours the grid runs in; null = the whole day. */
	window: [number, number] | null;
}

export const DAY_MINUTES = 1440;
export const isSubDaily = (freq: RepeatFreq) => freq === "minute" || freq === "hour";

export const sod = (ms: number) => {
	const d = new Date(ms);
	d.setHours(0, 0, 0, 0);
	return d.getTime();
};
/** Local midnight of a wall-clock day index (the engine's `anchor`). */
export const dayToLocal = (day: number) => new Date(1970, 0, 1 + day).getTime();
export const fmt = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
export const fmtTime = (minutes: number) => new Date(sod(Date.now()) + minutes * MIN).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
export const ordinalOf = (ms: number) => {
	const d = new Date(ms);
	const n = Math.ceil(d.getDate() / 7);
	const lastOfMonth = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
	return d.getDate() + 7 > lastOfMonth ? 5 : n;
};
export const suffix = (n: number) => (n % 10 === 1 && n !== 11 ? "st" : n % 10 === 2 && n !== 12 ? "nd" : n % 10 === 3 && n !== 13 ? "rd" : "th");
/** Times as the engine stores them: ascending, each once. */
export const sortedTimes = (times: number[]) => [...new Set(times)].sort((a, b) => a - b);

/** A rule that runs more than once on a day, so "when next" needs the time, not just the date. */
export const manyADay = (r: { freq: RepeatFreq; times: number[] }) => isSubDaily(r.freq) || r.times.length > 1;

export const toDraft = (r: RepeatJSON): RepeatDraft => ({
	freq: r.freq,
	interval: r.interval,
	weekdays: [...r.weekdays],
	monthly: r.monthly,
	anchor: dayToLocal(r.anchor),
	// A minute/hour rule has no times; switching it to daily starts from 9:00.
	times: r.times.length ? [...r.times] : [9 * 60],
	// The engine stores the whole day as [0, 1439]: that is no window.
	window: r.window && (r.window[0] > 0 || r.window[1] < DAY_MINUTES - 1) ? [r.window[0], r.window[1]] : null,
});

/** "Mon–Fri" for a run of three or more days, else "Mon, Wed". */
function dayList(weekdays: number[]): string {
	const wd = [...new Set(weekdays)].sort((a, b) => a - b);
	const run = wd.length >= 3 && wd.every((x, i) => i === 0 || x === wd[i - 1] + 1);
	return run ? `${WD[wd[0]]}–${WD[wd[wd.length - 1]]}` : wd.map((x) => WD[x]).join(", ");
}

export function describeDraft(d: RepeatDraft): string {
	const every = d.interval === 1 ? "Every" : `Every ${d.interval}`;
	const unit = d.interval === 1 ? d.freq : `${d.freq}s`;
	if (isSubDaily(d.freq)) {
		const days = d.weekdays.length ? ` on ${dayList(d.weekdays)}` : "";
		const span = d.window ? `, ${fmtTime(d.window[0])}–${fmtTime(d.window[1])}` : "";
		return `${every} ${unit}${days}${span}`;
	}
	const anchor = new Date(d.anchor);
	let s: string;
	if (d.freq === "week") {
		const wd = [...d.weekdays].sort();
		if (wd.length === 5 && wd.join() === "1,2,3,4,5" && d.interval === 1) s = "Every weekday";
		else s = `${every} ${unit} on ${wd.length ? dayList(wd) : WD[anchor.getDay()]}`;
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
	return `${s} at ${sortedTimes(d.times).map(fmtTime).join(", ")}`;
}

/** A table cell's words: the rule and when it next runs (with the time when it runs more than once a day). */
export function describeRepeat(rule: RepeatJSON): string {
	const when = manyADay(rule) ? `${fmt(rule.next)}, ${new Date(rule.next).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}` : fmt(rule.next);
	return `${describeDraft(toDraft(rule))} · next ${when}`;
}
