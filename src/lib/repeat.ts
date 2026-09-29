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
	time: number;
}

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

export const toDraft = (r: RepeatJSON): RepeatDraft => ({ freq: r.freq, interval: r.interval, weekdays: [...r.weekdays], monthly: r.monthly, anchor: dayToLocal(r.anchor), time: r.time });

export function describeDraft(d: RepeatDraft): string {
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

/** A table cell's words: the rule and when it next runs. */
export function describeRepeat(rule: RepeatJSON): string {
	return `${describeDraft(toDraft(rule))} · next ${fmt(rule.next)}`;
}
