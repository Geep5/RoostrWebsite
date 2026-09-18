/**
 * What a client is allowed to conclude from a descriptor card.
 *
 * These rules are the whole point of the schema: the client renders a form it
 * has never seen, and when a card is missing or half-published it must show
 * nothing rather than ask a human for the wrong secret.
 */

import { expect, test } from "bun:test";
import { capabilityCardsOf, cardOf, labelOf, type Card, type DescribedObject } from "../src/lib/card-shape";

const described = (descriptor: DescribedObject["descriptor"], id = "obj"): DescribedObject => ({
	id,
	typeKey: "descriptor",
	fields: {},
	blocks: [],
	deleted: false,
	createdAt: 0,
	updatedAt: 0,
	descriptor,
});

test("a card becomes a form: labels, and which inputs are secret", () => {
	const card = cardOf(
		described({
			key: "x",
			name: "X (Twitter)",
			description: "Post and read as an account you own.",
			kind: "integration",
			fields: [
				{ key: "apiKey", label: "API key", secret: false, format: "text", note: "" },
				{ key: "apiSecret", label: "API secret", secret: true, format: "password", note: "Never leaves this machine." },
			],
			auths: ["browser_profile", "api_key"],
			install: { prompt: "", uninstallPrompt: "", docsUrl: "https://x.com/login" },
			version: "1",
			author: "npub1fcpps",
		}),
	) as Card;
	expect(card.fields.map((f) => f.label)).toEqual(["API key", "API secret"]);
	expect(card.fields.filter((f) => f.secret).map((f) => f.key)).toEqual(["apiSecret"]);
	expect(card.install?.docsUrl).toBe("https://x.com/login");
	// Provenance travels with the card: publishing is open, so a client that
	// asks for a credential must be able to say who asked.
	expect(card.author).toBe("npub1fcpps");
});

test("an object without a usable card yields nothing", () => {
	// No descriptor at all, and a half-published one (no key) - a form built
	// from either would ask for the wrong thing.
	expect(cardOf(described(undefined))).toBeUndefined();
	expect(cardOf(described({ name: "Nameless" }))).toBeUndefined();
});

test("missing optional parts degrade instead of throwing", () => {
	const bare = cardOf(described({ key: "browserless" })) as Card;
	// A key with no name renders as the key, never as blank.
	expect(bare.name).toBe("browserless");
	expect(bare.fields).toEqual([]);
	expect(bare.auths).toEqual([]);
	expect(bare.check).toBeUndefined();
	expect(bare.kind).toBe("");
});

test("only skills and logins are things a machine can hold", () => {
	const cards = [
		cardOf(described({ key: "browserless", kind: "skill" }, "a")),
		cardOf(described({ key: "x", kind: "integration" }, "b")),
		cardOf(described({ key: "researcher", kind: "agent" }, "c")),
		cardOf(described({ key: "future", kind: "quantum_thing" }, "d")),
	].filter((c): c is Card => c !== undefined);
	expect(capabilityCardsOf(cards).map((c) => c.key)).toEqual(["browserless", "x"]);
});

test("an undescribed capability still renders, as its key", () => {
	const cards = [cardOf(described({ key: "browserless", name: "Headless Chrome", kind: "skill" }))].filter(
		(c): c is Card => c !== undefined,
	);
	expect(labelOf(cards, "browserless")).toBe("Headless Chrome");
	// A key published by a newer harness than this client knows about.
	expect(labelOf(cards, "telepathy")).toBe("telepathy");
});
