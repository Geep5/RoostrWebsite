/**
 * Descriptor card projection - pure, and deliberately store-free.
 *
 * `$lib/api` reaches the reactive store, and `$state` does not exist outside
 * the Svelte compiler, so anything testable cannot import it. The loading and
 * caching half lives in `$lib/cards`.
 */

import type { ObjectJSON } from "$lib/types";

export interface CardField {
	key: string;
	label: string;
	secret: boolean;
	/** "text" | "password" | "url" | "email"; "" from a newer writer. */
	format: string;
	note: string;
}

export interface Card {
	id: string;
	key: string;
	name: string;
	description: string;
	/** "skill" | "integration" | "agent"; "" from a newer writer. */
	kind: string;
	fields: CardField[];
	/** "browser_profile" | "oauth" | "api_key" | "none" */
	auths: string[];
	check?: { command: string; expectContains: string; timeoutMs: number };
	install?: { prompt: string; uninstallPrompt: string; docsUrl: string };
	version: string;
	author: string;
}

/**
 * An object's card, as the core decoded it (`descriptor` on the object JSON).
 * Nothing here parses protobuf - that lives once, in the core.
 */
export type DescribedObject = ObjectJSON & {
	descriptor?: Partial<Omit<Card, "id">> & { key?: string };
};

export function cardOf(object: DescribedObject): Card | undefined {
	const card = object.descriptor;
	// A row whose card failed to decode, or was never published, is not a
	// card: rendering a form from half of one would ask for the wrong thing.
	if (!card?.key) return undefined;
	return {
		id: object.id,
		key: card.key,
		name: card.name || card.key,
		description: card.description ?? "",
		kind: card.kind ?? "",
		fields: card.fields ?? [],
		auths: card.auths ?? [],
		check: card.check,
		install: card.install,
		version: card.version ?? "",
		author: card.author ?? "",
	};
}

/** Cards a machine can hold: skills and logins, never agents. */
export const capabilityCardsOf = (cards: Card[]): Card[] =>
	cards.filter((c) => c.kind === "skill" || c.kind === "integration");

/**
 * A capability's display name from a card list. Falls back to the key, which
 * is the honest answer for a machine publishing something no card describes -
 * a key from a newer harness must still render.
 */
export const labelOf = (cards: Card[], key: string): string => cards.find((c) => c.key === key)?.name ?? key;
