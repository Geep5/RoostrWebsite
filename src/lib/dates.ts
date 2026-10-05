/** One emoji per month, so a date's month reads at a glance (index = Date#getMonth()). */
const MONTH_EMOJI = ["🍾", "❤️", "🍀", "🐰", "🌷", "☀️", "🧨", "🌊", "🎒", "🎃", "🦃", "⛄"];

/** A date as the app shows it everywhere: its month's emoji, then the locale date. */
export function showDate(ms: number, options?: Intl.DateTimeFormatOptions): string {
	const d = new Date(ms);
	return `${MONTH_EMOJI[d.getMonth()]} ${d.toLocaleDateString(undefined, options)}`;
}
