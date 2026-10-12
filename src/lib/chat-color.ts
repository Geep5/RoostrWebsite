/**
 * Chat bubble colors: the user's own (Settings, `userPrefs` in api.ts) and
 * each agent's `chat_color` property. Colors are #RRGGBB hex; text inside
 * a bubble is ink or white, whichever reads better on it.
 */

/** Roostr cream: the front page background. */
export const DEFAULT_CHAT_COLOR = "#EDCF7F";
export const INK = "#1B1B1B";
export const WHITE = "#FFFFFF";

/** A #RGB or #RRGGBB hex, normalized to upper-case #RRGGBB; null when invalid. */
export function normalizeHex(raw: string): string | null {
	const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(raw.trim());
	if (!m) return null;
	const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1];
	return `#${h.toUpperCase()}`;
}

/** WCAG relative luminance of a hex color (0 when invalid). */
export function luminance(hex: string): number {
	const h = normalizeHex(hex);
	if (!h) return 0;
	const [r, g, b] = [1, 3, 5].map((i) => {
		const c = parseInt(h.slice(i, i + 2), 16) / 255;
		return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio of two luminances. */
const contrast = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);

/** Ink or white: whichever has the higher contrast on `bg`. */
export function inkFor(bg: string): string {
	const l = luminance(bg);
	return contrast(l, luminance(INK)) >= contrast(l, 1) ? INK : WHITE;
}

/** CSS custom properties for a bubble of `bg`: background, text, and a tint for code/links that follows the text. */
export function bubbleVars(bg: string): string {
	const ink = inkFor(bg);
	const tint = ink === INK ? "rgba(0, 0, 0, 0.12)" : "rgba(255, 255, 255, 0.22)";
	return `--bubble-bg: ${bg}; --bubble-ink: ${ink}; --bubble-tint: ${tint};`;
}
