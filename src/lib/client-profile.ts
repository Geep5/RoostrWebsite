import { isLocalBackend } from "./client-backend";
import { harnessFetch, onPairingChange, pairedSession } from "./local-transport";
import { cachedProfile as browserCached, fetchProfile as browserFetch, saveProfile as browserSave, type Profile } from "./engine/profile";
export { imageToAvatar } from "./engine/profile";
export type { Profile } from "./engine/profile";
let nativeProfile: Profile = {};
onPairingChange(() => { if (!pairedSession()) nativeProfile = {}; });
export function cachedProfile(): Profile { return isLocalBackend ? nativeProfile : browserCached(); }
async function nativeRequest(init?: RequestInit): Promise<Profile> {
	const response = await harnessFetch("/profile", init);
	if (!response.ok) throw new Error(`Local profile unavailable (${response.status})`);
	const profile: Profile = await response.json();
	nativeProfile = profile;
	return profile;
}
export function fetchProfile(): Promise<Profile> { return isLocalBackend ? nativeRequest() : browserFetch(); }
export function saveProfile(patch: Profile): Promise<Profile> {
	return isLocalBackend ? nativeRequest({ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(patch) }) : browserSave(patch);
}
