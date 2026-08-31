/**
 * Space keys (web replica) - the browser counterpart of the desktop's
 * <data>/channel-keys.json. Same shape, stored in localStorage:
 *   { version: 1, channels: { [spaceId]: { key, keyId, createdAt, owner? } } }
 *
 * `key` is 64-hex (32 bytes) and doubles as the NIP-44 conversation key
 * for shared-space sync. `owner` (hex pubkey) is set on IMPORTED entries
 * (spaces someone else invited us into) and marks who administers them.
 */

const STORE = "roostr-space-keys";

export interface SpaceKeyEntry {
	key: string;
	keyId: number;
	createdAt: number;
	owner?: string;
}

interface KeyFile {
	version: 1;
	channels: Record<string, SpaceKeyEntry>;
}

function readFile(): KeyFile {
	try {
		const parsed = JSON.parse(localStorage.getItem(STORE) ?? "null") as KeyFile | null;
		if (parsed?.version === 1 && parsed.channels) return parsed;
	} catch {
		/* fresh */
	}
	return { version: 1, channels: {} };
}

function writeFile(file: KeyFile): void {
	localStorage.setItem(STORE, JSON.stringify(file));
}

function randomKeyHex(): string {
	const raw = new Uint8Array(32);
	crypto.getRandomValues(raw);
	return [...raw].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function spaceKeyGet(spaceId: string): SpaceKeyEntry | null {
	return readFile().channels[spaceId] ?? null;
}

export function spaceKeyAll(): Record<string, SpaceKeyEntry> {
	return readFile().channels;
}

/** Create a key for a locally-created space (keyId 1). Idempotent. */
export function spaceKeyEnsure(spaceId: string): SpaceKeyEntry {
	const file = readFile();
	const existing = file.channels[spaceId];
	if (existing) return existing;
	const entry: SpaceKeyEntry = { key: randomKeyHex(), keyId: 1, createdAt: Date.now() };
	file.channels[spaceId] = entry;
	writeFile(file);
	return entry;
}

/** Rotate on member removal: new key, keyId+1. */
export function spaceKeyRotate(spaceId: string): SpaceKeyEntry {
	const file = readFile();
	const prev = file.channels[spaceId];
	const entry: SpaceKeyEntry = { key: randomKeyHex(), keyId: (prev?.keyId ?? 0) + 1, createdAt: Date.now(), ...(prev?.owner ? { owner: prev.owner } : {}) };
	file.channels[spaceId] = entry;
	writeFile(file);
	return entry;
}

/** Import a key from an invite link. Keeps a newer local keyId if present. */
export function spaceKeyImport(spaceId: string, key: string, keyId: number, owner: string): SpaceKeyEntry {
	const file = readFile();
	const prev = file.channels[spaceId];
	if (prev && prev.keyId >= keyId) return prev;
	const entry: SpaceKeyEntry = { key, keyId, createdAt: Date.now(), owner };
	file.channels[spaceId] = entry;
	writeFile(file);
	return entry;
}
