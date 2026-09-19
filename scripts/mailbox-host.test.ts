import { expect, test } from "bun:test";

// Isolate the real backend singleton and IndexedDB from other browser tests.
// This catches overlapping read/plan/commit calls, not just core planner races.
test("concurrent browser deliveries and claims execute a mailbox message once", async () => {
	const program = `
		import assert from "node:assert/strict";
		import { readFileSync } from "node:fs";
		import { indexedDB } from "fake-indexeddb";
		import { initCore } from "./src/lib/engine/core";
		import { backend } from "./src/lib/engine/backend";
		import { ChangeStore } from "./src/lib/engine/store";
		globalThis.indexedDB = indexedDB;
		await initCore({ wasmBytes: readFileSync("static/engine.wasm") });
		backend.store = new ChangeStore("mailbox-concurrency");
		backend.author = "mailbox-regression";
		await backend.store.open();
		const sender = (await backend.mutate("create", { name: "Sender", type_key: "note" })).id;
		const receiver = (await backend.mutate("create", { name: "Receiver", type_key: "note" })).id;
		const message = {
			id: "concurrent-message", exchangeId: "concurrent-exchange",
			sender: { objectId: sender, agentId: "" }, recipients: [{ objectId: receiver, agentId: "" }],
			text: "Record this once", replyTo: "", sentAt: 100, title: "Concurrent delivery",
			requestReply: false, historical: false, operation: "", author: "",
		};
		await backend.mutate("message_send", { object_id: sender, message });
		await Promise.all(Array.from({ length: 8 }, () => backend.mutate("message_deliver", {
			sender_object_id: sender, message_id: message.id, recipient_object_id: receiver,
		})));
		assert.deepEqual((await backend.fetchObject(receiver)).mailbox.map(row => row.message.text), [message.text]);
		const claims = await Promise.all(Array.from({ length: 12 }, (_, index) => backend.mutate("message_processing", {
			object_id: receiver, message_id: message.id, status: "processing", owner: "worker-" + index,
		})));
		assert.equal(claims.filter(result => result.claimed).length, 1);
		const received = await backend.fetchObject(receiver);
		const owner = received.mailbox[0].processing.owner;
		await backend.mutate("message_processing", { object_id: receiver, message_id: message.id, status: "processed", owner });
		const repeated = await backend.mutate("message_processing", { object_id: receiver, message_id: message.id, status: "processing", owner: "late-worker" });
		assert.equal(repeated.claimed, false);
		backend.store.close();
		console.log("one receiver copy; one successful claim; processed message never reclaimed");
	`;
	const proc = Bun.spawn([process.execPath, "-e", program], { cwd: new URL("..", import.meta.url).pathname, stdout: "pipe", stderr: "pipe" });
	const [out, err, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
	expect(code, err || out).toBe(0);
}, 20_000);
