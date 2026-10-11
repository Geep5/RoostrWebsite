/**
 * Files on Blossom (glonOdin/docs/state-sync.md "Files on Blossom"): the
 * framed AES-256-GCM blob every host agrees on, and the BUD-11 upload
 * authorization. Frames are checked against WebCrypto directly, so the
 * format - not just a round trip - is what is pinned.
 */
import { afterEach, describe, expect, test } from "bun:test";
import { finalizeEvent, getPublicKey, verifyEvent } from "nostr-tools";
import {
	BLOB_FRAME_BYTES,
	BLOSSOM_AUTH_KIND,
	DEFAULT_BLOSSOM_SERVER,
	authorizationHeader,
	blossomServer,
	decryptBlob,
	downloadBlob,
	encryptBlob,
	sha256Hex,
	uploadAuthTemplate,
	uploadBlob,
} from "../src/lib/blossom";

const key = new Uint8Array(32).map((_, i) => i + 1);
const sk = new Uint8Array(32).fill(9);

function nonce(i: number): Uint8Array<ArrayBuffer> {
	const n = new Uint8Array(12);
	new DataView(n.buffer).setBigUint64(4, BigInt(i));
	return n;
}
async function openFrame(frame: Uint8Array, i: number): Promise<Uint8Array> {
	const aes = await crypto.subtle.importKey("raw", key, "AES-GCM", false, ["decrypt"]);
	return new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce(i) }, aes, new Uint8Array(frame)));
}
function pattern(size: number): Uint8Array {
	const out = new Uint8Array(size);
	for (let i = 0; i < size; i++) out[i] = (i * 31 + 7) & 0xff;
	return out;
}

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

