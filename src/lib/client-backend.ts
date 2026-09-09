/** Platform selection is explicit, never inferred from the page hostname. */
import { backend as browserBackend } from "./engine/backend";
import { LocalBackend } from "./local-backend";
export type { SyncStatus } from "./engine/backend";
export const isLocalBackend = import.meta.env.VITE_ROOSTR_BACKEND === "local";
export const backend = isLocalBackend ? new LocalBackend() : browserBackend;
