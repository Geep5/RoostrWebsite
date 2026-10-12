/**
 * A credential's harness actions, shared by its Status property and the
 * views' popover. Connect runs on whichever computer this tab is paired
 * with: the sign-in window opens there and the login is saved on the
 * credential (`cookie_*` / `key_*` properties), which the computer its
 * Served by names then keeps and checks. Check works from any device: it
 * writes `check_requested` on the credential and the computer serving it
 * runs the check and stamps `checked_at` (a tab paired with that computer
 * asks it straight). Disconnect stops a sign-in running on this computer and
 * leaves the saved login on the credential.
 */

import { fetchObject, note } from "$lib/api";
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
	/** Check may be asked for: from any device, once a computer serves the credential. */
	canCheck: boolean;
	/** This tab is paired with the computer serving it: Check asks it straight instead of through the credential. */
	checkHere: boolean;
	/** Where to go for what this tab cannot do ("" when it can do everything). */
	reason: string;
}

/**
 * What this tab may run for a credential. Check runs from anywhere (the
 * computer its Served by names answers). A website or Google sign-in
 * connects from any paired computer - the login is saved on the credential
 * and handed to the Served by computer; connecting anything else (a
 * database) needs a tab paired with that computer.
 */
export function credentialActionGate(paired: boolean, thisMachine: string, servedBy: string, computer: string, fields: Record<string, ValueJSON>): CredentialGate {
	if (!servedBy) return { canConnect: false, canCheck: false, checkHere: false, reason: "Set Served by to a computer first." };
	const linked = paired && !!thisMachine;
	if (linked && thisMachine === servedBy) return { canConnect: true, canCheck: true, checkHere: true, reason: "" };
	if (!canConnect(fields)) return { canConnect: false, canCheck: true, checkHere: false, reason: `Open Roostr on ${computer} to connect this credential.` };
	if (linked) return { canConnect: true, canCheck: true, checkHere: false, reason: "" };
	return { canConnect: false, canCheck: true, checkHere: false, reason: "Pair this tab with one of your computers to sign in there." };
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

// ── Check, from any device ─────────────────────────────────────────────

/** On a credential: when a person last pressed Check (epoch ms); the computer serving it answers by stamping `checked_at` (harness credential-objects.ts). */
export const CHECK_REQUEST_KEY = "check_requested";
const CHECK_POLL_MS = 2_500;
/** No answer this long: the computer serving the credential is taken to be off. */
export const CHECK_TIMEOUT_MS = 60_000;

function fieldNum(fields: Record<string, ValueJSON>, key: string): number {
	const v = fields[key];
	return Number(v?.intValue ?? v?.floatValue ?? 0) || 0;
}

/**
 * A Check asked for (on any device) that the computer serving the
 * credential has not answered and still may: newer than `checked_at`, under
 * a minute old. Its time, or 0. While there is one, every device says
 * "Checking on …" - the request is not hidden.
 */
export function pendingCheck(fields: Record<string, ValueJSON>, now = Date.now()): number {
	const at = fieldNum(fields, CHECK_REQUEST_KEY);
	return at > fieldNum(fields, "checked_at") && now - at < CHECK_TIMEOUT_MS ? at : 0;
}

/**
 * The computer serving a credential answered a Check asked for at
 * `requestedAt`: it stamped `checked_at` since - later than the request, or
 * at all (`before`: `checked_at` when it was asked), since the asking
 * device's clock may run ahead of that computer's.
 */
export function checkAnswered(fields: Record<string, ValueJSON>, requestedAt: number, before: number): boolean {
	const at = fieldNum(fields, "checked_at");
	return at >= requestedAt || at !== before;
}

/** Where a Check is: asked and waiting, answered (`fields` carry the answer), no answer within a minute, or the ask itself refused. */
export type CheckPhase = "checking" | "checked" | "unanswered" | "failed";
export interface CheckState {
	phase: CheckPhase;
	/** The refusal (failed only). */
	error: string;
	/** The credential's fields as last read. */
	fields: Record<string, ValueJSON>;
	/** When it was asked for (epoch ms, the asking device's clock). */
	requestedAt: number;
}

export interface CheckDeps {
	/** POST /credentials/check on the paired harness. */
	run: (action: CredentialAction, id: string) => Promise<CredentialResult>;
	/** Write the request on the credential. */
	ask: (id: string, at: number) => Promise<unknown>;
	read: (id: string) => Promise<Record<string, ValueJSON> | null>;
	every: (ms: number, tick: () => void) => () => void;
	now: () => number;
}

const liveCheckDeps: CheckDeps = {
	run: runCredentialAction,
	ask: (id, at) => note.setField(id, CHECK_REQUEST_KEY, { intValue: at }),
	read: async (id) => (await fetchObject(id).catch(() => null))?.fields ?? null,
	every: (ms, tick) => {
		const timer = setInterval(tick, ms);
		return () => clearInterval(timer);
	},
	now: () => Date.now(),
};

/**
 * Check, from any device: ask the computer serving the credential to test
 * its login now and follow until it answers. A tab paired with that
 * computer (`here`) asks it straight (POST /credentials/check); anywhere
 * else `check_requested` is written on the credential and it is re-read
 * every 2.5 s until that computer stamps `checked_at` - or, after a minute
 * with no answer, `unanswered`. `pending` follows a request already written
 * (another device's) without writing one. `fields` are the credential's as
 * the Check was pressed; every change calls `onchanged`. Returns a stop.
 */
export function startCheck(
	id: string,
	fields: Record<string, ValueJSON>,
	onstate: (state: CheckState) => void,
	onchanged: () => Promise<void>,
	opts: { here?: boolean; pending?: number; deps?: CheckDeps } = {},
): () => void {
	const deps = opts.deps ?? liveCheckDeps;
	const before = fieldNum(fields, "checked_at");
	const requestedAt = opts.pending || deps.now();
	let stopped = false;
	let stopPoll = () => {};
	const emit = (s: Omit<CheckState, "requestedAt">) => {
		if (!stopped) onstate({ ...s, requestedAt });
	};
	const finish = async (s: Omit<CheckState, "requestedAt">) => {
		emit(s);
		await onchanged().catch(() => {});
	};
	const follow = () => {
		let busy = false;
		stopPoll = deps.every(CHECK_POLL_MS, async () => {
			if (busy || stopped) return;
			if (deps.now() - requestedAt > CHECK_TIMEOUT_MS) {
				stopPoll();
				emit({ phase: "unanswered", error: "", fields });
				return;
			}
			busy = true;
			try {
				const fresh = await deps.read(id);
				if (fresh && checkAnswered(fresh, requestedAt, before)) {
					stopPoll();
					await finish({ phase: "checked", error: "", fields: fresh });
				}
			} finally {
				busy = false;
			}
		});
		if (stopped) stopPoll();
	};
	emit({ phase: "checking", error: "", fields });
	void (async () => {
		if (opts.pending) return follow();
		if (opts.here) {
			let result: CredentialResult;
			try {
				result = await deps.run("check", id);
			} catch (e) {
				return finish({ phase: "failed", error: e instanceof Error ? e.message : String(e), fields });
			}
			const answer = (await deps.read(id)) ?? { ...fields, status: { stringValue: result.status ?? "" }, error: { stringValue: result.error ?? "" } };
			return finish({ phase: "checked", error: "", fields: answer });
		}
		try {
			await deps.ask(id, requestedAt);
		} catch (e) {
			return finish({ phase: "failed", error: e instanceof Error ? e.message : String(e), fields });
		}
		await onchanged().catch(() => {});
		follow();
	})();
	return () => {
		stopped = true;
		stopPoll();
	};
}

/** What a Check says while it runs and once answered (`computer`: the one serving the credential). */
export function checkLine(state: CheckState, computer: string): { text: string; tone: "muted" | "ok" | "error" } {
	if (state.phase === "checking") return { text: `Checking on ${computer || "its computer"}…`, tone: "muted" };
	if (state.phase === "unanswered") return { text: `${computer || "Its computer"} hasn't answered - is Roostr running there?`, tone: "error" };
	if (state.phase === "failed") return { text: state.error, tone: "error" };
	const status = fieldStr(state.fields, "status") || "missing";
	const error = fieldStr(state.fields, "error");
	if (status === "active") return { text: "Connected", tone: "ok" };
	if (status === "connecting") return { text: error || credentialStatusText(status), tone: "muted" };
	return { text: error || credentialStatusText(status), tone: "error" };
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
 * A credential's Status cell: its status, or - while a Check is waiting for
 * its answer, a reconnect started in this tab runs, or after one failed and
 * the credential is still not connected - that progress.
 */
export function credentialCellBadge(status: string, progress: ReconnectState | undefined, checking = false): { text: string; icon: BadgeIcon; color: string } {
	if (checking) return { text: "Checking…", ...credentialStatusBadge("connecting") };
	if (progress?.phase === "starting") return { text: "Starting sign-in…", ...credentialStatusBadge("connecting") };
	if (progress?.phase === "waiting") return { text: "Waiting for sign-in…", ...credentialStatusBadge("connecting") };
	if (progress?.phase === "failed" && status !== "active") return { text: progress.error, ...credentialStatusBadge("broken") };
	return { text: credentialStatusText(status), ...credentialStatusBadge(status) };
}
