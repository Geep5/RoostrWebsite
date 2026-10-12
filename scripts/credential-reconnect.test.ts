import { describe, expect, test } from "bun:test";
import {
	CONNECT_TIMEOUT_TEXT,
	credentialActionGate,
	credentialCellBadge,
	credentialsForError,
	linkedCredentialIds,
	reconnectTarget,
	signedOutNames,
	startReconnect,
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
	test("only the tab paired with the serving computer acts", () => {
		expect(credentialActionGate(true, "m1", "m1", "Mac").canAct).toBe(true);
		expect(credentialActionGate(true, "m2", "m1", "Mac")).toEqual({ canAct: false, reason: "Open Roostr on Mac to connect or check this credential." });
		expect(credentialActionGate(false, "", "m1", "Mac").canAct).toBe(false);
		expect(credentialActionGate(true, "m1", "", "").reason).toBe("Set Served by to a computer first.");
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
