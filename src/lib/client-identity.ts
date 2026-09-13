import { backend, isLocalBackend } from "./client-backend";
import { LocalBackend } from "./local-backend";
import { IOSBackend } from "./ios-backend";
import { harnessFetch, pairedSession } from "./local-transport";
import { myNpub as browserNpub, listJoinRequests as browserRequests, clearJoinRequest as browserClear, sendJoinRequest as browserSend, importSpaceInvite as browserImport, type JoinRequest } from "./engine/sync";
import type { SpaceJoinLink } from "./invite";
export type { JoinRequest } from "./engine/sync";
/** Synchronous view of this device's identity: the iOS host's npub is cached by `backend.start()`, like the daemon's in local mode. */
export function myNpub(): string | null {
	if (isLocalBackend) return pairedSession() && backend instanceof LocalBackend ? backend.npub || null : null;
	if (backend instanceof IOSBackend) return backend.npub || null;
	return browserNpub();
}
/** Like `myNpub`, but asks the iOS host when the backend has not started yet (the /j page runs outside the app shell). */
export async function loadNpub(): Promise<string | null> {
	if (backend instanceof IOSBackend) return (await backend.identity()).npub || null;
	return myNpub();
}
export async function listJoinRequests(): Promise<JoinRequest[]> {
	if (backend instanceof IOSBackend) return backend.joinRequests();
	if (!isLocalBackend) return browserRequests();
	const response = await harnessFetch("/join-requests");
	if (!response.ok) throw new Error(`Join requests unavailable (${response.status})`);
	const result = await response.json();
	if (!result || !Array.isArray(result.requests)) throw new Error("Invalid join requests response");
	return result.requests.map((row: Record<string, unknown>) => {
		if (!row || typeof row.key !== "string" || typeof row.space !== "string" || typeof row.requesterNpub !== "string" || typeof row.at !== "number") throw new Error("Invalid join request");
		return { key: row.key, space: row.space, requesterNpub: row.requesterNpub, at: row.at, spaceName: typeof row.spaceName === "string" ? row.spaceName : "", requester: typeof row.requester === "string" ? row.requester : "", name: typeof row.name === "string" ? row.name : undefined, picture: typeof row.picture === "string" ? row.picture : undefined };
	});
}
export async function clearJoinRequest(key: string): Promise<void> {
	if (backend instanceof IOSBackend) return backend.clearJoinRequest(key);
	if (!isLocalBackend) return browserClear(key);
	const response = await harnessFetch("/join-requests/clear", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ key }) });
	if (!response.ok) throw new Error(`Cannot clear join request (${response.status})`);
}
export async function sendJoinRequest(link: SpaceJoinLink): Promise<void> {
	if (backend instanceof IOSBackend) return backend.sendJoinRequest(link);
	if (!isLocalBackend) return browserSend(link);
	const response = await harnessFetch("/join-requests/send", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(link) });
	if (!response.ok) throw new Error((await response.text()) || `Cannot send join request (${response.status})`);
}
/** Key-carrying invite import: the iOS host keeps its own keyring; the browser writes localStorage. Not available in local mode (the daemon owns its keys). */
export async function importSpaceInvite(inv: { space: string; owner: string; key: string; keyId: number }): Promise<boolean> {
	if (backend instanceof IOSBackend) return backend.importSpaceInvite(inv);
	if (isLocalBackend) throw new Error("Import space invites from the daemon terminal in local mode.");
	return browserImport(inv);
}
