/**
 * File objects: the bytes live in each computer's harness blob store and
 * move between the owner's computers peer-to-peer. This tab only ever
 * talks to its paired local harness.
 */
import { harnessFetch } from "$lib/local-transport";

/** Shown wherever files are used without a local harness to hold the bytes. */
export const FILES_NEED_LOCAL = "Files move between your Roostr computers; open this on a computer running Roostr.";

/** The harness's `{ error }` sentence, else the HTTP status. */
async function failure(res: Response): Promise<Error> {
	const body = (await res.json().catch(() => null)) as { error?: unknown } | null;
	const message = typeof body?.error === "string" && body.error ? body.error : `${res.status} ${res.statusText}`.trim();
	return new Error(message);
}

/** Store `file`'s bytes on this computer's harness, which creates the File object in `spaceId`. */
export async function uploadFile(file: File, spaceId: string): Promise<{ id: string; hash: string; size: number }> {
	const query = new URLSearchParams({ name: file.name, mime: file.type, space: spaceId });
	const res = await harnessFetch(`/files?${query}`, {
		method: "POST",
		headers: { "Content-Type": "application/octet-stream" },
		body: file,
	});
	if (!res.ok) throw await failure(res);
	return (await res.json()) as { id: string; hash: string; size: number };
}

/** The bytes for `hash`; the harness fetches them from a peer first when this computer lacks them. */
export async function fetchFileBlob(hash: string): Promise<Blob> {
	const res = await harnessFetch(`/files/${encodeURIComponent(hash)}`);
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
