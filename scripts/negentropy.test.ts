import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { Negentropy, NegentropyStorage } from "../src/lib/engine/negentropy";

// Golden values produced by hoytech's reference (github.com/hoytech/negentropy js/Negentropy.js)
// over the same inputs; the vendored codec must match it byte for byte.
const idOf = (i: number) => createHash("sha256").update(`roostr-neg-${i}`).digest("hex");
const sha = (s: string) => createHash("sha256").update(s).digest("hex");
// Clustered timestamps: eight items per second, so bounds need id prefixes.
const ts = (i: number) => 1_700_000_000 + (i >> 3);

function storage(items: number[]): NegentropyStorage {
	const s = new NegentropyStorage();
	for (const i of items) s.insert(ts(i), idOf(i));
	s.seal();
	return s;
}

/** Drive an initiator against a responder; returns the wire trace and the initiator's have/need. */
function run(client: Negentropy, server: Negentropy) {
	const trace: string[] = [], have: string[] = [], need: string[] = [];
	let msg: string | null = client.initiate();
	while (msg !== null) {
		trace.push(msg);
		const { next: resp } = server.reconcile(msg);
		trace.push(resp!);
		const r = client.reconcile(resp!);
		have.push(...r.haveIds);
		need.push(...r.needIds);
		msg = r.next;
	}
	return { trace, have, need };
}

describe("negentropy V1", () => {
	test("fingerprints and initial messages match the reference", () => {
		expect(new Negentropy(storage([])).initiate()).toBe("6100000200");
		const empty = storage([]);
		expect(Buffer.from(empty.fingerprint(0, 0)).toString("hex")).toBe("7f9c9e31ac8256ca2f258583df262dbc");
		const small = new NegentropyStorage();
		for (const i of [0, 1, 2]) small.insert(ts(i * 9), idOf(i));
		small.seal();
		expect(Buffer.from(small.fingerprint(0, 3)).toString("hex")).toBe("3dfcd3fed9b374242a7469a5721fbf07");
		expect(new Negentropy(small).initiate()).toBe(
			"610000020386731aa29bf41d347d430bcdcedb105385cfedb662d460726784000877cf97de415adbd8b560618864dcd842bf40739b9923e235c561492ed97c281b0c9e9db6513fdf2cc148bea5b526b937c5d568e347ed810e78e0a4ae6601fa9e65c87d9a",
		);
	});

	test("a framed multi-round reconciliation is byte-identical to the reference", () => {
		const client: number[] = [], server: number[] = [];
		for (let i = 0; i < 2100; i++) {
			if (i < 2000 && i % 7 !== 0) client.push(i);
			if (i % 11 !== 0 || i >= 2000) server.push(i);
		}
		const { trace, have, need } = run(new Negentropy(storage(client), 4096), new Negentropy(storage(server), 4096));
		expect(trace.length / 2).toBe(22);
		expect(trace[0].slice(0, 40)).toBe("6186aacfe21001cb012672eecd18cf0ee328b906");
		expect(sha(trace.join("\n"))).toBe("d38c183f8340dc069acc7da539d86c8eecdc00e65a0a2650a1d06830199dc0d5");
		expect(need.length).toBe(360);
		expect(sha(need.join(","))).toBe("eb4fefb07bbe99808bdef3b95c64068eb90625566c67bfbec61e962d4ff594bb");
		expect(have.length).toBe(156);
		expect(sha(have.join(","))).toBe("4c0b7417418b516005f035eaf8ef6c3b91455fdb354dfa927aadff139c5c31b3");
		// Every frame stays under the limit.
		expect(trace.every((m) => m.length / 2 <= 4096)).toBe(true);
	});

	test("need and have are exactly the set differences, empty client to full server", () => {
		const server = Array.from({ length: 5000 }, (_, i) => i);
		const { need, have } = run(new Negentropy(storage([]), 250_000), new Negentropy(storage(server), 250_000));
		expect(have).toEqual([]);
		expect(new Set(need)).toEqual(new Set(server.map(idOf)));
		const same = run(new Negentropy(storage(server), 250_000), new Negentropy(storage(server), 250_000));
		expect(same.trace.length).toBe(2);
		expect(same.need).toEqual([]);
	});

	test("rejects malformed input", () => {
		const s = new NegentropyStorage();
		s.insert(1, idOf(1));
		expect(() => s.insert(1, idOf(1))).not.toThrow();
		expect(() => s.seal()).toThrow("duplicate");
		const neg = new Negentropy(storage([1]));
		neg.initiate();
		expect(() => neg.reconcile("62")).toThrow("unsupported protocol version");
		expect(() => neg.reconcile("61010101")).toThrow("prematurely");
		expect(() => new Negentropy(storage([]), 100)).toThrow("frameSizeLimit");
	});
});
