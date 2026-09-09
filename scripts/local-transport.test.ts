import { afterEach, describe, expect, test, spyOn } from "bun:test";
import { LocalBackend } from "../src/lib/local-backend";
import { harnessFetch, localFetch, onPairingChange, pairedSession, pairLocal, PairingError, streamLocalEvents, unpairLocal } from "../src/lib/local-transport";

const TOKEN = "ab".repeat(32);
const SESSION_KEY = "roostr-local-pairing";
const cleanup: Array<() => void> = [];
afterEach(() => { for (const close of cleanup.splice(0).reverse()) close(); });

class MemoryStorage implements Storage {
	private values = new Map<string, string>();
	get length(): number { return this.values.size; }
	clear(): void { this.values.clear(); }
	getItem(key: string): string | null { return this.values.get(key) ?? null; }
	key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
	removeItem(key: string): void { this.values.delete(key); }
	setItem(key: string, value: string): void { this.values.set(key, String(value)); }
}

function fixture(handler: (url: string, init?: RequestInit) => Response | Promise<Response> = () => Response.json({ ok: true })) {
	const descriptors = {
		sessionStorage: Object.getOwnPropertyDescriptor(globalThis, "sessionStorage"),
		localStorage: Object.getOwnPropertyDescriptor(globalThis, "localStorage"),
	};
	const session = new MemoryStorage(), local = new MemoryStorage();
	Object.defineProperty(globalThis, "sessionStorage", { configurable: true, value: session });
	Object.defineProperty(globalThis, "localStorage", { configurable: true, value: local });
	const requests: Array<{ url: string; init?: RequestInit }> = [];
	const fetchMock = spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
		const url = input instanceof Request ? input.url : String(input);
		requests.push({ url, init });
		return handler(url, init);
	});
	cleanup.push(() => {
		unpairLocal();
		fetchMock.mockRestore();
		for (const key of ["sessionStorage", "localStorage"] as const) {
			const descriptor = descriptors[key];
			if (descriptor) Object.defineProperty(globalThis, key, descriptor);
			else Reflect.deleteProperty(globalThis, key);
		}
	});
	function seedPairing(expiresAt = Date.now() + 60_000) {
		const value = { token: TOKEN, expiresAt, role: "ui" as const };
		session.setItem(SESSION_KEY, JSON.stringify(value));
		return value;
	}
	return { session, local, requests, seedPairing };
}

function splitStream(text: string): ReadableStream<Uint8Array> {
	const bytes = new TextEncoder().encode(text);
	return new ReadableStream({
		start(controller) {
			// Single-byte chunks split field names, CRLF, JSON and multibyte UTF-8.
			for (let i = 0; i < bytes.length; i++) controller.enqueue(bytes.slice(i, i + 1));
			controller.close();
		},
	});
}

