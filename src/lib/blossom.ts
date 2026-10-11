/**
 * Files on Blossom (BUD-01/02/11), served by RoostrRelay on its own origin
 * (glonOdin/docs/state-sync.md "Files on Blossom"). Every host agrees on the
 * blob format, so a file added here opens on the iOS app and the computers:
 *
 *   fresh random 32-byte key per file; AES-256-GCM over 4 MiB plaintext
 *   frames; frame i's nonce = 12-byte big-endian i; each frame's output is
 *   ciphertext || 16-byte tag; the blob is their concatenation. The last
 *   frame may be shorter; a zero-length file is one empty frame (the tag).
 *
 * Blobs are public by hash and opaque without the key, which only travels
 * inside the (relay-encrypted) File object.
 */
import type { Event, EventTemplate } from "nostr-tools";
import { bytesToBase64 } from "./engine/proto";

/** Plaintext bytes per frame. */
export const BLOB_FRAME_BYTES = 4 * 1024 * 1024;
const TAG_BYTES = 16;
/** BUD-11 authorization event kind. */
export const BLOSSOM_AUTH_KIND = 24242;
/** Upload authorizations expire this long after signing. */
const AUTH_TTL_S = 600;
export const DEFAULT_BLOSSOM_SERVER = "https://roostr-relay.fly.dev";

/** BUD-02 blob descriptor. */
export interface BlobDescriptor {
	url: string;
	sha256: string;
	size: number;
	type?: string;
	uploaded?: number;
}

/** Signs a kind-24242 template: the vault key in the browser, the host over the iOS bridge. */
export type BlossomSigner = (template: EventTemplate) => Promise<Event>;

function frameNonce(index: number): Uint8Array<ArrayBuffer> {
	const nonce = new Uint8Array(12);
	new DataView(nonce.buffer).setBigUint64(4, BigInt(index));
	return nonce;
}

function aesKey(key: Uint8Array, usage: KeyUsage): Promise<CryptoKey> {
	if (key.length !== 32) throw new Error("a blob key is 32 bytes");
	return crypto.subtle.importKey("raw", new Uint8Array(key), "AES-GCM", false, [usage]);
}

/** Encrypt a whole file into the framed blob format. */
export async function encryptBlob(plain: Uint8Array, key: Uint8Array): Promise<Uint8Array> {
	const aes = await aesKey(key, "encrypt");
	const frames = Math.max(1, Math.ceil(plain.length / BLOB_FRAME_BYTES));
	const out = new Uint8Array(plain.length + frames * TAG_BYTES);
	for (let i = 0; i < frames; i++) {
		const chunk = plain.slice(i * BLOB_FRAME_BYTES, (i + 1) * BLOB_FRAME_BYTES);
		out.set(new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: frameNonce(i) }, aes, chunk)), i * (BLOB_FRAME_BYTES + TAG_BYTES));
	}
	return out;
}

/** Decrypt a framed blob; any tampered, truncated or reordered frame fails. */
export async function decryptBlob(blob: Uint8Array, key: Uint8Array): Promise<Uint8Array> {
	const aes = await aesKey(key, "decrypt");
	const stride = BLOB_FRAME_BYTES + TAG_BYTES;
	const frames = Math.max(1, Math.ceil(blob.length / stride));
	const size = blob.length - frames * TAG_BYTES;
	if (size < 0 || blob.length - (frames - 1) * stride < TAG_BYTES) throw new Error("not an encrypted Roostr blob (truncated frame)");
	const out = new Uint8Array(size);
	for (let i = 0; i < frames; i++) {
		const frame = blob.slice(i * stride, (i + 1) * stride);
		let plain: ArrayBuffer;
		try {
			plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: frameNonce(i) }, aes, frame);
		} catch {
			throw new Error(`the file could not be decrypted (frame ${i} failed authentication)`);
		}
		out.set(new Uint8Array(plain), i * BLOB_FRAME_BYTES);
	}
	return out;
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
	const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new Uint8Array(bytes)));
	let hex = "";
	for (const byte of digest) hex += byte.toString(16).padStart(2, "0");
	return hex;
}

/** BUD-11 upload authorization for one blob, valid for AUTH_TTL_S. */
export function uploadAuthTemplate(sha256: string, name: string, now = Math.floor(Date.now() / 1000)): EventTemplate {
	return {
		kind: BLOSSOM_AUTH_KIND,
		created_at: now,
		tags: [["t", "upload"], ["x", sha256], ["expiration", String(now + AUTH_TTL_S)]],
		content: `Upload ${name}`,
	};
}

/** `Authorization` header value: "Nostr " + base64 of the signed event's JSON (UTF-8: names may be any script). */
export function authorizationHeader(event: Event): string {
	return `Nostr ${bytesToBase64(new TextEncoder().encode(JSON.stringify(event)))}`;
}

const servers = new Map<string, Promise<string>>();

/**
 * The Blossom server for these relays: the https origin of the first relay
 * whose NIP-11 document lists the "blossom" feature, else Roostr's relay.
 * Cached per relay list for the page's life.
 */
export function blossomServer(relays: string[]): Promise<string> {
	const key = relays.join(" ");
	let server = servers.get(key);
	if (!server) {
		server = (async () => {
			for (const relay of relays) {
				let origin: string;
				try {
					const url = new URL(relay);
					url.protocol = url.protocol === "ws:" ? "http:" : "https:";
					origin = url.origin;
				} catch {
					continue;
				}
				try {
					const res = await fetch(origin, { headers: { Accept: "application/nostr+json" }, signal: AbortSignal.timeout(5_000) });
					const doc = (await res.json()) as { supported_features?: unknown };
					if (Array.isArray(doc.supported_features) && doc.supported_features.includes("blossom")) return origin;
				} catch {
					/* unreachable or no NIP-11: try the next relay */
				}
			}
			return DEFAULT_BLOSSOM_SERVER;
		})();
		servers.set(key, server);
	}
	return server;
}

/** Why the server refused, as BUD-01 servers say it (X-Reason), else the status. */
function refusal(what: string, res: Response): Error {
	return new Error(`${what} failed: ${res.headers.get("X-Reason") || `${res.status} ${res.statusText}`.trim()}`);
}

/** PUT /upload with a fresh BUD-11 authorization; resolves with the server's descriptor. */
export async function uploadBlob(server: string, blob: Uint8Array, sha256: string, name: string, sign: BlossomSigner): Promise<BlobDescriptor> {
	const auth = await sign(uploadAuthTemplate(sha256, name));
	const res = await fetch(`${server}/upload`, {
		method: "PUT",
		headers: { Authorization: authorizationHeader(auth), "X-SHA-256": sha256, "Content-Type": "application/octet-stream" },
		body: new Uint8Array(blob),
	});
	if (!res.ok) throw refusal("Upload to Blossom", res);
	const descriptor = (await res.json()) as BlobDescriptor;
	if (descriptor.sha256 !== sha256) throw new Error(`Blossom stored ${descriptor.sha256}, not the uploaded ${sha256}`);
	return descriptor;
}

/** GET a blob and check it is the one named: a server is never trusted with the bytes. */
export async function downloadBlob(url: string, sha256: string): Promise<Uint8Array> {
	const res = await fetch(url);
	if (!res.ok) throw refusal("Download from Blossom", res);
	const bytes = new Uint8Array(await res.arrayBuffer());
	if ((await sha256Hex(bytes)) !== sha256) throw new Error("Blossom returned different bytes than the file names");
	return bytes;
}
