import { describe, expect, test } from "bun:test";
import {
	CHECK_REQUEST_KEY,
	CHECK_TIMEOUT_MS,
	CONNECT_TIMEOUT_TEXT,
	checkAnswered,
	checkLine,
	connectingHint,
	credentialActionGate,
	credentialCellBadge,
	credentialsForError,
	linkedCredentialIds,
	isSecretField,
	pendingCheck,
	reconnectTarget,
	signedOutNames,
	startCheck,
	startReconnect,
	type CheckDeps,
	type CheckState,
	type CredentialAction,
	type CredentialResult,
	type ReconnectDeps,
	type ReconnectState,
} from "../src/lib/credential-actions";
import type { ValueJSON } from "../src/lib/types";

const s = (stringValue: string): ValueJSON => ({ stringValue });
const badge = (names: string[]) => `credential signed out: ${names.join(", ")} - reconnect ${names.length === 1 ? "it" : "them"}`;

describe("signed-out Error → credential", () => {
	test("reads the names the harness badge lists", () => {
		expect(signedOutNames(badge(["X (Twitter)"]))).toEqual(["X (Twitter)"]);
		expect(signedOutNames(badge(["Gmail", "LinkedIn"]))).toEqual(["Gmail", "LinkedIn"]);
		expect(signedOutNames("run failed: timeout")).toEqual([]);
		expect(signedOutNames("")).toEqual([]);
	});

	test("prefers the credentials the object lists over a same-named one elsewhere", () => {
		const linked = [{ id: "mine", name: "Gmail" }];
		const all = [{ id: "other", name: "Gmail" }, { id: "mine", name: "Gmail" }, { id: "li", name: "LinkedIn" }];
		expect(credentialsForError(badge(["LinkedIn", "Gmail"]), linked, all).map((c) => c.id)).toEqual(["li", "mine"]);
	});

	test("falls back to every credential with the name when the object lists none", () => {
		const all = [{ id: "a", name: "Gmail" }, { id: "b", name: "Gmail" }, { id: "c", name: "Slack" }];
		expect(credentialsForError(badge(["Gmail"]), [], all).map((c) => c.id)).toEqual(["a", "b"]);
	});

	test("a name with a comma matches whole; partial names and other errors do not", () => {
		const all = [{ id: "acme", name: "Acme, Inc" }, { id: "acm", name: "Acme" }, { id: "inc", name: "Inc" }];
		expect(credentialsForError(badge(["Acme, Inc"]), [], all).map((c) => c.id)).toEqual(["acme"]);
		expect(credentialsForError(badge(["Acme, Inc"]), [{ id: "acme", name: "Acme, Inc" }], all).map((c) => c.id)).toContain("acme");
		// Cut off by the 300-character limit: no " - reconnect" tail, last name partial.
		expect(credentialsForError("credential signed out: Gmail, Linked", [], [{ id: "g", name: "Gmail" }, { id: "l", name: "LinkedIn" }]).map((c) => c.id)).toEqual(["g"]);
		expect(credentialsForError("Gmail is down", [], [{ id: "g", name: "Gmail" }])).toEqual([]);
	});

	test("reads an agent's Credentials links and plain ids", () => {
		expect(linkedCredentialIds({ credentials: { valuesValue: { items: [{ linkValue: { targetId: "c1", relationKey: "credentials" } }, s("c2")] } } })).toEqual(["c1", "c2"]);
		expect(linkedCredentialIds({ credentials: s("c3") })).toEqual(["c3"]);
		expect(linkedCredentialIds({})).toEqual([]);
	});

	test("which cells reconnect", () => {
		const cred = (status: string, error = "") => ({ id: "c", typeKey: "credential", fields: { status: s(status), ...(error ? { error: s(error) } : {}) } });
		expect(reconnectTarget(cred("needs_auth"), "status")).toEqual({ credentialId: "c" });
		expect(reconnectTarget(cred("needs_auth", "session expired"), "error")).toEqual({ credentialId: "c" });
		expect(reconnectTarget(cred("needs_auth"), "error")).toBeNull();
		expect(reconnectTarget(cred("active"), "status")).toBeNull();
		expect(reconnectTarget(cred("needs_auth"), "name")).toBeNull();
		const task = (error: string) => ({ id: "t", typeKey: "task", fields: { error: s(error) } });
		expect(reconnectTarget(task(badge(["Gmail"])), "error")).toEqual({ error: badge(["Gmail"]) });
		expect(reconnectTarget(task("run failed"), "error")).toBeNull();
		expect(reconnectTarget(task(badge(["Gmail"])), "status")).toBeNull();
	});
});

