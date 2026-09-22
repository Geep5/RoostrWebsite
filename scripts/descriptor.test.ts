/**
 * The host side of the descriptor contract: a browser renders a setup form,
 * and names a thread's participants, WITHOUT its own protobuf reader. The
 * knowledge arrives as bytes and the shared core decodes it.
 *
 * This is the test that would have caught the drift the codebase already
 * admits to: `src/lib/serving.ts` still carries a hand-copy of the harness's
 * capability list, kept in step by hand.
 */

import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { coreCall, initCore } from "../src/lib/engine/core";

await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

type Descriptor = {
	key: string;
	name: string;
	kind: string;
	fields: Array<{ key: string; label: string; secret: boolean; format: string; note: string }>;
	auths: string[];
	check?: { command: string; expectContains: string; timeoutMs: number };
	agent?: { system: string; model: string; requires: string[]; skills: string[]; responsibleTypes: string[] };
	unknown?: string;
};

test("a client renders a login form it has never seen", () => {
	// Published by whoever knows how X works; this host knows nothing about it.
	const wire = coreCall<string>("descriptor", {
		action: "encode",
		type: "descriptor",
		value: {
			key: "x",
			name: "X (Twitter)",
			description: "Post and read as an account you own.",
			kind: "integration",
			fields: [
				{ key: "apiKey", label: "API key", secret: false, format: "text", note: "" },
				{ key: "apiSecret", label: "API secret", secret: true, format: "password", note: "Never leaves this machine." },
			],
			auths: ["browser_profile", "api_key"],
			check: { command: "x-retweet --whoami", expectContains: "@", timeoutMs: 15000 },
			version: "1",
			author: "npub1fcppsmdf84swh33vqwklscppskw5j8tcu280n27ejlz53lvl5xcqxj0vl2",
		},
	});
	expect(typeof wire).toBe("string");

	const d = coreCall<Descriptor>("descriptor", { action: "decode", type: "descriptor", bytes: wire });
	expect(d.name).toBe("X (Twitter)");
	expect(d.kind).toBe("integration");
	// The form IS the field list: labels, and which inputs are passwords.
	expect(d.fields.map((f) => f.label)).toEqual(["API key", "API secret"]);
	expect(d.fields.filter((f) => f.secret).map((f) => f.key)).toEqual(["apiSecret"]);
	expect(d.fields[1].format).toBe("password");
	expect(d.auths).toEqual(["browser_profile", "api_key"]);
	// No secret VALUE exists anywhere in the schema to leak.
	expect(JSON.stringify(d)).not.toContain("apiSecretValue");
});

test("a descriptor from a newer writer survives this host", () => {
	// Field 12 (length-delimited) does not exist in this build: tag 0x62,
	// length 8, payload "unknown!" = Ygh1bmtub3duIQ==. Decode, hand it back, and the
	// bytes must be identical - otherwise every old client is data loss.
	const original = coreCall<string>("descriptor", {
		action: "encode",
		type: "descriptor",
		value: { key: "future", name: "Next year", kind: "skill", fields: [], auths: [], unknown: "Ygh1bmtub3duIQ==" },
	});
	const decoded = coreCall<Descriptor>("descriptor", { action: "decode", type: "descriptor", bytes: original });
	expect(decoded.unknown).toBe("Ygh1bmtub3duIQ==");
	const again = coreCall<string>("descriptor", { action: "encode", type: "descriptor", value: decoded });
	expect(again).toBe(original);
});

test("an agent card says what the kind is", () => {
	// A kind picker copies `requires` onto the agent it mints; every list has
	// to come back intact, or the agent lands on a machine that cannot run it.
	const agent = { system: "You are Marco.", model: "kimi-k2-0905-preview", requires: ["matcherino-dev", "discord-bot"], skills: [], responsibleTypes: ["task"] };
	const wire = coreCall<string>("descriptor", {
		action: "encode",
		type: "descriptor",
		value: { key: "marco", name: "Marco", kind: "agent", fields: [], auths: [], agent },
	});
	const d = coreCall<Descriptor>("descriptor", { action: "decode", type: "descriptor", bytes: wire });
	expect(d.kind).toBe("agent");
	expect(d.agent).toEqual(agent);
	expect(coreCall<string>("descriptor", { action: "encode", type: "descriptor", value: d })).toBe(wire);
	// The key's presence is how a client tells an agent card from the rest.
	const plain = coreCall<Descriptor>("descriptor", {
		action: "decode",
		type: "descriptor",
		bytes: coreCall<string>("descriptor", { action: "encode", type: "descriptor", value: { key: "browserless", name: "", kind: "skill", fields: [], auths: [] } }),
	});
	expect(plain.agent).toBeUndefined();
});

test("a client names a thread's participants from its root bytes", () => {
	const wire = coreCall<string>("descriptor", {
		action: "encode",
		type: "conversation",
		value: {
			id: "__thread__50706675",
			kind: "a2a",
			title: "Pricing research",
			participants: ["agent-scout", "agent-analyst"],
			createdAt: 1789700000000,
			openedBy: "device-a",
			aboutMessageId: "c8609781",
			closed: false,
		},
	});
	const c = coreCall<{ kind: string; title: string; participants: string[]; closed: boolean; aboutMessageId: string }>(
		"descriptor",
		{ action: "decode", type: "conversation", bytes: wire },
	);
	expect(c.kind).toBe("a2a");
	expect(c.title).toBe("Pricing research");
	expect(c.participants).toEqual(["agent-scout", "agent-analyst"]);
	expect(c.aboutMessageId).toBe("c8609781");
	expect(c.closed).toBe(false);
});

test("an installation carries the error a view can sort on", () => {
	const wire = coreCall<string>("descriptor", {
		action: "encode",
		type: "installation",
		value: {
			key: "x",
			machineId: "820d3a06-1eee-4b68-b997-12eb7e44b7fe",
			status: "needs_auth",
			auth: "browser_profile",
			checkedAt: 1789603984879,
			error: 'credential "x" has no logged-in browser profile',
		},
	});
	const i = coreCall<{ status: string; error: string; machineId: string; checkedAt: number }>("descriptor", {
		action: "decode",
		type: "installation",
		bytes: wire,
	});
	expect(i.status).toBe("needs_auth");
	expect(i.error).toBe('credential "x" has no logged-in browser profile');
	expect(i.checkedAt).toBe(1789603984879);
});

test("bad input is refused, not guessed at", () => {
	expect(() => coreCall("descriptor", { action: "decode", type: "descriptor", bytes: "!!!not base64" })).toThrow();
	expect(() => coreCall("descriptor", { action: "decode", type: "nonsense", bytes: "" })).toThrow();
	expect(() => coreCall("descriptor", { action: "frobnicate", type: "descriptor" })).toThrow();
});
