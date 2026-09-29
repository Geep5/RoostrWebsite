/**
 * A credential's harness actions, shared by its Status property. The
 * computer its Served by names opens the sign-in window and writes the
 * credential's `status`; Connect, Check and Disconnect only succeed on that
 * computer's harness (anywhere else answers 409 with a sentence).
 */

import { fetchObject } from "$lib/api";
import { harnessFetch } from "$lib/local-transport";
import type { BadgeIcon } from "$lib/options";
import { fieldStr, type ValueJSON } from "$lib/types";

export type CredentialAction = "connect" | "check" | "disconnect";

/** What the harness answers: the credential's new status, or a refusal in `error`. */
export interface CredentialResult {
	status?: string;
	error?: string;
}

const POLL_MS = 3_000;
const CONNECT_TIMEOUT_MS = 5 * 60_000;
/** The fields the owning harness rewrites while a sign-in runs. */
const HARNESS_FIELDS = ["status", "error", "checked_at", "auth"];

/** The credential's status in words. */
export function credentialStatusText(status: string): string {
	switch (status || "missing") {
		case "active": return "Connected";
		case "connecting": return "Waiting for sign-in";
		case "needs_auth": return "Signed out - reconnect";
		case "broken": return "Broken";
		case "missing": return "Not connected";
		default: return status.replaceAll("_", " ");
	}
}

/** The status row's badge: glyph and options.ts colour name. */
export function credentialStatusBadge(status: string): { icon: BadgeIcon; color: string } {
	switch (status || "missing") {
		case "active": return { icon: "check", color: "lime" };
		case "connecting":
		case "needs_auth": return { icon: "clock", color: "orange" };
		case "broken": return { icon: "x", color: "red" };
		default: return { icon: "dashed", color: "" };
	}
}

/** Sign-in through a browser window needs the login page and the cookie (host + name) that proves it worked. */
export function hasBrowserSignIn(fields: Record<string, ValueJSON>): boolean {
	return ["login_url", "session_host", "session_cookie"].every((k) => fieldStr(fields, k).trim() !== "");
}

/** Run one action on the paired harness. Throws the harness's sentence when it refuses. */
export async function runCredentialAction(action: CredentialAction, id: string): Promise<CredentialResult> {
	const res = await harnessFetch(`/credentials/${action}`, { method: "POST", body: JSON.stringify({ id }) });
	const result = (await res.json().catch(() => ({}))) as CredentialResult;
	// A body with `error` and no `status` is a refusal (409 included).
	if (!res.ok || (result.error && !result.status)) throw new Error(result.error ?? `HTTP ${res.status}`);
	return result;
}

/**
 * While a sign-in window is open the harness writes the credential's status:
 * re-read it every 3 s, call `onchanged` when a harness field moved, and stop
 * once it is no longer `connecting` - or after 5 minutes, calling `ontimeout`.
 * Returns the stop function.
 */
export function pollWhileConnecting(
	id: string,
	current: () => Record<string, ValueJSON>,
	onchanged: () => Promise<void>,
	ontimeout: () => void,
): () => void {
	const deadline = Date.now() + CONNECT_TIMEOUT_MS;
	let busy = false;
	const stop = () => clearInterval(timer);
	const timer = setInterval(async () => {
		if (busy) return;
		if (Date.now() > deadline) {
			stop();
			ontimeout();
			return;
		}
		busy = true;
		try {
			const fresh = await fetchObject(id).catch(() => null);
			if (!fresh) return;
			const before = current();
			if (HARNESS_FIELDS.some((k) => JSON.stringify(fresh.fields[k]) !== JSON.stringify(before[k]))) await onchanged().catch(() => {});
			if (fieldStr(fresh.fields, "status") !== "connecting") stop();
		} finally {
			busy = false;
		}
	}, POLL_MS);
	return stop;
}
