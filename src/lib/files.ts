/**
 * File objects: the bytes live in each computer's harness blob store and
 * move between the owner's computers peer-to-peer. A tab only ever talks to
 * the harness on its own computer: the local build through its terminal
 * pairing, the hosted app by proving it holds the vault owner key the first
 * time a file is added or shown (and again whenever that session lapses).
 * The iOS app has no Roostr underneath it, so files stay unavailable there.
 */
import { isIOSBackend, isLocalBackend } from "$lib/client-backend";
import { loadKey } from "$lib/engine/keys";
import { PairingError, harnessFetch, pairWithOwnerKey, pairedSession } from "$lib/local-transport";

/** Files need a Roostr computer under the tab: everywhere but the iOS app. */
export const filesSupported = !isIOSBackend;
/** Shown where files can never work (the iOS app). */
export const FILES_NEED_LOCAL = "Files move between your Roostr computers; open this on a computer running Roostr.";
const NOT_RUNNING_TO_ADD = "Roostr isn't running on this computer - start it to add files.";
const NOT_RUNNING_TO_VIEW = "Start Roostr on this computer to see this file.";

/** Nothing answered on this computer; `blocked` when the browser refused local network access rather than Roostr being absent. */
class LocalRoostrUnreachable extends Error {
	readonly blocked: boolean;
	constructor(blocked: boolean) {
		super(blocked ? "This browser blocked the page from reaching Roostr on this computer - allow local network access for this site in the address bar's site settings, then retry." : "Roostr isn't running on this computer.");
		this.name = "LocalRoostrUnreachable";
		this.blocked = blocked;
	}
}

/** Why `name` could not be added, as the person should read it. */
export function addFailureText(err: unknown, name: string): string {
	if (err instanceof LocalRoostrUnreachable) return err.blocked ? err.message : NOT_RUNNING_TO_ADD;
	return `Could not add ${name}: ${err instanceof Error ? err.message : String(err)}`;
}

/** Why a file's bytes could not be shown: Roostr out of reach here, else the harness's reason (no holder online…). */
export function viewFailureText(err: unknown): string {
	if (err instanceof LocalRoostrUnreachable) return err.blocked ? err.message : NOT_RUNNING_TO_VIEW;
	return err instanceof Error ? err.message : String(err);
}

/** Chrome's local network permission (named per version); "denied" means the browser said no, not that Roostr is absent. */
async function localAccessDenied(): Promise<boolean> {
	for (const name of ["loopback-network", "local-network-access"]) {
		try {
			if ((await navigator.permissions.query({ name: name as PermissionName })).state === "denied") return true;
		} catch {
			/* this browser does not know the name */
		}
	}
	return false;
}

/** One proof at a time: every file on the page waits on the same pairing. */
let ownerPairing: Promise<void> | null = null;

/**
 * The harness on this computer. The hosted app pairs (or re-pairs, once,
 * when the daemon rejects a lapsed session) by signing with the owner key.
 */
async function filesFetch(path: string, init?: RequestInit): Promise<Response> {
	try {
		if (isLocalBackend) return await harnessFetch(path, init);
		for (let attempt = 0; ; attempt++) {
			if (!pairedSession()) {
				const key = loadKey();
				if (!key) throw new PairingError("Sign in with your key to use files.");
				ownerPairing ??= pairWithOwnerKey(key.sk).finally(() => (ownerPairing = null));
				await ownerPairing;
			}
			try {
				return await harnessFetch(path, init);
			} catch (err) {
				// A rejected session is already dropped (next loop pairs again); a
				// pairing replaced mid-flight is simply used.
				if (!(err instanceof PairingError) || attempt > 0) throw err;
			}
		}
	} catch (err) {
		// fetch rejects with TypeError only when nothing usable answered.
		if (err instanceof TypeError) throw new LocalRoostrUnreachable(await localAccessDenied());
		throw err;
	}
}

/** The harness's `{ error }` sentence, else the HTTP status. */
async function failure(res: Response): Promise<Error> {
	const body = (await res.json().catch(() => null)) as { error?: unknown } | null;
	const message = typeof body?.error === "string" && body.error ? body.error : `${res.status} ${res.statusText}`.trim();
	return new Error(message);
}

/** Store `file`'s bytes on this computer's harness, which creates the File object in `spaceId`. */
export async function uploadFile(file: File, spaceId: string): Promise<{ id: string; hash: string; size: number }> {
	const query = new URLSearchParams({ name: file.name, mime: file.type, space: spaceId });
	const res = await filesFetch(`/files?${query}`, {
		method: "POST",
		headers: { "Content-Type": "application/octet-stream" },
		body: file,
	});
	if (!res.ok) throw await failure(res);
	return (await res.json()) as { id: string; hash: string; size: number };
}

/** The bytes for `hash`; the harness fetches them from a peer first when this computer lacks them. */
export async function fetchFileBlob(hash: string): Promise<Blob> {
	const res = await filesFetch(`/files/${encodeURIComponent(hash)}`);
	if (!res.ok) throw await failure(res);
	return res.blob();
}

/**
 * Open the system file picker. Resolves null when the user cancels. Must
 * run within the click that asked for it (browsers require a user gesture).
 */
export function pickFile(): Promise<File | null> {
	const { promise, resolve } = Promise.withResolvers<File | null>();
	const input = document.createElement("input");
	input.type = "file";
	input.style.display = "none";
	const done = (file: File | null) => {
		input.remove();
		resolve(file);
	};
	input.addEventListener("change", () => done(input.files?.[0] ?? null), { once: true });
	input.addEventListener("cancel", () => done(null), { once: true });
	document.body.append(input);
	input.click();
	return promise;
}

/** Bytes as the header shows them: B, KB, MB, GB (1024-based). */
export function humanSize(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return "";
	if (bytes < 1024) return `${bytes} B`;
	const units = ["KB", "MB", "GB", "TB"];
	let value = bytes / 1024;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
