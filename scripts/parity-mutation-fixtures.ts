import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { coreCall, initCore } from "../src/lib/engine/core";
import { unpackCoreValueMaps } from "../src/lib/engine/core-values";
import { encodeChange, decodeChange } from "../src/lib/engine/proto";
import type { ChangeJSON } from "../src/lib/engine/contracts";

interface MutationFixture {
	name: string;
	payload: unknown;
	error?: boolean;
	expected_changes: unknown[];
	expected_result: unknown;
	expected_vanish_ids: string[];
	expected_vanish_changes: unknown[];
}
interface MutationResult {
	changes: ChangeJSON[];
	result: unknown;
	vanish_ids: string[];
	vanish_changes: ChangeJSON[];
}

await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
const fixtureUrl = new URL("../../glonOdin/core/mutation_fixtures.json", import.meta.url);
const fixtures = JSON.parse(readFileSync(fixtureUrl, "utf8")) as MutationFixture[];
for (const fixture of fixtures) {
	if (fixture.error) {
		assert.throws(() => coreCall("mutation", fixture.payload), undefined, fixture.name);
		continue;
	}
	const result = unpackCoreValueMaps<MutationResult>(coreCall("mutation", fixture.payload));
	assert.deepStrictEqual(result.result, fixture.expected_result, `${fixture.name}: result`);
	assert.deepStrictEqual(result.vanish_ids, fixture.expected_vanish_ids, `${fixture.name}: vanish ids`);
	for (const key of ["changes", "vanish_changes"] as const) {
		const expected = unpackCoreValueMaps<ChangeJSON[]>(fixture[`expected_${key}`]);
		assert.equal(result[key].length, expected.length, `${fixture.name}: ${key} count`);
		for (let index = 0; index < expected.length; index++) {
			// Encoding compares the full operation model including ordered maps;
			// differing omitted JSON defaults are the same protobuf contract.
			const expectedBytes = encodeChange(expected[index]);
			const actualBytes = encodeChange(result[key][index]);
			assert.deepStrictEqual(actualBytes, expectedBytes, `${fixture.name}: ${key}[${index}]`);
			assert.ok(decodeChange(actualBytes), `${fixture.name}: encoded change must decode`);
		}
	}
}
console.log(`PASS: ${fixtures.length} shared mutation fixtures`);
