/**
 * Descriptor cards, read from the vault.
 *
 * A card says what a skill or a login IS - its name, what inputs it needs,
 * which of them are secret, how it authenticates. The harness publishes it
 * from its catalogs (`harness/src/descriptors.ts`), the core decodes the
 * protobuf and serves the result as `descriptor` on the object, and this
 * module only caches it.
 *
 * What it replaces: a hand-copied list in `serving.ts` that admitted in its
 * own comment to mirroring two TypeScript files in another package, plus the
 * harness's `/credentials` endpoint as the source of form shape - which meant
 * an unpaired browser or a phone could not even name what X needs.
 *
 * No secret value is here, or anywhere in the schema: `secret` says a value
 * exists on some machine, and the value never leaves it.
 */

import { fetchObject, fetchQuery } from "$lib/api";
import { cardOf, capabilityCardsOf, labelOf, type Card, type DescribedObject } from "$lib/card-shape";

export type { Card, CardField, DescribedObject } from "$lib/card-shape";

let cache: Card[] = [];
let loaded = false;

/**
 * Every card in the vault. Cached because a card changes only when a machine
 * republishes its catalog; `force` re-reads after that.
 */
export async function loadCards(force = false): Promise<Card[]> {
	if (loaded && !force) return cache;
	const rows = await fetchQuery({ filters: [{ key: "typeKey", condition: "equal", value: "descriptor" }], limit: 200 });
	const objects = await Promise.all(rows.records.map((r) => fetchObject(r.id) as Promise<DescribedObject>));
	cache = objects.map(cardOf).filter((c): c is Card => c !== undefined);
	cache.sort((a, b) => a.name.localeCompare(b.name));
	loaded = true;
	return cache;
}

/** Cards already loaded; empty before the first `loadCards`. */
export const cards = (): Card[] => cache;

export const cardFor = (key: string): Card | undefined => cache.find((c) => c.key === key);

export const capabilityLabel = (key: string): string => labelOf(cache, key);

export const capabilityCards = (): Card[] => capabilityCardsOf(cache);
