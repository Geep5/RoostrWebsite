import { deepStrictEqual, throws } from "node:assert/strict";
import { readFileSync } from "node:fs";
import type { ChangeJSON } from "../src/lib/engine/contracts";
import type { ObjectJSON } from "../src/lib/types";
import { coreCall, initCore } from "../src/lib/engine/core";
import { computeObject } from "../src/lib/engine/replay";

interface ReplayFixture {
	name: string;
	changes: ChangeJSON[];
	expected: ObjectJSON | null;
}

// The native test embeds this exact corpus; neither side derives expected
// state from its current replay implementation.
const fixtures: ReplayFixture[] = JSON.parse(readFileSync(
	new URL("../../glonOdin/core/replay_fixtures.json", import.meta.url), "utf8",
));
await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

for (const fixture of fixtures) {
	const input = JSON.stringify(fixture.changes);
	deepStrictEqual(computeObject(fixture.changes), fixture.expected, fixture.name);
	deepStrictEqual(computeObject([...fixture.changes].reverse()), fixture.expected, `${fixture.name}: reversed`);
	deepStrictEqual(computeObject(fixture.changes), fixture.expected, `${fixture.name}: repeated`);
	deepStrictEqual(JSON.stringify(fixture.changes), input, `${fixture.name}: input unchanged`);
}

for (const payload of [null, {}, { changes: {} }, { changes: [null] }]) {
	throws(() => coreCall("replay", payload), undefined, "invalid replay payload must report an error");
}
console.log(`Replay parity: ${fixtures.length} shared legacy fixtures, reversed and repeated; invalid payloads rejected`);
