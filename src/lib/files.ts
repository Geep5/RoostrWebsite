/**
 * File objects. A file's bytes live on Blossom, encrypted (see blossom.ts):
 * the File object names the blob (`blob_sha256`, `blob_url`) and holds its
 * key (`blob_key`), so every host - this tab, the iOS app, the computers -
 * opens it. The hosted app adds a file by encrypting and uploading it
 * itself, then creating the File object; with a Roostr computer paired to
 * this tab (and always in the local build) the harness adds it instead,
 * which also keeps the bytes in its peer-to-peer store and uploads them to
 * Blossom. Files added before Blossom name only `file_hash`: their bytes
 * come from this computer's harness, peer-to-peer, as before.
 */
import type { Event, EventTemplate } from "nostr-tools";
import { finalizeEvent } from "nostr-tools";
import { hexToBytes } from "@noble/hashes/utils.js";
import { backend, isIOSBackend, isLocalBackend } from "$lib/client-backend";
import type { IOSBackend } from "$lib/ios-backend";
import { note } from "$lib/api";
import { blossomServer, decryptBlob, downloadBlob, encryptBlob, sha256Hex, uploadBlob, type BlossomSigner } from "$lib/blossom";
import { loadKey } from "$lib/engine/keys";
import { PairingError, harnessFetch, pairWithOwnerKey, pairedSession } from "$lib/local-transport";
import type { ValueJSON } from "$lib/types";

/** A file from before Blossom, opened where no Roostr computer can serve it (the iOS app). */
const FILES_NEED_LOCAL = "This file was added before files moved to Blossom; open it on a computer running Roostr.";
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
async function uploadToHarness(file: File, spaceId: string): Promise<{ id: string; hash: string; size: number }> {
	const query = new URLSearchParams({ name: file.name, mime: file.type, space: spaceId });
	const res = await filesFetch(`/files?${query}`, {
		method: "POST",
		headers: { "Content-Type": "application/octet-stream" },
		body: file,
	});
	if (!res.ok) throw await failure(res);
	return (await res.json()) as { id: string; hash: string; size: number };
}

/** Who signs the upload authorization: the iOS host over its bridge (the key never enters the page), else the vault key. */
function blossomSigner(): BlossomSigner {
	if (isIOSBackend) return (template: EventTemplate): Promise<Event> => (backend as IOSBackend).signEvent(template);
	const key = loadKey();
	if (!key) throw new Error("Sign in with your key to add files.");
	return async (template) => finalizeEvent(template, key.sk);
}

/** Encrypt `file` under a fresh key, upload it to Blossom, then create its File object in `spaceId`. */
async function uploadToBlossom(file: File, spaceId: string): Promise<{ id: string; hash: string; size: number }> {
	const sign = blossomSigner();
	const plain = new Uint8Array(await file.arrayBuffer());
	const key = crypto.getRandomValues(new Uint8Array(32));
	const blob = await encryptBlob(plain, key);
	const [hash, blobHash] = await Promise.all([sha256Hex(plain), sha256Hex(blob)]);
	const descriptor = await uploadBlob(await blossomServer(backend.relays()), blob, blobHash, file.name, sign);
	let blobKey = "";
	for (const byte of key) blobKey += byte.toString(16).padStart(2, "0");
	const fields: Record<string, ValueJSON> = {
		file_hash: { stringValue: hash },
		file_size: { intValue: plain.length },
		file_mime: { stringValue: file.type },
		blob_sha256: { stringValue: blobHash },
		blob_key: { stringValue: blobKey },
		blob_url: { stringValue: descriptor.url },
	};
	if (spaceId) fields.channel = { stringValue: spaceId };
	const { id } = await note.create(file.name || "Untitled file", "file", fields);
	return { id, hash, size: plain.length };
}

/**
 * Add `file` to `spaceId` and return its File object. The local build and a
 * hosted tab paired with a running Roostr computer go through the harness;
 * otherwise (no computer, or the paired one not running) straight to Blossom.
 */
export async function uploadFile(file: File, spaceId: string): Promise<{ id: string; hash: string; size: number }> {
	if (isLocalBackend) return uploadToHarness(file, spaceId);
	if (!isIOSBackend && pairedSession()) {
		try {
			return await uploadToHarness(file, spaceId);
		} catch (err) {
			if (!(err instanceof LocalRoostrUnreachable)) throw err;
		}
	}
	return uploadToBlossom(file, spaceId);
}

/** The bytes of a File object (its fields): from Blossom when it names a blob, else from this computer's harness, which fetches them from a peer when it lacks them. */
export async function fetchFileBlob(fields: Record<string, ValueJSON>): Promise<Blob> {
	const mime = fields["file_mime"]?.stringValue || "application/octet-stream";
	const blobHash = (fields["blob_sha256"]?.stringValue ?? "").toLowerCase();
	if (blobHash) {
		const key = hexToBytes(fields["blob_key"]?.stringValue ?? "");
		const named = fields["blob_url"]?.stringValue ?? "";
		const fallback = `${await blossomServer(backend.relays())}/${blobHash}`;
		let blob: Uint8Array;
		try {
			blob = await downloadBlob(named || fallback, blobHash);
		} catch (err) {
			// The descriptor's server may have moved; the configured one serves the same hash.
			if (!named || named === fallback) throw err;
			blob = await downloadBlob(fallback, blobHash);
		}
		return new Blob([new Uint8Array(await decryptBlob(blob, key))], { type: mime });
	}
	if (isIOSBackend) throw new Error(FILES_NEED_LOCAL);
	const hash = (fields["file_hash"]?.stringValue ?? "").toLowerCase();
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
