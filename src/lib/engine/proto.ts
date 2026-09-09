/** Shared Odin codec. Await initCore() before using this synchronous API.
 * Change ids/parents remain hex; nested byte payloads remain base64 JSON.
 */
import { coreCall } from "./core";
import { packCoreValueMaps, unpackCoreValueMaps } from "./core-values";
import type { ChangeJSON, ProtoApi } from "./contracts";

function base64ToBytes(base64: string): Uint8Array {
	const binary = atob(base64);
	return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

export function decodeChange(bytes: Uint8Array): ChangeJSON | null {
	let binary = "";
	for (const byte of bytes) binary += String.fromCharCode(byte);
	try {
		return unpackCoreValueMaps<ChangeJSON>(coreCall("codec", { action: "decode", bytes: btoa(binary) }));
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

export const proto: ProtoApi = { decodeChange, encodeChange, changeId };
