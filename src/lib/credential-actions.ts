/**
 * A credential's harness actions, shared by its Status property. Connect
 * runs on whichever computer this tab is paired with: the sign-in window
 * opens there and the login is saved on the credential (`cookie_*` /
 * `key_*` properties), which the computer its Served by names then keeps
 * and checks. Check runs only on that computer (anywhere else answers 409
 * with a sentence); Disconnect stops a sign-in running on this computer and
 * leaves the saved login on the credential.
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

/** The Google-account service: its sign-in is `gws auth login` on the computer Connect runs on. */
export const GOOGLE_ACCOUNT = "google-account";

/**
 * Whether Connect can sign this credential in: a Google account (gws opens
 * the browser), or a browser sign-in with its login page and the cookie
 * (host + name) that proves it worked. Either signs in on any paired computer.
 */
export function canConnect(fields: Record<string, ValueJSON>): boolean {
	if (fieldStr(fields, "service") === GOOGLE_ACCOUNT) return true;
	return ["login_url", "session_host", "session_cookie"].every((k) => fieldStr(fields, k).trim() !== "");
}

/** A saved cookie's value (`cookie_<name>`; `cookie_meta` only describes them). */
export const COOKIE_PREFIX = "cookie_";
export const COOKIE_META = "cookie_meta";

/**
 * A credential field holding a secret: a pasted key (`key_*`, not the legacy
 * `key_fields` list) or a saved cookie's value (`cookie_*`, not
 * `cookie_meta`). Its row says it is set (••••••••); its editor shows it.
 */
export function isSecretField(key: string): boolean {
	if (key.startsWith("key_")) return key.length > 4 && key !== "key_fields";
	return key.startsWith(COOKIE_PREFIX) && key.length > COOKIE_PREFIX.length && key !== COOKIE_META;
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
 * re-read it every 3 s, call `onchanged` (with the fresh fields) when a
 * harness field moved, and stop once it is no longer `connecting` - or after
 * 5 minutes, calling `ontimeout`. Returns the stop function.
 */
export function pollWhileConnecting(
	id: string,
	current: () => Record<string, ValueJSON>,
	onchanged: (fields: Record<string, ValueJSON>) => Promise<void>,
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
			if (HARNESS_FIELDS.some((k) => JSON.stringify(fresh.fields[k]) !== JSON.stringify(before[k]))) await onchanged(fresh.fields).catch(() => {});
			if (fieldStr(fresh.fields, "status") !== "connecting") stop();
		} finally {
			busy = false;
		}
	}, POLL_MS);
	return stop;
}

/** What the connect poll says when the sign-in never finished. */
export const CONNECT_TIMEOUT_TEXT = "Still not signed in after 5 minutes. Connect again when you are ready.";

/** The Connect button's word for a credential in this status. */
export function connectLabel(status: string): "Reconnect" | "Connect" {
	return status === "active" || status === "needs_auth" ? "Reconnect" : "Connect";
}

/** Where a tab may run a credential's actions, and what to say about the rest. */
export interface CredentialGate {
	/** Connect (and Disconnect, which stops a sign-in here) may run from this tab's computer. */
	canConnect: boolean;
	/** Check now may run: only on the computer its Served by names. */
	canCheck: boolean;
	/** Where to go for what this tab cannot do ("" when it can do everything). */
	reason: string;
}

/**
 * What this tab may run for a credential. A website or Google sign-in
 * connects from any paired computer - the login is saved on the credential
 * and handed to the Served by computer; checking it (and connecting
 * anything else, a database) needs a tab paired with that computer.
 */
export function credentialActionGate(paired: boolean, thisMachine: string, servedBy: string, computer: string, fields: Record<string, ValueJSON>): CredentialGate {
	if (!servedBy) return { canConnect: false, canCheck: false, reason: "Set Served by to a computer first." };
	const linked = paired && !!thisMachine;
	if (linked && thisMachine === servedBy) return { canConnect: true, canCheck: true, reason: "" };
	if (!canConnect(fields)) return { canConnect: false, canCheck: false, reason: `Open Roostr on ${computer} to connect or check this credential.` };
	if (linked) return { canConnect: true, canCheck: false, reason: `Check now runs on ${computer}.` };
	return { canConnect: false, canCheck: false, reason: `Pair this tab with one of your computers to sign in there, or open Roostr on ${computer} to check this credential.` };
}