describe("action gate", () => {
	const website = { login_url: s("https://x.com/login"), session_host: s("x.com"), session_cookie: s("auth_token") };
	const google = { service: s("google-account"), account: s("me@example.com") };
	const database = { service: s("postgres"), key_url: s("postgres://x") };

	test("the tab paired with the serving computer does everything, and checks straight", () => {
		for (const fields of [website, google, database]) expect(credentialActionGate(true, "m1", "m1", "Mac", fields)).toEqual({ canConnect: true, canCheck: true, checkHere: true, reason: "" });
	});

	test("a website or Google sign-in connects from any paired computer; Check is asked of the serving one", () => {
		for (const fields of [website, google]) expect(credentialActionGate(true, "m2", "m1", "Mac", fields)).toEqual({ canConnect: true, canCheck: true, checkHere: false, reason: "" });
	});

	test("anything else connects only on the serving computer, and checks from anywhere", () => {
		expect(credentialActionGate(true, "m2", "m1", "Mac", database)).toEqual({ canConnect: false, canCheck: true, checkHere: false, reason: "Open Roostr on Mac to connect this credential." });
	});

	test("an unpaired tab (the website, iOS) still checks; no Served by, nobody acts", () => {
		expect(credentialActionGate(false, "", "m1", "Mac", website)).toEqual({ canConnect: false, canCheck: true, checkHere: false, reason: "Pair this tab with one of your computers to sign in there." });
		for (const fields of [google, database]) expect(credentialActionGate(false, "", "m1", "Mac", fields).canCheck).toBe(true);
		expect(credentialActionGate(false, "", "m1", "Mac", database).reason).toBe("Open Roostr on Mac to connect this credential.");
		expect(credentialActionGate(true, "m1", "", "", website)).toEqual({ canConnect: false, canCheck: false, checkHere: false, reason: "Set Served by to a computer first." });
	});

	test("the sign-in hint names where the window opened and who gets the login", () => {
		expect(connectingHint(website, "this computer", "Sharky's Mac Studio")).toBe("Sign in in the Chrome window on this computer; Roostr then hands the login to Sharky's Mac Studio.");
		expect(connectingHint(website, "this computer", "")).toBe("Sign in in the Chrome window on this computer.");
		expect(connectingHint(google, "this computer", "Mac")).toBe("Sign in to me@example.com in the browser window gws opens on this computer; Roostr then hands the login to Mac.");
	});
});

describe("secret rows", () => {
	test("pasted keys and saved cookie values are masked; cookie details and the legacy key list are not", () => {
		for (const key of ["key_api_key", "key_refresh_token", "cookie_auth_token", "cookie_meta_2"]) expect(isSecretField(key)).toBe(true);
		for (const key of ["key_fields", "key_", "cookie_meta", "cookie_", "session_cookie", "status"]) expect(isSecretField(key)).toBe(false);
	});
});