describe("explicit local pairing", () => {
	for (const expired of [false, true]) {
		test(`${expired ? "expired" : "missing"} pairing blocks both services without a request`, async () => {
			const f = fixture();
			if (expired) f.seedPairing(Date.now() - 1);
			expect(pairedSession()).toBeNull();
			await expect(localFetch("/api/settings")).rejects.toBeInstanceOf(PairingError);
			await expect(harnessFetch("/auth/status")).rejects.toBeInstanceOf(PairingError);
			expect(f.requests).toHaveLength(0);
		});
	}

	test("only an explicit terminal code establishes a session and both services use its bearer", async () => {
		const session = { token: TOKEN, expiresAt: Date.now() + 60_000, role: "ui" };
		const f = fixture((url) => Response.json(url.endsWith("/api/pair") ? session : { ok: true }));
		expect(pairedSession()).toBeNull();
		await pairLocal("  terminal-code  ");
		expect(pairedSession()).toEqual(session);
		expect(f.requests[0].url).toBe("http://127.0.0.1:7333/api/pair");
		expect(f.requests[0].init?.method).toBe("POST");
		expect(JSON.parse(String(f.requests[0].init?.body))).toEqual({ code: "terminal-code" });
		expect(new Headers(f.requests[0].init?.headers).get("Authorization")).toBeNull();
		await localFetch("/api/settings", { credentials: "include", redirect: "follow", headers: { Authorization: "Bearer attacker", "X-Request": "kept" } });
		await harnessFetch("/auth/status", { credentials: "include", redirect: "follow" });
		expect(f.requests.map((r) => r.url)).toEqual([
			"http://127.0.0.1:7333/api/pair", "http://127.0.0.1:7333/api/settings", "http://127.0.0.1:7334/auth/status",
		]);
		for (const { init } of f.requests) {
			expect(init?.credentials).toBe("omit");
			expect(init?.redirect).toBe("error");
		}
		for (const { init } of f.requests.slice(1)) expect(new Headers(init?.headers).get("Authorization")).toBe(`Bearer ${TOKEN}`);
		expect(new Headers(f.requests[1].init?.headers).get("X-Request")).toBe("kept");
	});

	test("empty codes never send a pairing request", async () => {
		const f = fixture();
		await expect(pairLocal(" \t ")).rejects.toBeInstanceOf(PairingError);
		expect(f.requests).toHaveLength(0);
	});

	for (const value of [
		{ token: "not-a-token", expiresAt: Date.now() + 60_000, role: "ui" },
		{ token: TOKEN, expiresAt: 0, role: "ui" },
		{ token: TOKEN, expiresAt: Date.now() + 60_000, role: "operator" },
	]) {
		test(`rejects invalid pairing response ${JSON.stringify(value)}`, async () => {
			fixture(() => Response.json(value));
			await expect(pairLocal("terminal-code")).rejects.toBeInstanceOf(PairingError);
			expect(pairedSession()).toBeNull();
		});
	}

	test("absolute, network-path and backslash origin escapes are rejected before fetch", async () => {
		const f = fixture();
		f.seedPairing();
		for (const path of ["https://evil.example/api/settings", "http://127.0.0.1:7333/api/settings", "//evil.example/api/settings", "/\\evil.example/api/settings", "\\\\evil.example/api/settings", "api/settings"]) {
			await expect(localFetch(path)).rejects.toThrow();
			await expect(harnessFetch(path)).rejects.toThrow();
		}
		expect(f.requests).toHaveLength(0);
	});

	for (const request of [localFetch, harnessFetch]) {
		test(`${request.name} invalidates a rejected session and notifies listeners`, async () => {
			const f = fixture(() => new Response("Unauthorized", { status: 401 }));
			f.seedPairing();
			let changes = 0;
			cleanup.push(onPairingChange(() => { changes++; }));
			await expect(request("/status")).rejects.toBeInstanceOf(PairingError);
			expect(pairedSession()).toBeNull();
			expect(f.session.getItem(SESSION_KEY)).toBeNull();
			expect(changes).toBe(1);
			await expect(request("/status")).rejects.toBeInstanceOf(PairingError);
			expect(f.requests).toHaveLength(1);
		});
	}

	for (const request of [localFetch, harnessFetch]) {
		for (const status of [200, 401]) {
			test(`${request.name} discards stale ${status} responses without clearing a replacement pairing`, async () => {
				const deferred = Promise.withResolvers<Response>();
				const replacement = { token: "cd".repeat(32), expiresAt: Date.now() + 60_000, role: "ui" };
				const f = fixture((url) => url.endsWith("/api/pair") ? Response.json(replacement) : deferred.promise);
				f.seedPairing();
				const pending = request("/status");
				unpairLocal();
				await pairLocal("replacement-code");
				let cancelled = false;
				const body = new ReadableStream<Uint8Array>({ cancel() { cancelled = true; } });
				deferred.resolve(new Response(body, { status }));
				await expect(pending).rejects.toBeInstanceOf(PairingError);
				expect(cancelled).toBe(true);
				expect(pairedSession()).toEqual(replacement);
				expect(new Headers(f.requests[0].init?.headers).get("Authorization")).toBe(`Bearer ${TOKEN}`);
			});
		}
	}
});

