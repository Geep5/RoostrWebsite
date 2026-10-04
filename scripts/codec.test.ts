/**
 * The host side of the codec contract: a browser names a thread's
 * participants WITHOUT its own protobuf reader. The knowledge arrives as
 * bytes and the shared core decodes it.
 */

import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import { coreCall, initCore } from "../src/lib/engine/core";

await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

test("a client names a thread's participants from its root bytes", () => {
	const wire = coreCall<string>("codec", {
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
		"codec",
		{ action: "decode", type: "conversation", bytes: wire },
	);
	expect(c.kind).toBe("a2a");
	expect(c.title).toBe("Pricing research");
	expect(c.participants).toEqual(["agent-scout", "agent-analyst"]);
	expect(c.aboutMessageId).toBe("c8609781");
	expect(c.closed).toBe(false);
});

test("bad input is refused, not guessed at", () => {
	expect(() => coreCall("codec", { action: "decode", type: "conversation", bytes: "!!!not base64" })).toThrow();
	expect(() => coreCall("codec", { action: "decode", type: "descriptor", bytes: "" })).toThrow();
	expect(() => coreCall("codec", { action: "decode", type: "installation", bytes: "" })).toThrow();
	expect(() => coreCall("codec", { action: "frobnicate", type: "conversation" })).toThrow();
});