/** Fakes for the harness Connect and the status poll; drive the poll by hand. */
function fakes(run: (action: CredentialAction, id: string) => Promise<CredentialResult>) {
	const calls: string[] = [];
	const poll: { current?: () => Record<string, ValueJSON>; changed?: (f: Record<string, ValueJSON>) => Promise<void>; timeout?: () => void; stopped: boolean } = { stopped: false };
	const deps: ReconnectDeps = {
		run: async (action, id) => {
			calls.push(`${action}:${id}`);
			return run(action, id);
		},
		follow: (_id, current, onchanged, ontimeout) => {
			poll.current = current;
			poll.changed = onchanged;
			poll.timeout = ontimeout;
			return () => (poll.stopped = true);
		},
	};
	const states: ReconnectState[] = [];
	// The step after "starting" is the Connect's answer; tests await it instead of a timer.
	const answered = Promise.withResolvers<void>();
	let reloads = 0;
	const onchanged = async () => {
		reloads++;
	};
	const onstate = (st: ReconnectState) => {
		states.push(st);
		if (states.length === 2) answered.resolve();
	};
	return { deps, calls, poll, states, onstate, onchanged, answered: answered.promise, reloads: () => reloads };
}

describe("startReconnect", () => {
	test("starting → waiting → connected, re-reading the view on every change", async () => {
		const f = fakes(async () => ({ status: "connecting" }));
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		expect(f.states.map((x) => x.phase)).toEqual(["starting"]);
		await f.answered;
		expect(f.calls).toEqual(["connect:c1"]);
		expect(f.states.map((x) => x.phase)).toEqual(["starting", "waiting"]);
		expect(f.reloads()).toBe(1);
		await f.poll.changed!({ status: s("connecting"), checked_at: s("1") });
		await f.poll.changed!({ status: s("active") });
		expect(f.states.map((x) => x.phase)).toEqual(["starting", "waiting", "waiting", "connected"]);
		expect(f.reloads()).toBe(3);
		expect(f.poll.current!()).toEqual({ status: s("active") });
	});

	test("a refusal fails with the harness's sentence and still re-reads", async () => {
		const f = fakes(async () => {
			throw new Error("This credential is looked after by Studio Mac.");
		});
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		await f.answered;
		expect(f.states.at(-1)).toEqual({ phase: "failed", error: "This credential is looked after by Studio Mac.", fields: {} });
		expect(f.reloads()).toBe(1);
		expect(f.poll.changed).toBeUndefined();
	});

	test("a sign-in that ends signed out fails with the credential's error", async () => {
		const f = fakes(async () => ({ status: "connecting" }));
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		await f.answered;
		await f.poll.changed!({ status: s("needs_auth"), error: s("Window closed before sign-in") });
		expect(f.states.at(-1)?.phase).toBe("failed");
		expect(f.states.at(-1)?.error).toBe("Window closed before sign-in");
	});

	test("the poll giving up fails with the pane's timeout sentence", async () => {
		const f = fakes(async () => ({ status: "connecting" }));
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		await f.answered;
		f.poll.timeout!();
		expect(f.states.at(-1)?.error).toBe(CONNECT_TIMEOUT_TEXT);
	});

	test("a connect that answers active at once (database, Google already signed in) connects without polling", async () => {
		const f = fakes(async () => ({ status: "active" }));
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		await f.answered;
		expect(f.states.map((x) => x.phase)).toEqual(["starting", "connected"]);
		expect(f.poll.changed).toBeUndefined();
	});

	test("following a sign-in started elsewhere does not connect again", async () => {
		const f = fakes(async () => ({ status: "connecting" }));
		startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps, alreadyConnecting: true });
		expect(f.calls).toEqual([]);
		expect(f.states.map((x) => x.phase)).toEqual(["waiting"]);
		await f.poll.changed!({ status: s("active") });
		expect(f.states.at(-1)?.phase).toBe("connected");
	});

	test("stop ends the poll and silences later steps", async () => {
		const f = fakes(async () => ({ status: "connecting" }));
		const stop = startReconnect("c1", f.onstate, f.onchanged, { deps: f.deps });
		await f.answered;
		stop();
		expect(f.poll.stopped).toBe(true);
		const before = f.states.length;
		await f.poll.changed!({ status: s("active") });
		expect(f.states.length).toBe(before);
	});
});

