import type { ObjectJSON } from "$lib/types";
import type { ChangeJSON, ReplayApi } from "./contracts";
import { coreCall } from "./core";
import { bytesToBase64 } from "./proto";

/** Replay through the same Odin implementation used by the native engine. */
export function computeObject(changes: ChangeJSON[], checkpoint?: Uint8Array): ObjectJSON | null {
	return coreCall<ObjectJSON | null>("replay", checkpoint ? { changes, checkpoint: bytesToBase64(checkpoint) } : { changes });
}

export const replay: ReplayApi = { computeObject };
