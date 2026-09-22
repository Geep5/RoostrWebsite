import { deepStrictEqual, equal, throws } from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CoreError, coreCall, initCore } from "../src/lib/engine/core";

interface WireFixtures {
	blind: Array<{ name: string; id: string; expected: string; secret?: string; keyHex?: string }>;
	conversationKeys: Array<{ sharedX: string; expected: string }>;
	seal: Array<{
		name: string; change: string; conversationKey: string; nonce: string; objectId: string;
		secret?: string; space?: { keyHex: string; spaceId: string };
		expected: { gid: string; parts: Array<{ content: string; tags: string[][] }> };
	}>;
	open: Array<{ name: string; content: string; conversationKey: string; expected?: string; error?: true }>;
	verify: Array<{ name: string; bytes: string; gid?: string; expectedId?: string | null; error?: true }>;
}

// Produced by the TypeScript reference (nostr-tools NIP-44 plus the wire code
// that used to live in sync.ts); the core must reproduce every byte. Native
// wire_test.odin pins the same file.
const fixtures: WireFixtures = JSON.parse(readFileSync(
	new URL("../../Roostr/core/wire_fixtures.json", import.meta.url), "utf8",
));
await initCore({ wasmBytes: readFileSync(new URL("../static/engine.wasm", import.meta.url)) });

for (const { name, expected, ...args } of fixtures.blind) {
	equal(coreCall("wire", { action: "blind", ...args }), expected, `blind ${name}`);
}
for (const { sharedX, expected } of fixtures.conversationKeys) {
	equal(coreCall("wire", { action: "conversation_key", sharedX }), expected, `conversation key ${sharedX.slice(0, 8)}`);
}
for (const { name, expected, ...args } of fixtures.seal) {
	const sealed = coreCall<typeof expected>("wire", { action: "seal", ...args });
	deepStrictEqual(sealed, expected, `seal ${name}`);
	for (const [index, part] of sealed.parts.entries()) {
		equal(coreCall("wire", { action: "open", content: part.content, conversationKey: args.conversationKey }),
			args.change.slice(index * 40_000, (index + 1) * 40_000), `seal ${name}: part ${index} opens to its chunk`);
	}
}
for (const { name, content, conversationKey, expected, error } of fixtures.open) {
	if (error) throws(() => coreCall("wire", { action: "open", content, conversationKey }), (e: unknown) => e instanceof CoreError && e.code === "domain", `open ${name}: domain error`);
	else equal(coreCall("wire", { action: "open", content, conversationKey }), expected, `open ${name}`);
}
for (const { name, bytes, gid, expectedId, error } of fixtures.verify) {
	if (error) {
		throws(() => coreCall("wire", { action: "verify", bytes, gid }), (e: unknown) => e instanceof CoreError && e.code === "domain", `verify ${name}: domain error`);
		continue;
	}
	const verified = coreCall<{ id: string; change: { id: string } }>("wire", { action: "verify", bytes, gid });
	equal(verified.id, expectedId, `verify ${name}: raw content address`);
	equal(verified.change.id, expectedId, `verify ${name}: decoded id`);
}
console.log(`Wire parity: ${fixtures.blind.length} blind, ${fixtures.conversationKeys.length} conversation-key, ${fixtures.seal.length} seal, ${fixtures.open.length} open, ${fixtures.verify.length} verify fixtures match the TS reference`);