describe("cell badge", () => {
	test("shows progress over the status, and a failure only while still not connected", () => {
		expect(credentialCellBadge("needs_auth", undefined).text).toBe("Signed out - reconnect");
		expect(credentialCellBadge("needs_auth", { phase: "starting", error: "", fields: {} }).text).toBe("Starting sign-in…");
		expect(credentialCellBadge("connecting", { phase: "waiting", error: "", fields: {} }).text).toBe("Waiting for sign-in…");
		const failed = { phase: "failed" as const, error: "Window closed", fields: {} };
		expect(credentialCellBadge("needs_auth", failed)).toEqual({ text: "Window closed", icon: "x", color: "red" });
		expect(credentialCellBadge("active", failed).text).toBe("Connected");
	});
});

/** Fakes for Check: the harness POST, the request write, reads and a hand-driven clock and poll. */
function checkFakes(answers: Array<Record<string, ValueJSON> | null>, opts: { run?: () => Promise<CredentialResult>; ask?: () => Promise<unknown> } = {}) {
	const calls: string[] = [];
	const clock = { now: 1_000_000 };
	let tick: (() => void | Promise<void>) | null = null;
	let polling = false;
	const deps: CheckDeps = {
		run: async (action, id) => {
			calls.push(`${action}:${id}`);
			return opts.run ? opts.run() : { status: "active" };
		},
		ask: async (id, at) => {
			calls.push(`ask:${id}:${at}`);
			return opts.ask?.();
		},
		read: async () => answers.shift() ?? null,
		every: (_ms, fn) => {
			tick = fn;
			polling = true;
			return () => (polling = false);
		},
		now: () => clock.now,
	};
	const states: CheckState[] = [];
	let reloads = 0;
	const settle = () => new Promise((r) => setTimeout(r, 0));
	return {
		deps,
		calls,
		clock,
		states,
		phases: () => states.map((x) => x.phase),
		polling: () => polling,
		reloads: () => reloads,
		onstate: (st: CheckState) => void states.push(st),
		onchanged: async () => void reloads++,
		settle,
		/** One poll tick, after `ms` more milliseconds. */
		tick: async (ms = 2_500) => {
			clock.now += ms;
			await tick?.();
			await settle();
		},
	};
}

const i = (intValue: number): ValueJSON => ({ intValue });

