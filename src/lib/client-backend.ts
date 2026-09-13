/** Platform selection is explicit, never inferred from the page hostname. */
import { backend as browserBackend } from "./engine/backend";
import { LocalBackend } from "./local-backend";
import { IOSBackend } from "./ios-backend";
export type { SyncStatus } from "./engine/backend";
export const isLocalBackend = import.meta.env.VITE_ROOSTR_BACKEND === "local";
// The iOS host injects this marker at document start, before any module runs; the global type does not declare it.
const platformGlobal = globalThis as { __ROOSTR_PLATFORM?: string };
export const isIOSBackend = platformGlobal.__ROOSTR_PLATFORM === "ios";
export const backend = isLocalBackend ? new LocalBackend() : isIOSBackend ? new IOSBackend() : browserBackend;
