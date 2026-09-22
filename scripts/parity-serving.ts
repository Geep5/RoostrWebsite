import { deepStrictEqual, throws } from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CoreError, coreCall, initCore } from "../src/lib/engine/core";
import type { ObjectJSON } from "../src/lib/types";

interface ResolveFixture {
	name: string;
	object?: ObjectJSON;
	space?: ObjectJSON;
	machines: unknown[];
	expected?: { machineId: string; reason: string; requires: string[]; candidates: string[] };
	error?: string;
}

// Native serving_test.odin embeds this same corpus: every host resolves the
// serving machine for an object through one engine rule (docs/object-serving.md).
const fixtures: { resolve: ResolveFixture[] } = JSON.parse(readFileSync(
	new URL("../../Roostr/core/serving_fixtures.json", import.meta.url), "utf8",
));
await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

for (const { name, object, space, machines, expected, error } of fixtures.resolve) {
	const payload = { action: "resolve", object: object ?? null, space: space ?? null, machines };
	if (error !== undefined) {
		throws(() => coreCall("serving", payload),
			(e: unknown) => e instanceof CoreError && e.code === "domain" && e.message === error, `resolve ${name}: domain error "${error}"`);
		continue;
	}
	deepStrictEqual(coreCall("serving", payload), expected, `resolve ${name}`);
}

console.log(`Serving parity: ${fixtures.resolve.length} resolve fixed native/WASM goldens`);
