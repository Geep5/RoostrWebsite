import { deepStrictEqual, equal, throws } from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CoreError, coreCall, initCore } from "../src/lib/engine/core";
import type { ChangeJSON, SharedProvenance } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";

interface AuthorizeFixture {
	name: string;
	change: ChangeJSON;
	provenance: SharedProvenance;
	space: { spaceId: string; keyId: number; owner: string };
	trustedSpace: ObjectJSON | null;
	existing: ObjectJSON | null;
	ok?: boolean;
	reason?: string;
	/** Envelope-level rejection: the call must throw a domain CoreError with this message. */
	error?: string;
}
interface VanishedFixture {
	name: string;
	ledger: ObjectJSON | null;
	expected: Array<{ objectId: string; at: number }>;
}
interface AuthorityFixtures {
	authorize: AuthorizeFixture[];
	vanished: VanishedFixture[];
}

// Native authority_test.odin embeds this same hand-authored corpus, so every
// replica's gate is checked against one set of expected outcomes.
const fixtures: AuthorityFixtures = JSON.parse(readFileSync(
	new URL("../../Roostr/core/authority_fixtures.json", import.meta.url), "utf8",
));
await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

for (const { name, change, provenance, space, trustedSpace, existing, ok, reason, error } of fixtures.authorize) {
	const payload = { action: "authorize", change, provenance, space, trustedSpace, existing };
	if (error !== undefined) {
		throws(() => coreCall("sync", payload),
			(e: unknown) => e instanceof CoreError && e.code === "domain" && e.message === error, `authorize ${name}: domain error "${error}"`);
		continue;
	}
	deepStrictEqual(coreCall<{ ok: boolean; reason: string }>("sync", payload), { ok, reason }, `authorize ${name}`);
}

for (const { name, ledger, expected } of fixtures.vanished) {
	const actual = coreCall<Array<{ objectId: string; at: number }>>("sync", { action: "vanished", ledger });
	equal(actual.length, expected.length, `vanished ${name}: entry count`);
	const sorted = (xs: Array<{ objectId: string; at: number }>) => [...xs].sort((a, b) => a.objectId.localeCompare(b.objectId));
	deepStrictEqual(sorted(actual), sorted(expected), `vanished ${name}`);
}

console.log(`Authority parity: ${fixtures.authorize.length} authorize and ${fixtures.vanished.length} vanished fixed native/WASM goldens`);