describe("blob framing", () => {
	test("a zero-length file is one empty frame: the 16-byte tag alone", async () => {
		const blob = await encryptBlob(new Uint8Array(0), key);
		expect(blob.length).toBe(16);
		expect(await openFrame(blob, 0)).toEqual(new Uint8Array(0));
		expect(await decryptBlob(blob, key)).toEqual(new Uint8Array(0));
	});

	test("a small file is one frame under nonce 0", async () => {
		const plain = new TextEncoder().encode("hello, roostr");
		const blob = await encryptBlob(plain, key);
		expect(blob.length).toBe(plain.length + 16);
		expect(await openFrame(blob, 0)).toEqual(plain);
		expect(await decryptBlob(blob, key)).toEqual(plain);
	});

	test("4 MiB frames, each sealed under its big-endian index, the last one shorter", async () => {
		const plain = pattern(2 * BLOB_FRAME_BYTES + 5);
		const blob = await encryptBlob(plain, key);
		const stride = BLOB_FRAME_BYTES + 16;
		expect(blob.length).toBe(plain.length + 3 * 16);
		for (let i = 0; i < 3; i++) {
			const frame = blob.slice(i * stride, (i + 1) * stride);
			expect(await openFrame(frame, i)).toEqual(plain.slice(i * BLOB_FRAME_BYTES, (i + 1) * BLOB_FRAME_BYTES));
		}
		expect(await decryptBlob(blob, key)).toEqual(plain);
	});

	test("an exact multiple of the frame size has no trailing empty frame", async () => {
		const plain = pattern(BLOB_FRAME_BYTES);
		const blob = await encryptBlob(plain, key);
		expect(blob.length).toBe(BLOB_FRAME_BYTES + 16);
		expect(await decryptBlob(blob, key)).toEqual(plain);
	});

	test("tampered, truncated, reordered or wrongly keyed blobs never decrypt", async () => {
		const plain = pattern(BLOB_FRAME_BYTES + 100);
		const blob = await encryptBlob(plain, key);
		const flipped = blob.slice();
		flipped[BLOB_FRAME_BYTES + 20] ^= 1;
		await expect(decryptBlob(flipped, key)).rejects.toThrow("frame 1");
		await expect(decryptBlob(blob.slice(0, blob.length - 1), key)).rejects.toThrow();
		await expect(decryptBlob(blob.slice(0, BLOB_FRAME_BYTES + 16 + 10), key)).rejects.toThrow("truncated");
		const stride = BLOB_FRAME_BYTES + 16;
		const swapped = new Uint8Array([...blob.slice(stride), ...blob.slice(0, stride)]);
		await expect(decryptBlob(swapped, key)).rejects.toThrow();
		await expect(decryptBlob(blob, new Uint8Array(32))).rejects.toThrow("frame 0");
		await expect(decryptBlob(new Uint8Array(5), key)).rejects.toThrow("truncated");
	});

	test("every file gets a different blob under a fresh key", async () => {
		const plain = pattern(64);
		const other = crypto.getRandomValues(new Uint8Array(32));
		expect(await sha256Hex(await encryptBlob(plain, key))).not.toBe(await sha256Hex(await encryptBlob(plain, other)));
		expect(await sha256Hex(new Uint8Array(0))).toBe("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
	});
});

describe("upload authorization (BUD-11)", () => {
	test("kind 24242 with t=upload, x=<sha256>, a ten-minute expiration and the file name", () => {
		const sha = "ab".repeat(32);
		const template = uploadAuthTemplate(sha, "Résumé.pdf", 1_760_000_000);
		expect(template).toEqual({
			kind: BLOSSOM_AUTH_KIND,
			created_at: 1_760_000_000,
			tags: [["t", "upload"], ["x", sha], ["expiration", "1760000600"]],
			content: "Upload Résumé.pdf",
		});
		const signed = finalizeEvent(template, sk);
		expect(verifyEvent(signed)).toBe(true);
		const header = authorizationHeader(signed);
		expect(header.startsWith("Nostr ")).toBe(true);
		// UTF-8 base64 of the event JSON: any script in a name survives.
		const decoded = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(header.slice(6)), (c) => c.charCodeAt(0))));
		expect(decoded).toEqual(JSON.parse(JSON.stringify(signed)));
	});

	test("PUT /upload carries the blob, its hash and the signed authorization; the descriptor must name that hash", async () => {
		const blob = await encryptBlob(pattern(1000), key);
		const sha = await sha256Hex(blob);
		const seen: Array<{ url: string; init: RequestInit }> = [];
		globalThis.fetch = (async (url: string, init: RequestInit) => {
			seen.push({ url, init });
			return Response.json({ url: `https://blobs.test/${sha}`, sha256: sha, size: blob.length, type: "application/octet-stream", uploaded: 1 });
		}) as typeof fetch;
		const templates: unknown[] = [];
		const descriptor = await uploadBlob("https://blobs.test", blob, sha, "photo.png", async (template) => {
			templates.push(template);
			return finalizeEvent(template, sk);
		});
		expect(descriptor.url).toBe(`https://blobs.test/${sha}`);
		expect(seen.length).toBe(1);
		expect(seen[0].url).toBe("https://blobs.test/upload");
		expect(seen[0].init.method).toBe("PUT");
		const headers = seen[0].init.headers as Record<string, string>;
		expect(headers["X-SHA-256"]).toBe(sha);
		const auth = JSON.parse(atob(headers.Authorization.slice(6)));
		expect(auth.kind).toBe(24242);
		expect(auth.pubkey).toBe(getPublicKey(sk));
		expect(auth.tags).toContainEqual(["x", sha]);
		expect(new Uint8Array(seen[0].init.body as Uint8Array)).toEqual(blob);
		expect(templates.length).toBe(1);

		globalThis.fetch = (async () => Response.json({ url: "x", sha256: "cd".repeat(32), size: 1 })) as unknown as typeof fetch;
		await expect(uploadBlob("https://blobs.test", blob, sha, "photo.png", async (t) => finalizeEvent(t, sk))).rejects.toThrow("not the uploaded");
		globalThis.fetch = (async () => new Response("", { status: 401, headers: { "X-Reason": "pubkey not allowed" } })) as unknown as typeof fetch;
		await expect(uploadBlob("https://blobs.test", blob, sha, "photo.png", async (t) => finalizeEvent(t, sk))).rejects.toThrow("pubkey not allowed");
	});

	test("downloads are checked against the named hash", async () => {
		const blob = await encryptBlob(pattern(10), key);
		const sha = await sha256Hex(blob);
		globalThis.fetch = (async () => new Response(blob)) as unknown as typeof fetch;
		expect(await downloadBlob(`https://blobs.test/${sha}`, sha)).toEqual(blob);
		globalThis.fetch = (async () => new Response(new Uint8Array([1, 2, 3]))) as unknown as typeof fetch;
		await expect(downloadBlob(`https://blobs.test/${sha}`, sha)).rejects.toThrow("different bytes");
	});

	test("the server is the first relay whose NIP-11 lists blossom, else Roostr's relay", async () => {
		const asked: string[] = [];
		globalThis.fetch = (async (url: string, init: RequestInit) => {
			asked.push(`${url} ${(init.headers as Record<string, string>).Accept}`);
			if (url === "https://plain.test") return Response.json({ supported_nips: [1] });
			return Response.json({ supported_features: ["blossom"] });
		}) as typeof fetch;
		expect(await blossomServer(["wss://plain.test", "ws://127.0.0.1:7777/"])).toBe("http://127.0.0.1:7777");
		expect(asked).toEqual(["https://plain.test application/nostr+json", "http://127.0.0.1:7777 application/nostr+json"]);
		globalThis.fetch = (async () => { throw new TypeError("offline"); }) as unknown as typeof fetch;
		expect(await blossomServer(["wss://down.test"])).toBe(DEFAULT_BLOSSOM_SERVER);
	});
});