/**
 * What to do while a sign-in started from this tab runs: `where` is the
 * computer the window opened on ("this computer"), `handsTo` the Served by
 * computer when that is another one ("" when the login stays where it was made).
 */
export function connectingHint(fields: Record<string, ValueJSON>, where: string, handsTo: string): string {
	const place = where || "its computer";
	const then = handsTo ? `; Roostr then hands the login to ${handsTo}` : "";
	return fieldStr(fields, "service") === GOOGLE_ACCOUNT
		? `Sign in to ${fieldStr(fields, "account") || "the Google account"} in the browser window gws opens on ${place}${then}.`
		: `Sign in in the Chrome window on ${place}${then}.`;
}

/** Run one action, then let the caller re-read the credential either way. Resolves to the refusal's sentence, or "". */
export async function performCredentialAction(
	action: CredentialAction,
	id: string,
	onchanged: () => Promise<void>,
	run: (action: CredentialAction, id: string) => Promise<CredentialResult> = runCredentialAction,
): Promise<{ result: CredentialResult | null; error: string }> {
	try {
		return { result: await run(action, id), error: "" };
	} catch (e) {
		return { result: null, error: e instanceof Error ? e.message : String(e) };
	} finally {
		await onchanged().catch(() => {});
	}
}

// ── Reconnect from a view (table cell / card) ──────────────────────────

/** Where a reconnect started from a cell is. */
export type ReconnectPhase = "starting" | "waiting" | "connected" | "failed";
export interface ReconnectState {
	phase: ReconnectPhase;
	/** The refusal, the credential's own error, or the timeout sentence (failed only). */
	error: string;
	/** The credential's fields as last read (waiting: for the sign-in hint). */
	fields: Record<string, ValueJSON>;
}

export interface ReconnectDeps {
	run: (action: CredentialAction, id: string) => Promise<CredentialResult>;
	follow: typeof pollWhileConnecting;
}

const liveDeps: ReconnectDeps = { run: runCredentialAction, follow: pollWhileConnecting };

/** A credential's status after a sign-in as a reconnect outcome. */
function settled(fields: Record<string, ValueJSON>, fallbackError = ""): ReconnectState {
	const status = fieldStr(fields, "status");
	if (status === "connecting") return { phase: "waiting", error: "", fields };
	if (status === "active") return { phase: "connected", error: "", fields };
	return { phase: "failed", error: fieldStr(fields, "error") || fallbackError || credentialStatusText(status), fields };
}

/**
 * The Properties pane's Connect, run from a view: ask the harness to open the
 * sign-in window, then follow the `connecting` status it writes exactly as
 * the pane does (same poll, same 5-minute give-up) until it settles.
 * `alreadyConnecting` skips the Connect and only follows a sign-in someone
 * started elsewhere. Every change calls `onchanged` so the view re-reads its
 * rows; `onstate` gets each step. Returns a stop for the poll.
 */
export function startReconnect(
	id: string,
	onstate: (state: ReconnectState) => void,
	onchanged: () => Promise<void>,
	opts: { alreadyConnecting?: boolean; deps?: ReconnectDeps } = {},
): () => void {
	const deps = opts.deps ?? liveDeps;
	let stopped = false;
	let stopPoll = () => {};
	let last: Record<string, ValueJSON> = {};
	const emit = (s: ReconnectState) => {
		if (!stopped) onstate(s);
	};
	const follow = () => {
		stopPoll = deps.follow(
			id,
			() => last,
			async (fields) => {
				last = fields;
				emit(settled(fields));
				await onchanged();
			},
			() => emit({ phase: "failed", error: CONNECT_TIMEOUT_TEXT, fields: last }),
		);
		if (stopped) stopPoll();
	};
	if (opts.alreadyConnecting) {
		emit({ phase: "waiting", error: "", fields: last });
		follow();
	} else {
		emit({ phase: "starting", error: "", fields: last });
		void performCredentialAction("connect", id, onchanged, deps.run).then(({ result, error }) => {
			if (!result) return emit({ phase: "failed", error, fields: last });
			const state = settled({ status: { stringValue: result.status ?? "" } }, result.error);
			emit(state);
			if (state.phase === "waiting") follow();
		});
	}
	return () => {
		stopped = true;
		stopPoll();
	};
}

