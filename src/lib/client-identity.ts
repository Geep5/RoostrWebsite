import { backend, isLocalBackend } from "./client-backend";
import { LocalBackend } from "./local-backend";
import { harnessFetch, pairedSession } from "./local-transport";
import { myNpub as browserNpub, listJoinRequests as browserRequests, clearJoinRequest as browserClear, sendJoinRequest as browserSend, type JoinRequest } from "./engine/sync";
import type { SpaceJoinLink } from "./invite";
export type { JoinRequest } from "./engine/sync";
export function myNpub(): string | null { return isLocalBackend ? (pairedSession() && backend instanceof LocalBackend ? backend.npub || null : null) : browserNpub(); }
export async function listJoinRequests(): Promise<JoinRequest[]> {
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
	if (!isLocalBackend) return browserClear(key);
	const response = await harnessFetch("/join-requests/clear", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ key }) });
	if (!response.ok) throw new Error(`Cannot clear join request (${response.status})`);
}
export async function sendJoinRequest(link: SpaceJoinLink): Promise<void> {
	if (!isLocalBackend) return browserSend(link);
	const response = await harnessFetch("/join-requests/send", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(link) });
	if (!response.ok) throw new Error((await response.text()) || `Cannot send join request (${response.status})`);
}
