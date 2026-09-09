import type { ObjectJSON } from "$lib/types";
import type { ChangeJSON, ReplayApi } from "./contracts";
import { coreCall } from "./core";

/** Replay through the same Odin implementation used by the native engine. */
export function computeObject(changes: ChangeJSON[]): ObjectJSON | null {
	return coreCall<ObjectJSON | null>("replay", { changes });
}

export const replay: ReplayApi = { computeObject };
