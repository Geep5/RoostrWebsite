import { beforeAll, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { CoreError, coreCall, coreGeneration, initCore, resetCore } from "../src/lib/engine/core";

beforeAll(async () => {
	await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });
});

describe("shared Odin ABI lifecycle", () => {
	test("reports domain errors and remains usable after them", () => {
		expect(() => coreCall("not-a-method", {})).toThrow("unknown core method");
		expect(coreCall("replay", { changes: [] })).toBeNull();
	});

	test("bounds request bytes and recursive parser depth", () => {
		expect(() => coreCall("replay", { data: "x".repeat(16 * 1024 * 1024) })).toThrow("Core request exceeds 16 MiB");
		let nested: unknown = {};
		for (let index = 0; index < 130; index++) nested = { next: nested };
		expect(() => coreCall("replay", nested)).toThrow("nesting exceeds 128");
		expect(coreCall("replay", { changes: [] })).toBeNull();
	});

	test("rejects reentrant serialization without corrupting the next call", () => {
		const payload = { toJSON() { return coreCall("replay", { changes: [] }); } };
		try {
			coreCall("replay", payload);
			throw new Error("Reentrant request unexpectedly succeeded");
		} catch (error) {
			expect(error).toBeInstanceOf(CoreError);
			expect((error as CoreError).code).toBe("reentrant");
		}
		expect(coreCall("replay", { changes: [] })).toBeNull();
	});

	test("copies results before arena reuse and invalidates query generations", () => {
		const body = { objects: [], body: {}, nowMs: 1704067200000 };
		const first = coreCall("query", body);
		for (let index = 0; index < 128; index++) coreCall("query", body);
		expect(first).toEqual({ total: 0, records: [] });
		const previous = coreGeneration();
		resetCore();
		expect(coreGeneration()).toBe(previous + 1);
		expect(coreCall("query", body)).toEqual(first);
	});
});
