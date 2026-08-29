/**
 * keys.ts — nsec handling + on-device storage.
 *
 * Accepts either an nsec1… bech32 string or a 64-char hex secret key and
 * derives the KeyInfo contract shape. Persists the hex secret under
 * localStorage['roostr-key'] (guarded so tests without a DOM still work).
 */

import { getPublicKey, nip19 } from "nostr-tools";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { KeyInfo } from "./contracts";

const STORAGE_KEY = "roostr-key";

const HEX64 = /^[0-9a-fA-F]{64}$/;

/** Parse an nsec1… or 64-hex secret key. Returns null on anything else. */
export function parseKey(input: string): KeyInfo | null {
	const trimmed = input.trim();
	let sk: Uint8Array | null = null;
	if (HEX64.test(trimmed)) {
		sk = hexToBytes(trimmed.toLowerCase());
	} else if (trimmed.startsWith("nsec1")) {
		try {
			const decoded = nip19.decode(trimmed);
			if (decoded.type === "nsec") sk = decoded.data;
		} catch {
			return null;
		}
	}
	if (!sk || sk.length !== 32) return null;
	try {
		const pk = getPublicKey(sk);
		return { sk, pk, npub: nip19.npubEncode(pk) };
	} catch {
		return null; // out-of-range scalar etc.
	}
}

/**
 * Author id, parity with the Odin server (nostr.odin): sha256 of the
 * UTF-8 bytes of the 64-char privkey hex STRING (not the raw key bytes),
 * first 8 bytes as 16 hex chars.
 */
export function authorIdFor(key: KeyInfo): string {
	const hexString = bytesToHex(key.sk);
	return bytesToHex(sha256(new TextEncoder().encode(hexString))).slice(0, 16);
}

/** Parse + persist. Throws on unparseable input. */
export function saveKey(input: string): KeyInfo {
	const info = parseKey(input);
	if (!info) throw new Error("invalid key: expected nsec1… or 64 hex chars");
	if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, bytesToHex(info.sk));
	return info;
}

export function loadKey(): KeyInfo | null {
	if (typeof localStorage === "undefined") return null;
	const hex = localStorage.getItem(STORAGE_KEY);
	if (!hex) return null;
	return parseKey(hex);
}

export function clearKey(): void {
	if (typeof localStorage === "undefined") return;
	localStorage.removeItem(STORAGE_KEY);
}
