/** Shared Odin codec. Await initCore() before using this synchronous API.
 * Change ids/parents remain hex; nested byte payloads remain base64 JSON.
 */
import { coreCall } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";
import type { ChangeJSON, ProtoApi } from "./contracts";

export function base64ToBytes(base64: string): Uint8Array {
	const binary = atob(base64);
	const out = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
	return out;
}

export function bytesToBase64(bytes: Uint8Array): string {
	let binary = "";
	for (let i = 0; i < bytes.length; i += 0x8000) {
		binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
	}
	return btoa(binary);
}

export function decodeChange(bytes: Uint8Array): ChangeJSON | null {
	try {
		return unpackCoreValueMaps<ChangeJSON>(coreCall("codec", { action: "decode", bytes: bytesToBase64(bytes) }));
	} catch {
		return null;
	}
}

/** Hashes the legacy preimage including the zeroed id field (0a00). */
export function changeId(change: ChangeJSON): string {
	return coreCall<string>("codec", { action: "hash", change: packCoreValueMaps(change) });
}

/** Recomputes the content address and returns canonical protobuf bytes. */
export function encodeChange(change: ChangeJSON): Uint8Array {
	return base64ToBytes(coreCall<string>("codec", { action: "encode", change: packCoreValueMaps(change) }));
}

/** The one store rule for checkpoints (core.checkpoint_supersedes): `candidate` covers a strict superset, or the same set with the larger hash. */
export function checkpointSupersedes(candidate: Uint8Array, existing: Uint8Array): boolean {
	return coreCall<boolean>("codec", { action: "checkpoint_supersedes", candidate: bytesToBase64(candidate), existing: bytesToBase64(existing) });
}

export const proto: ProtoApi = { decodeChange, encodeChange, changeId };