// ── A signed-out credential named on another object's Error ────────────

/** The harness's Error badge on a task or agent whose run hit a dead login (harness credential-issues.ts). */
export const SIGNED_OUT_PREFIX = "credential signed out: ";

/** The list part of a signed-out badge ("a, b"), or null when the Error is something else. */
function signedOutList(error: string): string | null {
	if (!error.startsWith(SIGNED_OUT_PREFIX)) return null;
	return error.slice(SIGNED_OUT_PREFIX.length).replace(/ - reconnect (it|them)$/, "");
}

/** The credential names a signed-out badge lists ([] for any other Error). */
export function signedOutNames(error: string): string[] {
	return (signedOutList(error) ?? "").split(", ").map((n) => n.trim()).filter(Boolean);
}

/** Credential ids an object lists in its Credentials property (an agent's). */
export function linkedCredentialIds(fields: Record<string, ValueJSON>): string[] {
	const v = fields["credentials"];
	if (!v) return [];
	const items = v.valuesValue?.items ?? [v];
	return items.map((i) => i.linkValue?.targetId || i.stringValue || "").filter(Boolean);
}

export interface CredentialRef {
	id: string;
	name: string;
}

/**
 * The credentials a signed-out badge means, in the badge's order. The badge
 * carries names only, so a name is first looked up among the credentials the
 * object itself lists (`linked`, by id), and only otherwise among all of them.
 * A name with a comma still matches whole; one cut off by the badge's
 * 300-character limit does not match at all.
 */
export function credentialsForError(error: string, linked: CredentialRef[], all: CredentialRef[]): CredentialRef[] {
	const list = signedOutList(error);
	if (list === null) return [];
	const hay = `, ${list}, `;
	const at = (c: CredentialRef) => (c.name ? hay.indexOf(`, ${c.name}, `) : -1);
	const mine = linked.filter((c) => at(c) >= 0);
	const names = new Set(mine.map((c) => c.name));
	const ids = new Set(mine.map((c) => c.id));
	const found = [...mine, ...all.filter((c) => at(c) >= 0 && !names.has(c.name) && !ids.has(c.id))];
	// "Acme, Inc" listed is that one credential, not also "Acme" and "Inc".
	const inside = (c: CredentialRef) => found.some((d) => d.name.length > c.name.length && at(d) <= at(c) && at(c) + c.name.length <= at(d) + d.name.length);
	return found.filter((c) => !inside(c)).sort((a, b) => at(a) - at(b));
}

/**
 * What a view cell can reconnect: a credential row's Status or Error while it
 * is not connected (that credential), or another object's signed-out Error
 * (the credentials it names). Null for every other cell.
 */
export function reconnectTarget(
	row: { id: string; typeKey: string; fields: Record<string, ValueJSON> },
	key: string,
): { credentialId: string } | { error: string } | null {
	if (row.typeKey === "credential") {
		if (key !== "status" && key !== "error") return null;
		const status = fieldStr(row.fields, "status");
		if (status === "active" || (key === "error" && !fieldStr(row.fields, "error"))) return null;
		return { credentialId: row.id };
	}
	if (key !== "error") return null;
	const error = fieldStr(row.fields, "error");
	return signedOutNames(error).length > 0 ? { error } : null;
}

/**
 * A credential's Status cell: its status, or - while a reconnect started in
 * this tab runs, or after one failed and the credential is still not
 * connected - that reconnect's progress.
 */
export function credentialCellBadge(status: string, progress: ReconnectState | undefined): { text: string; icon: BadgeIcon; color: string } {
	if (progress?.phase === "starting") return { text: "Starting sign-in…", ...credentialStatusBadge("connecting") };
	if (progress?.phase === "waiting") return { text: "Waiting for sign-in…", ...credentialStatusBadge("connecting") };
	if (progress?.phase === "failed" && status !== "active") return { text: progress.error, ...credentialStatusBadge("broken") };
	return { text: credentialStatusText(status), ...credentialStatusBadge(status) };
}