describe("authenticated native event stream", () => {
	test("parses split UTF-8/CRLF and multiline data while sending bearer authorization", async () => {
		const f = fixture(() => new Response(splitStream(': heartbeat\r\nevent: changed\r\ndata: {"objectId":\r\ndata: "doc-é"}\r\n\r\n'), { headers: { "Content-Type": "text/event-stream" } }));
		f.seedPairing();
		const signal = new AbortController().signal;
		const events: Array<{ event: string; objectId: string }> = [];
		await expect(streamLocalEvents("/api/events", signal, (event, data) => {
			const parsed: unknown = JSON.parse(data);
			if (!parsed || typeof parsed !== "object" || !("objectId" in parsed) || typeof parsed.objectId !== "string") throw new Error("Expected an object ID event");
			events.push({ event, objectId: parsed.objectId });
		})).rejects.toThrow("Local event stream disconnected.");
		expect(events).toEqual([{ event: "changed", objectId: "doc-é" }]);
		expect(f.requests[0].url).toBe("http://127.0.0.1:7333/api/events");
		expect(f.requests[0].init?.signal).toBe(signal);
		const headers = new Headers(f.requests[0].init?.headers);
		expect(headers.get("Authorization")).toBe(`Bearer ${TOKEN}`);
		expect(headers.get("Accept")).toBe("text/event-stream");
	});

	test("LocalBackend publishes parsed object IDs, not serialized event bodies", async () => {
		const f = fixture((url) => url.endsWith("/api/settings")
			? Response.json({ hasKey: true, authorId: "native-author", npub: "native-npub", relays: [] })
			: new Response(splitStream('data: {"hello":true}\n\ndata: {"objectId":"doc-é"}\n\n')));
		f.seedPairing();
		const backend = new LocalBackend();
		cleanup.push(() => backend.stop());
		const received: string[][] = [];
		const committed = Promise.withResolvers<void>();
		cleanup.push(backend.onCommit((ids) => {
			received.push(ids);
			if (ids.length) { backend.stop(); committed.resolve(); }
		}));
		await backend.start();
		await committed.promise;
		expect(received).toEqual([[], ["doc-é"]]);
		expect(backend.status.phase).toBe("live");
		expect(backend.status.bootstrapped).toBe(false);
		expect(new Headers(f.requests[1].init?.headers).get("Authorization")).toBe(`Bearer ${TOKEN}`);
	});
});

describe("native mutation and logout boundaries", () => {
	test("mutations use the real action wire shape and surface daemon failures", async () => {
		let fail = false;
		const f = fixture(() => Response.json(fail ? { ok: false, error: "writer permission required" } : { ok: true, objectId: "doc" }));
		f.seedPairing();
		const backend = new LocalBackend();
		expect(await backend.mutate("field_set", { objectId: "doc", key: "name", value: "Title", action: "must-not-override" })).toEqual({ ok: true, objectId: "doc" });
		expect(f.requests[0].url).toBe("http://127.0.0.1:7333/api/mutate");
		expect(f.requests[0].init?.method).toBe("POST");
		expect(new Headers(f.requests[0].init?.headers).get("Content-Type")).toBe("application/json");
		expect(JSON.parse(String(f.requests[0].init?.body))).toEqual({ objectId: "doc", key: "name", value: "Title", action: "field_set" });
		fail = true;
		await expect(backend.mutate("field_set", { objectId: "doc" })).rejects.toThrow("writer permission required");
	});

	test("logout only removes pairing, never browser key/vault storage or remote native data", async () => {
		const f = fixture();
		f.seedPairing();
		f.session.setItem("unrelated-session", "keep");
		f.local.setItem("roostr-key", "browser-secret");
		f.local.setItem("browser-vault", "unpublished-change");
		const backend = new LocalBackend();
		backend.author = "native-author";
		await backend.logout();
		expect(pairedSession()).toBeNull();
		expect(backend.author).toBe("");
		expect(f.session.getItem("unrelated-session")).toBe("keep");
		expect(f.local.length).toBe(2);
		expect(f.local.getItem("roostr-key")).toBe("browser-secret");
		expect(f.local.getItem("browser-vault")).toBe("unpublished-change");
		expect(f.requests).toHaveLength(0);
	});
});
