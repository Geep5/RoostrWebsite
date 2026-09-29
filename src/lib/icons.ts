import { store } from "$lib/data.svelte";
/** Object icon: the emoji when set, else the type glyph (Anytype's IconObject rule). */

export const TYPE_GLYPHS: Record<string, string> = {
	// Anytype bundle equivalents (heart types.json iconNames as emoji).
	page: "📄",
	note: "📝",
	task: "✅",
	person: "👤",
	project: "🔨",
	bookmark: "🔖",
	query: "🔍",
	set: "🔍",
	collection: "🗂️",
	peer: "◉",
	// An agent with no emoji of its own shows the robot, like the Agent type.
	agent: "🤖",
	channel: "◍",
	chat: "💬",
	credential: "🔑",
};

/** The icon a type shows: its type object's emoji (people choose these), else the built-in glyph. */
export function typeIcon(typeKey: string): string {
	return store.types.find((t) => t.key === typeKey && t.icon)?.icon || TYPE_GLYPHS[typeKey] || "";
}

/** An object's icon: its own emoji, else its type's. */
export function objectIcon(emoji: string | undefined, typeKey: string): string {
	return emoji || typeIcon(typeKey) || "•";
}