describe("Check from any device", () => {
	const before = { status: s("connecting"), checked_at: i(900_000) };

	test("writes the request, says Checking, and settles when checked_at moves past it", async () => {
		const f = checkFakes([{ ...before }, { status: s("active"), checked_at: i(1_002_000) }]);
		startCheck("c1", before, f.onstate, f.onchanged, { deps: f.deps });
		expect(f.phases()).toEqual(["checking"]);
		await f.settle();
		expect(f.calls).toEqual(["ask:c1:1000000"]);
		await f.tick();
		expect(f.phases()).toEqual(["checking"]);
		await f.tick();
		expect(f.phases()).toEqual(["checking", "checked"]);
		expect(f.polling()).toBe(false);
		expect(checkLine(f.states.at(-1)!, "Mac")).toEqual({ text: "Connected", tone: "ok" });
		expect(f.reloads()).toBe(2);
	});

	test("the answer is the credential's error when the login is gone", async () => {
		const f = checkFakes([{ status: s("needs_auth"), error: s("The X sign-in expired"), checked_at: i(1_001_000) }]);
		startCheck("c1", before, f.onstate, f.onchanged, { deps: f.deps });
		await f.settle();
		await f.tick();
		expect(checkLine(f.states.at(-1)!, "Mac")).toEqual({ text: "The X sign-in expired", tone: "error" });
	});

	test("an answer stamped by a computer whose clock runs behind still counts", async () => {
		const f = checkFakes([{ status: s("active"), checked_at: i(950_000) }]);
		startCheck("c1", before, f.onstate, f.onchanged, { deps: f.deps });
		await f.settle();
		await f.tick();
		expect(f.phases().at(-1)).toBe("checked");
	});

	test("no answer within a minute: the serving computer hasn't answered", async () => {
		const f = checkFakes(Array.from({ length: 30 }, () => ({ ...before })));
		startCheck("c1", before, f.onstate, f.onchanged, { deps: f.deps });
		await f.settle();
		expect(checkLine(f.states[0], "Mac")).toEqual({ text: "Checking on Mac…", tone: "muted" });
		for (let n = 0; n < 24; n++) await f.tick();
		expect(f.phases().at(-1)).toBe("checking");
		await f.tick();
		expect(f.phases().at(-1)).toBe("unanswered");
		expect(f.polling()).toBe(false);
		expect(checkLine(f.states.at(-1)!, "Mac").text).toBe("Mac hasn't answered - is Roostr running there?");
	});

	test("a tab paired with the serving computer asks it straight, writing no request", async () => {
		const f = checkFakes([{ status: s("active"), checked_at: i(1_000_500) }]);
		startCheck("c1", before, f.onstate, f.onchanged, { deps: f.deps, here: true });
		await f.settle();
		expect(f.calls).toEqual(["check:c1"]);
		expect(f.phases()).toEqual(["checking", "checked"]);
		expect(f.polling()).toBe(false);
	});

	test("a refusal or failed write fails with its sentence", async () => {
		const refused = checkFakes([], { run: async () => { throw new Error("This credential is looked after by Mac."); } });
		startCheck("c1", before, refused.onstate, refused.onchanged, { deps: refused.deps, here: true });
		await refused.settle();
		expect(refused.states.at(-1)).toMatchObject({ phase: "failed", error: "This credential is looked after by Mac." });
		const offline = checkFakes([], { ask: async () => { throw new Error("offline"); } });
		startCheck("c1", before, offline.onstate, offline.onchanged, { deps: offline.deps });
		await offline.settle();
		expect(offline.states.at(-1)).toMatchObject({ phase: "failed", error: "offline" });
		expect(offline.polling()).toBe(false);
	});

	test("following another device's request writes nothing and waits for the same answer", async () => {
		const asked = { ...before, [CHECK_REQUEST_KEY]: i(999_000) };
		const f = checkFakes([{ status: s("active"), checked_at: i(1_001_000) }]);
		startCheck("c1", asked, f.onstate, f.onchanged, { deps: f.deps, pending: pendingCheck(asked, 1_000_000) });
		await f.settle();
		expect(f.calls).toEqual([]);
		expect(f.states[0].requestedAt).toBe(999_000);
		await f.tick();
		expect(f.phases()).toEqual(["checking", "checked"]);
	});

	test("a request is pending until answered, and for a minute at most", () => {
		const asked = { checked_at: i(900_000), [CHECK_REQUEST_KEY]: i(1_000_000) };
		expect(pendingCheck(asked, 1_010_000)).toBe(1_000_000);
		expect(pendingCheck(asked, 1_000_000 + CHECK_TIMEOUT_MS + 1)).toBe(0);
		expect(pendingCheck({ ...asked, checked_at: i(1_000_100) }, 1_010_000)).toBe(0);
		expect(pendingCheck({ checked_at: i(900_000) }, 1_010_000)).toBe(0);
		expect(checkAnswered({ checked_at: i(900_000) }, 1_000_000, 900_000)).toBe(false);
	});

	test("the cell says Checking… over any other progress", () => {
		expect(credentialCellBadge("connecting", { phase: "waiting", error: "", fields: {} }, true).text).toBe("Checking…");
		expect(credentialCellBadge("needs_auth", undefined, false).text).toBe("Signed out - reconnect");
	});
});
