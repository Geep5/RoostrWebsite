/**
 * Identity profile (nostr kind 0) for the vault key. The picture is a
 * small data-URI (Settings downsizes uploads), synced through the relay
 * like any metadata event, so every device shows the same avatar.
 * Cached in localStorage for instant paint; refreshed from the relay.
 */

import { SimplePool, finalizeEvent, getPublicKey } from "nostr-tools";
import { loadKey } from "./keys";
import { DEFAULT_RELAYS } from "./sync";

const CACHE = "roostr-profile";

export interface Profile {
	picture?: string;
	name?: string;
	display_name?: string;
	[key: string]: unknown;
}

function relays(): string[] {
	try {
		const v = JSON.parse(localStorage.getItem("roostr-relays") ?? "null") as string[] | null;
		return v && v.length ? v : [...DEFAULT_RELAYS];
	} catch {
		return [...DEFAULT_RELAYS];
	}
}

export function cachedProfile(): Profile {
	try {
		return (JSON.parse(localStorage.getItem(CACHE) ?? "{}") as Profile) ?? {};
	} catch {
		return {};
	}
}

/** Latest kind-0 for the vault key; updates the cache. */
export async function fetchProfile(): Promise<Profile> {
	const key = loadKey();
	if (!key) return {};
	const pool = new SimplePool();
	try {
		const events = await pool.querySync(relays(), { kinds: [0], authors: [getPublicKey(key.sk)] });
		events.sort((a, b) => b.created_at - a.created_at);
		const profile = events[0] ? ((JSON.parse(events[0].content) as Profile) ?? {}) : {};
		localStorage.setItem(CACHE, JSON.stringify(profile));
		return profile;
	} catch {
		return cachedProfile();
	} finally {
		try {
			pool.close(relays());
		} catch {
			/* closed */
		}
	}
}

/** Merge-publish kind 0 (other fields survive a picture change). */
export async function saveProfile(patch: Profile): Promise<Profile> {
	const key = loadKey();
	if (!key) throw new Error("no key");
	const current = await fetchProfile();
	const next: Profile = { ...current, ...patch };
	for (const [k, v] of Object.entries(next)) if (v === undefined || v === "") delete next[k];
	const pool = new SimplePool();
	try {
		const event = finalizeEvent(
			{ kind: 0, created_at: Math.floor(Date.now() / 1000), tags: [], content: JSON.stringify(next) },
			key.sk,
		);
		await Promise.any(pool.publish(relays(), event));
		localStorage.setItem(CACHE, JSON.stringify(next));
		return next;
	} finally {
		try {
			pool.close(relays());
		} catch {
			/* closed */
		}
	}
}

/** File -> square-cropped, downscaled JPEG data URI (~10-40KB). */
export async function imageToAvatar(file: File, size = 256): Promise<string> {
	const url = URL.createObjectURL(file);
	try {
		const img = await new Promise<HTMLImageElement>((resolve, reject) => {
			const el = new Image();
			el.onload = () => resolve(el);
			el.onerror = () => reject(new Error("unreadable image"));
			el.src = url;
		});
		const side = Math.min(img.width, img.height);
		const canvas = document.createElement("canvas");
		canvas.width = size;
		canvas.height = size;
		const ctx = canvas.getContext("2d")!;
		ctx.drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, size, size);
		return canvas.toDataURL("image/jpeg", 0.85);
	} finally {
		URL.revokeObjectURL(url);
	}
}
