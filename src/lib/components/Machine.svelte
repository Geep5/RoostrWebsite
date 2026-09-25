<script lang="ts">
	// ── This machine ────────────────────────────────────────────────
	//
	// The machine-scoped panel: holdups (agents that needed a capability
	// and couldn't proceed) and integrations (device capabilities like
	// browserless and gws). Account-scoped things stay in Settings -
	// this surface describes the box the harness runs on.
	//
	// Everything the panel SHOWS about capabilities is DAG data:
	// descriptor cards say what each skill or login IS, installation
	// objects say what is TRUE on a machine. Only execution (install,
	// login, save, approve) and the machine's private ledger (holdup
	// counts, live job phases, job logs) go through the paired harness.
	import { onMount } from "svelte";
	import { fetchAllQuery, fetchObject, mailbox, note, type QueryResultRow } from "$lib/api";
	import Machines from "./Machines.svelte";
	import { goto } from "$app/navigation";
	import { harnessFetch, pairedSession, onPairingChange } from "$lib/local-transport";
	import { loadCards, type Card } from "$lib/cards";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	let { onclose }: { onclose: () => void } = $props();

	let paired = $state(pairedSession() !== null);
	let harnessError = $state("");

	// ── DAG state: cards (what a thing IS) + installs (what is TRUE here) ──
	let cards = $state<Card[]>([]);
	let installs = $state<QueryResultRow[]>([]);
	/** This machine's stable id, from the paired harness ("" unpaired). */
	let machineId = $state("");
	/** Catalog skill prompt bodies, by skill key. */
	let skillBodies = $state<Record<string, { id: string; text: string }>>({});
	let dagError = $state("");

	// ── The machine's private ledger (paired harness only) ──
	interface Holdup {
		id: string;
		capability: string;
		agentId: string;
		agentName: string;
		objectId: string;
		objectName: string;
		error: string;
		count: number;
		firstAt: number;
		updatedAt: number;
	}
	let holdups = $state<Holdup[]>([]);
	let jobPhases = $state<Record<string, string>>({});
	let jobLogs = $state<Record<string, string>>({});

	interface CapabilityRequest {
		objectId: string;
		messageId: string;
		key: string;
		account: string;
		operation: string;
		sender: { objectId: string; agentId: string };
		status: string;
		error: string;
		canApprove: boolean;
		fields?: Array<{ key: string; label: string; secret: boolean }>;
	}
	let capabilityRequests = $state<CapabilityRequest[]>([]);
	let requestError = $state("");
	let requestNotice = $state("");
	let requestBusy = $state("");
	let requestPoll: ReturnType<typeof setInterval> | undefined;
	let statePoll: ReturnType<typeof setInterval> | undefined;
	let credRemoveConfirm = $state("");
	let credError = $state("");
	let credBusy = $state(false);
	let credSetupFor = $state("");
	let credDraft = $state<Record<string, string>>({});
	let googleAccountDraft = $state("");
	let googleAccountBusy = $state(false);
	let googleRemoveConfirm = $state("");
	let skillPromptDraft = $state<Record<string, string>>({});
	let skillPromptSaved = $state<string>("");
	let skillOpen = $state<string>("");
	let skillConfirm = $state<string>("");
	let skillResetConfirm = $state<string>("");

	const integrations = $derived(cards.filter((c) => c.kind === "integration"));
	const skillCards = $derived(cards.filter((c) => c.kind === "skill"));
	const googleRows = $derived(
		installs.filter((r) => fieldStr(r.fields, "key") === "google" && fieldStr(r.fields, "account") !== "" && fieldStr(r.fields, "machine_id") === machineId),
	);

	/** This machine's install row for a capability key (+ account). */
	function installFor(key: string, account = ""): QueryResultRow | undefined {
		return installs.find((r) => fieldStr(r.fields, "key") === key && fieldStr(r.fields, "machine_id") === machineId && fieldStr(r.fields, "account") === account);
	}

	/** Chip state for a skill card: live job phase wins, then the install row. */
	function skillPhase(key: string): { phase: string; installed: boolean; error: string } {
		const job = jobPhases[key];
		if (job) return { phase: job, installed: true, error: "" };
		const row = installFor(key);
		if (!row) return { phase: "off", installed: false, error: "" };
		const status = fieldStr(row.fields, "status");
		const phase =
			status === "active" ? "on"
			: status === "needs_auth" ? "needs-auth"
			: status === "broken" ? "failed"
			: status === "processing" || status === "needs_approval" ? "installing"
			: "off";
		return { phase, installed: status !== "" && status !== "missing", error: fieldStr(row.fields, "error") };
	}

	/** Auth state of a login from its install row: the auth method is the badge. */
	function loginBadges(key: string): { password: boolean; browser: boolean; status: string; error: string } {
		const row = installFor(key);
		if (!row) return { password: false, browser: false, status: "", error: "" };
		const auth = fieldStr(row.fields, "auth");
		return {
			password: auth === "api_key",
			browser: auth === "browser_profile" || auth === "oauth",
			status: fieldStr(row.fields, "status"),
			error: fieldStr(row.fields, "error"),
		};
	}

	/** Plain-text body of a skill object (the prompt agents read). */
	function objectText(obj: ObjectJSON): string {
		const byId = new Map(obj.blocks.map((b) => [b.id, b]));
		const referenced = new Set<string>();
		for (const b of obj.blocks) for (const c of b.childrenIds) referenced.add(c);
		const roots = obj.blocks.filter((b) => !referenced.has(b.id) && b.id !== "__discussion__");
		const out: string[] = [];
		const walk = (id: string) => {
			const b = byId.get(id);
			if (!b) return;
			const kind = b.content.custom?.contentType;
			if (kind === "chat" || kind === "discussion") return;
			const line = b.content.text?.text ?? "";
			if (line) out.push(line);
			for (const c of b.childrenIds) walk(c);
		};
		for (const r of roots) walk(r.id);
		return out.join("\n");
	}

	async function loadDag(forceCards = false) {
		try {
			const [nextCards, nextInstalls, skillRows] = await Promise.all([
				loadCards(forceCards),
				fetchAllQuery({ type: "install" }),
				fetchAllQuery({ type: "skill" }),
			]);
			cards = nextCards;
			installs = nextInstalls;
			const bodies: Record<string, { id: string; text: string }> = {};
			await Promise.all(
				skillRows
					.filter((r) => fieldStr(r.fields, "scope") === "global")
					.map(async (r) => {
						const obj = await fetchObject(r.id);
						bodies[(fieldStr(r.fields, "name") || r.id).toLowerCase()] = { id: r.id, text: objectText(obj) };
					}),
			);
			skillBodies = bodies;
			dagError = "";
		} catch (error) {
			dagError = error instanceof Error ? error.message : "Cannot load capability data.";
		}
	}

	async function loadMachineState() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/machine-state");
			if (!res.ok) throw new Error(`Cannot load machine state (HTTP ${res.status}).`);
			const out = (await res.json()) as { holdups?: Holdup[]; phases?: Record<string, string>; logs?: Record<string, string> };
			if (!pairedSession()) return;
			holdups = (out.holdups ?? []).sort((a, b) => b.updatedAt - a.updatedAt);
			jobPhases = out.phases ?? {};
			jobLogs = out.logs ?? {};
			harnessError = "";
			const busy = Object.keys(jobPhases).length > 0;
			if (busy && !statePoll) statePoll = setInterval(() => void loadMachineState(), 2000);
			if (!busy && statePoll) {
				clearInterval(statePoll);
				statePoll = undefined;
			}
		} catch (error) {
			harnessError = error instanceof Error ? error.message : "The paired harness is unreachable.";
			if (statePoll) clearInterval(statePoll);
			statePoll = undefined;
		}
	}

	async function loadCapabilityRequests() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/capability-requests");
			if (!res.ok) throw new Error(`Cannot load approvals (HTTP ${res.status}).`);
			const previous = capabilityRequests;
			capabilityRequests = ((await res.json()) as { requests: CapabilityRequest[] }).requests;
			if (previous.some((request) => request.status === "processing" && !capabilityRequests.some((next) => next.messageId === request.messageId && next.status === "processing"))) {
				await Promise.all([loadDag(), loadMachineState()]);
				window.dispatchEvent(new Event("roostr:machines-changed"));
			}
		} catch (error) {
			requestError = error instanceof Error ? error.message : "Cannot load approvals.";
		}
	}

	/** The DAG carries only the intent. A separate paired click grants execution. */
	async function requestOperation(key: string, operation: string, account = ""): Promise<boolean> {
		if (requestBusy) return false;
		requestBusy = `stage:${key}:${operation}`;
		requestError = "";
		requestNotice = "";
		try {
			const res = await harnessFetch("/machine");
			if (!res.ok) throw new Error("Cannot identify the owning machine.");
			const machine = (await res.json()) as { id: string };
			const [installRows, machines] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "machine" })]);
			let installationId = installRows.find((row) => row.fields["key"]?.stringValue === key && row.fields["machine_id"]?.stringValue === machine.id && (row.fields["account"]?.stringValue ?? "") === account)?.id;
			if (!installationId && key === "google" && account) {
				installationId = (await note.create(`Google ${account}`, "install", { key: { stringValue: key }, machine_id: { stringValue: machine.id }, account: { stringValue: account }, status: { stringValue: "missing" } })).id;
			}
			if (!installationId) throw new Error("This machine has not published the installation object yet.");
			const source = machines.find((row) => row.fields["machine_id"]?.stringValue === machine.id);
			if (!source) throw new Error("This machine has not published its object yet.");
			if (capabilityRequests.some((request) => request.objectId === installationId && request.operation === operation && ["pending", "awaiting_approval", "processing"].includes(request.status))) {
				requestNotice = "This request is already waiting below.";
				return true;
			}
			await mailbox.send({ id: crypto.randomUUID(), exchangeId: crypto.randomUUID(), sender: { objectId: source.id, agentId: "" }, recipients: [{ objectId: installationId, agentId: "" }], text: `Request ${operation} for ${key}${account ? ` (${account})` : ""}.`, replyTo: "", sentAt: Date.now(), title: "Capability request", requestReply: true, historical: false, operation, author: "" });
			requestNotice = "Request sent to its installation object. Approve it below when it arrives on this machine.";
			await loadCapabilityRequests();
			return true;
		} catch (error) {
			requestError = error instanceof Error ? error.message : "Cannot stage capability request.";
			return false;
		} finally {
			requestBusy = "";
		}
	}

	async function resolveRequest(request: CapabilityRequest, action: "approve" | "reject" | "finish-login") {
		requestBusy = request.messageId;
		requestError = "";
		try {
			const body: { objectId: string; messageId: string; fields?: Record<string, string> } = { objectId: request.objectId, messageId: request.messageId };
			if (action === "approve" && request.operation === "auth.save") {
				body.fields = Object.fromEntries((request.fields ?? []).map((field) => [field.key, credDraft[`${request.key}:${field.key}`] ?? ""]));
			}
			const res = await harnessFetch(`/capability-requests/${action}`, { method: "POST", body: JSON.stringify(body) });
			const result = (await res.json()) as { error?: string; active?: boolean };
			if (!res.ok || result.error) throw new Error(result.error ?? `HTTP ${res.status}`);
			if (action === "finish-login" && !result.active) requestNotice = "Authentication is not confirmed yet. Finish signing in on this machine, then check again.";
			else requestNotice = "";
			if (body.fields) for (const field of request.fields ?? []) delete credDraft[`${request.key}:${field.key}`];
			await Promise.all([loadCapabilityRequests(), loadDag(), loadMachineState()]);
			window.dispatchEvent(new Event("roostr:machines-changed"));
		} catch (error) {
			requestError = error instanceof Error ? error.message : "Approval failed.";
		} finally {
			requestBusy = "";
		}
	}

	async function savePasswordCredential(key: string) {
		credBusy = true;
		try {
			if (await requestOperation(key, "auth.save")) credSetupFor = "";
		} finally {
			credBusy = false;
		}
	}

	async function addGoogleAccount() {
		const account = googleAccountDraft.trim().toLowerCase();
		if (!account) return;
		googleAccountBusy = true;
		try {
			if (await requestOperation("google", "auth.login", account)) googleAccountDraft = "";
		} finally {
			googleAccountBusy = false;
		}
	}

	async function removeGoogleAccount(account: string) {
		googleAccountBusy = true;
		try {
			if (await requestOperation("google", "auth.revoke", account)) googleRemoveConfirm = "";
		} finally {
			googleAccountBusy = false;
		}
	}

	async function clearHoldup(id: string) {
		await changeSkill("/skills/holdup-clear", { id });
	}

	function ago(ts: number): string {
		const m = Math.max(0, Math.round((Date.now() - ts) / 60000));
		if (m < 1) return "just now";
		if (m < 60) return `${m}m ago`;
		const h = Math.round(m / 60);
		if (h < 48) return `${h}h ago`;
		return `${Math.round(h / 24)}d ago`;
	}

	// ── Unassigned skills ───────────────────────────────────────────
	//
	// A hand-written skill belongs to one agent, set in that agent's
	// prompt panel - these are the ones with no owner yet, so every agent
	// still lists them.
	interface GlobalSkill {
		id: string;
		name: string;
		description: string;
	}
	let globalSkills = $state<GlobalSkill[]>([]);

	async function loadGlobalSkills() {
		const records = await fetchAllQuery({ type: "skill" });
		globalSkills = records
			.filter((r) => !(r.fields["agent"]?.stringValue ?? "") && r.fields["scope"]?.stringValue !== "global")
			.map((r) => ({
				id: r.id,
				name: r.fields["name"]?.stringValue || "Untitled",
				description: r.fields["description"]?.stringValue ?? "",
			}));
	}

	async function resetSkillPrompt(key: string) {
		skillResetConfirm = "";
		if (await changeSkill("/skills/prompt-reset", { key })) delete skillPromptDraft[key];
	}

	async function saveSkillPrompt(key: string) {
		const text = (skillPromptDraft[key] ?? "").trim();
		if (await changeSkill("/skills/prompt", { key, text })) {
			skillPromptSaved = key;
			setTimeout(() => (skillPromptSaved = ""), 1500);
		}
	}

	async function skillOp(key: string, op: "enable" | "disable" | "recheck" | "uninstall") {
		skillConfirm = "";
		const operation = op === "recheck" ? "auth.check" : op === "enable" && !skillPhase(key).installed ? "skill.install" : `skill.${op}`;
		await requestOperation(key, operation);
	}

	async function changeSkill(path: string, body: Record<string, string>): Promise<boolean> {
		try {
			const res = await harnessFetch(path, {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify(body),
			});
			if (!res.ok) throw new Error(`Harness operation failed (HTTP ${res.status}).`);
			await Promise.all([loadDag(), loadMachineState()]);
			return true;
		} catch (error) {
			harnessError = error instanceof Error ? error.message : "The paired harness is unreachable.";
			return false;
		}
	}

	function refreshPairing() {
		paired = !!pairedSession();
		if (!paired) {
			machineId = "";
			holdups = [];
			jobPhases = {};
			jobLogs = {};
			harnessError = "";
			capabilityRequests = [];
			credDraft = {};
			if (requestPoll) clearInterval(requestPoll);
			requestPoll = undefined;
			if (statePoll) clearInterval(statePoll);
			statePoll = undefined;
			return;
		}
		void (async () => {
			try {
				const res = await harnessFetch("/machine");
				if (res.ok) machineId = ((await res.json()) as { id: string }).id;
			} catch {
				/* harness down: DAG rows still render, actions say so */
			}
			void loadMachineState();
			void loadCapabilityRequests();
		})();
		if (!requestPoll) requestPoll = setInterval(() => void loadCapabilityRequests(), 2000);
	}

	onMount(() => {
		refreshPairing();
		void loadDag();
		// Statuses flip as the owning machine publishes; cards change only
		// when a catalog is republished, so they stay cached.
		const dagPoll = setInterval(() => void loadDag(), 10_000);
		void loadGlobalSkills().catch((error) => {
			dagError = error instanceof Error ? error.message : "Cannot load saved skills.";
		});
		const unsubscribe = onPairingChange(refreshPairing);
		return () => {
			unsubscribe();
			clearInterval(dagPoll);
			if (statePoll) clearInterval(statePoll);
			if (requestPoll) clearInterval(requestPoll);
		};
	});
</script>

<div class="overlay" role="presentation" onclick={(e) => { if (e.target === e.currentTarget) onclose(); }}>
	<div class="modal" role="dialog" aria-label="This machine">
		<header>
			<h2><span class="cog">🖥️</span> This machine</h2>
			<button class="x" onclick={onclose}>×</button>
		</header>
		{#if !paired}
			<PairGate compact onready={refreshPairing} />
			<p class="hint">Pair with your native app to manage this machine. Saved skills remain available in browser mode.</p>
		{/if}
		{#if harnessError}<p class="hint" role="alert">{harnessError} Check that the paired native app and harness are running.</p>{/if}
		{#if dagError}<p class="hint" role="alert">{dagError}</p>{/if}

		<section>
			<h3>Holdups</h3>
			{#if !paired}
				<p class="hint">Pair to view machine holdups.</p>
			{:else if holdups.length === 0}
				<p class="hint">Nothing held up — no agent has been blocked on a machine capability.</p>
			{:else}
				<p class="hint">Agents that needed a capability and couldn't proceed. Fix the integration below, then clear.</p>
				{#each holdups as h (h.id)}
					<div class="holdup">
						<div class="holdup-row">
							<span class="holdup-cap">{h.capability}</span>
							<span class="holdup-who">
								{h.agentName}{h.objectName && h.objectName !== h.agentName ? ` · ${h.objectName}` : ""}
							</span>
							<span class="holdup-when">{h.count > 1 ? `×${h.count} · ` : ""}{ago(h.updatedAt)}</span>
							<button class="subtle-btn" onclick={() => void clearHoldup(h.id)}>Clear</button>
						</div>
						<p class="holdup-err">{h.error}</p>
					</div>
				{/each}
			{/if}
		</section>

		<Machines />

		{#if paired}
			<section>
				<h3>Capability requests</h3>
				<p class="hint">Requests arrive on the owning installation object. Nothing installs, signs in, or changes credentials until you approve it here.</p>
				{#if requestError}<p class="hint" role="alert">{requestError}</p>{/if}
				{#if requestNotice}<p class="hint" role="status">{requestNotice}</p>{/if}
				{#if capabilityRequests.length === 0}<p class="hint">No waiting approvals or failed requests.</p>{/if}
				{#each capabilityRequests as request (request.messageId)}
					<div class="skill">
						<div class="skill-row">
							<span class="skill-name">{request.key}{request.account ? ` · ${request.account}` : ""}</span>
							<span class="chip">{request.operation} · {request.status === "processing" && request.operation === "auth.login" ? "waiting for login" : request.status.replaceAll("_", " ")}</span>
						</div>
						<p class="hint">From object {request.sender.objectId}{request.sender.agentId ? ` · agent ${request.sender.agentId}` : ""}</p>
						{#if request.error}<p class="hint" role="status">{request.error}</p>{/if}
						{#if request.status === "pending" || request.status === "awaiting_approval"}
							{#if request.operation === "auth.save" && request.canApprove}
								<p class="hint">These values go only to this machine's local store, never into the request or object history.</p>
								{#each request.fields ?? [] as field (field.key)}
									<label class="cred-field">
										<span>{field.label}</span>
										<input type="password" autocomplete="off" value={credDraft[`${request.key}:${field.key}`] ?? ""} oninput={(e) => (credDraft[`${request.key}:${field.key}`] = e.currentTarget.value)} />
									</label>
								{/each}
							{/if}
							<button class="subtle-btn" disabled={!!requestBusy || !request.canApprove} onclick={() => void resolveRequest(request, "approve")}>{request.operation === "auth.save" ? "Approve & save locally" : "Approve on this machine"}</button>
							<button class="subtle-btn" disabled={!!requestBusy} onclick={() => void resolveRequest(request, "reject")}>Reject</button>
						{:else if request.status === "processing" && request.operation === "auth.login"}
							<button class="subtle-btn" disabled={!!requestBusy} onclick={() => void resolveRequest(request, "finish-login")}>Done signing in — verify</button>
						{:else if request.status === "failed"}
							<button class="subtle-btn" disabled={!!requestBusy || !request.canApprove} onclick={() => void requestOperation(request.key, request.operation, request.account)}>Stage a new request</button>
						{/if}
					</div>
				{/each}
			</section>
		{/if}

		<section>
			<h3>Integrations</h3>
			<p class="hint">
				What each integration IS is a descriptor object; what is TRUE on this machine is its
				installation object. Secrets stay on the machine - only requests and status enter the DAG.
			</p>
			{#if integrations.length === 0 && skillCards.length === 0}
				<p class="hint">{dagError || "No machine has published its integrations yet."}</p>
			{/if}
			{#each integrations as c (c.key)}
				{@const badges = loginBadges(c.key)}
				<div class="skill">
					<div class="skill-row">
						<span class="skill-name cred-label">{c.name}</span>
						{#if paired && machineId}
							{#if badges.password}<span class="chip on">password ✓</span>{/if}
							{#if badges.browser}<span class="chip on">browser ✓</span>{/if}
							{#if badges.status === "needs_auth"}<span class="chip needs-auth">auth needed</span>{/if}
							{#if badges.status === "broken"}<span class="chip failed">broken</span>{/if}
							{#if !badges.password && !badges.browser && badges.status !== "needs_auth" && badges.status !== "broken"}<span class="chip">not set up</span>{/if}
						{/if}
						<span class="row-gap"></span>
						{#if c.fields.length > 0}
							<button class="subtle-btn" disabled={credBusy} onclick={() => { credSetupFor = credSetupFor === c.key ? "" : c.key; credError = ""; }}>{badges.password ? "Replace" : "Enter keys"}</button>
						{/if}
						{#if c.install?.docsUrl && !badges.browser}
							<button class="subtle-btn" disabled={credBusy} onclick={() => void requestOperation(c.key, "auth.login")}>Request login</button>
						{/if}
						{#if badges.password || badges.browser}
							{#if credRemoveConfirm === c.key}
								<button class="subtle-btn reset-right" disabled={credBusy} onclick={async () => { if (await requestOperation(c.key, "auth.revoke")) credRemoveConfirm = ""; }}>Request removal?</button>
								<button class="subtle-btn" onclick={() => (credRemoveConfirm = "")}>Cancel</button>
							{:else}
								<button class="remove-link" onclick={() => (credRemoveConfirm = c.key)}>Remove</button>
							{/if}
						{/if}
					</div>
					<p class="hint cred-note">{c.description}</p>
					{#if badges.error}<p class="hint" role="status">{badges.error}</p>{/if}
					{#if credSetupFor === c.key && c.fields.length > 0}
						<div class="cred-form">
							{#each c.fields as f (f.key)}
								<label class="cred-field">
									<span>{f.label}</span>
									<input
										type={f.secret || f.format === "password" ? "password" : f.format === "email" ? "email" : f.format === "url" ? "url" : "text"}
										placeholder={f.note ?? ""}
										autocomplete="off"
										value={credDraft[`${c.key}:${f.key}`] ?? ""}
										oninput={(e) => (credDraft = { ...credDraft, [`${c.key}:${f.key}`]: e.currentTarget.value })}
									/>
								</label>
							{/each}
							<div class="cred-actions">
								<button class="subtle-btn" disabled={credBusy} onclick={() => void savePasswordCredential(c.key)}>Request save to this machine</button>
								<button class="subtle-btn" onclick={() => (credSetupFor = "")}>Cancel</button>
							</div>
						</div>
					{/if}
				</div>
			{/each}
			{#if credError}<p class="hint" role="alert">{credError}</p>{/if}
			<div class="gskills">
				<p class="hint">
					Google account selectors for <code>gws-as</code>. Secrets stay in each account's local config directory; agents choose the account explicitly.
				</p>
				{#each googleRows as row (row.id)}
					{@const account = fieldStr(row.fields, "account")}
					{@const auth = fieldStr(row.fields, "auth")}
					{@const status = fieldStr(row.fields, "status")}
					<div class="gskill">
						<span class="skill-name cred-label">{account}</span>
						<span class="chip {auth !== "" && auth !== "none" ? "on" : status === "broken" ? "failed" : "needs-auth"}">
							{auth !== "" && auth !== "none" ? auth.replaceAll("_", " ") : status === "broken" ? "failed" : status === "missing" ? "missing client" : "auth needed"}
						</span>
						<button class="subtle-btn" disabled={googleAccountBusy} onclick={() => void requestOperation("google", "auth.login", account)}>Request login</button>
						<button class="subtle-btn" disabled={googleAccountBusy} onclick={() => void requestOperation("google", "auth.check", account)}>Request check</button>
						{#if googleRemoveConfirm === account}
							<button class="subtle-btn reset-right" disabled={googleAccountBusy} onclick={() => void removeGoogleAccount(account)}>Remove?</button>
							<button class="subtle-btn" onclick={() => (googleRemoveConfirm = "")}>Cancel</button>
						{:else}
							<button class="remove-link" onclick={() => (googleRemoveConfirm = account)}>Remove</button>
						{/if}
					</div>
					{#if fieldStr(row.fields, "error")}<p class="hint" role="status">{fieldStr(row.fields, "error")}</p>{/if}
				{/each}
				<form
					onsubmit={(e) => {
						e.preventDefault();
						void addGoogleAccount();
					}}
				>
					<input bind:value={googleAccountDraft} placeholder="support@matcherino.com" autocomplete="off" />
					<button type="submit" disabled={googleAccountBusy || !googleAccountDraft.trim()}>Request Google login</button>
				</form>
			</div>
			{#each skillCards as s (s.key)}
				{@const state = skillPhase(s.key)}
				{@const body = skillBodies[s.key.toLowerCase()]}
				<div class="skill">
					<div class="skill-row">
						<button
							class="skill-name"
							onclick={() => {
								skillOpen = skillOpen === s.key ? "" : s.key;
								skillConfirm = "";
							}}>{s.name}</button
						>
						<span class="chip {state.phase}">
							{state.phase === "on"
								? "on"
								: state.phase === "installing"
									? "installing…"
									: state.phase === "uninstalling"
										? "removing…"
										: state.phase === "needs-auth"
											? "auth needed"
											: state.phase === "failed"
												? "failed"
												: "off"}
						</span>
						{#if state.phase === "needs-auth" || state.phase === "failed"}
							<button class="subtle-btn" onclick={() => void skillOp(s.key, "recheck")}>Re-check</button>
						{/if}
						<label class="switch">
							<input
								type="checkbox"
								checked={state.phase === "on" || state.phase === "installing"}
								disabled={state.phase === "installing" || state.phase === "uninstalling"}
								onchange={(e) =>
									void skillOp(s.key, (e.currentTarget as HTMLInputElement).checked ? "enable" : "disable")}
							/>
							<span class="slider"></span>
						</label>
					</div>
					{#if skillOpen === s.key}
						<div class="skill-detail">
							<p class="hint">{s.description}</p>
							<p class="skill-install">Installation: {state.installed ? "done ✓" : state.phase === "installing" ? "running…" : "not installed"}</p>
							{#if state.error}
								<p class="hint auth-hint">{state.error}</p>
							{/if}
							<div class="skill-prompt">
								<div class="pname">
									Prompt <span class="hint-inline">what the agent reads via skill_read</span>
								</div>
								<textarea
									rows="6"
									value={skillPromptDraft[s.key] ?? body?.text ?? ""}
									oninput={(e) => (skillPromptDraft[s.key] = (e.currentTarget as HTMLTextAreaElement).value)}
								></textarea>
								<div class="skill-prompt-actions">
									<button
										class="subtle-btn"
										disabled={(skillPromptDraft[s.key] ?? body?.text ?? "") === (body?.text ?? "")}
										onclick={() => void saveSkillPrompt(s.key)}
									>{skillPromptSaved === s.key ? "Saved" : "Save prompt"}</button>
									{#if (skillPromptDraft[s.key] ?? body?.text ?? "") !== (body?.text ?? "")}
										<button class="subtle-btn" onclick={() => { delete skillPromptDraft[s.key]; }}>Revert</button>
									{/if}
									{#if body}
										{#if skillResetConfirm === s.key}
											<span class="hint-inline">Replace with the stock prompt?</span>
											<button class="danger-btn" onclick={() => void resetSkillPrompt(s.key)}>Reset</button>
											<button class="subtle-btn" onclick={() => (skillResetConfirm = "")}>Cancel</button>
										{:else}
											<button class="subtle-btn reset-right" onclick={() => (skillResetConfirm = s.key)}>Reset to default</button>
										{/if}
									{/if}
								</div>
							</div>
							{#if jobLogs[s.key]}
								<pre class="skill-log">{jobLogs[s.key].slice(-2000)}</pre>
							{/if}
							{#if state.installed && state.phase !== "installing" && state.phase !== "uninstalling"}
								{#if skillConfirm === s.key}
									<p class="hint">
										Remove {s.name}? This uninstalls it from this device.
										<button class="danger-btn" onclick={() => void skillOp(s.key, "uninstall")}>Remove</button>
										<button class="subtle-btn" onclick={() => (skillConfirm = "")}>Cancel</button>
									</p>
								{:else}
									<button class="remove-link" onclick={() => (skillConfirm = s.key)}>Remove from this device</button>
								{/if}
							{/if}
						</div>
					{/if}
				</div>
			{/each}
			<div class="gskills">
				<p class="hint">
					The integrations above are the global set — every agent lists them. Any other skill belongs
					to one agent, assigned in that agent's prompt panel. These have no owner yet, so every agent
					still lists them:
				</p>
				{#each globalSkills as g (g.id)}
					<div class="gskill">
						<button
							class="skill-name"
							onclick={() => {
								onclose();
								void goto(`/app/object/${g.id}`);
							}}>{g.name}</button>
						<span class="hint-inline">{g.description || "no description — agents pick skills by it"}</span>
					</div>
				{/each}
			</div>
		</section>
	</div>
</div>

<style>
	.overlay {
		position: fixed;
		inset: 0;
		background: rgb(0 0 0 / 0.55);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 200;
	}
	.modal {
		box-sizing: border-box;
		width: 580px;
		max-width: calc(100vw - 48px);
		max-height: 80vh;
		overflow-y: auto;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 14px;
		padding: 18px 22px 22px;
		box-shadow: 0 24px 80px rgb(0 0 0 / 0.6);
	}
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	h2 {
		margin: 0;
		font-size: 16px;
	}
	.cog {
		margin-right: 4px;
	}
	.x {
		background: none;
		border: none;
		color: var(--muted);
		font-size: 20px;
		cursor: pointer;
		padding: 0 4px;
	}
	section {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 14px 16px 16px;
		margin-top: 14px;
	}
	h3 {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--muted);
		margin: 0 0 6px;
	}
	.hint {
		color: var(--muted);
		font-size: 12px;
		margin: 0 0 10px;
		line-height: 1.5;
	}
	.holdup {
		border-top: 1px solid var(--border);
		padding: 8px 0 6px;
	}
	.holdup:first-of-type {
		border-top: none;
	}
	.holdup-row {
		display: flex;
		align-items: baseline;
		gap: 10px;
	}
	.holdup-cap {
		font-weight: 600;
		font-size: 13px;
	}
	.holdup-who {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		font-size: 12.5px;
		color: var(--fg);
	}
	.holdup-when {
		font-size: 11.5px;
		color: var(--muted);
		flex: none;
	}
	.holdup-err {
		margin: 3px 0 0;
		font-size: 12px;
		color: var(--orange);
		line-height: 1.45;
		word-break: break-word;
	}
	.gskills {
		border-top: 1px solid var(--border);
		margin-top: 10px;
		padding-top: 8px;
	}
	.gskill {
		display: flex;
		align-items: baseline;
		gap: 8px;
		padding: 3px 0;
	}
	.skill {
		border-top: 1px solid var(--border, #2a2a2a);
		padding: 6px 0;
	}
	.skill:first-of-type {
		border-top: none;
	}
	.skill-row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.skill-name {
		background: none;
		border: none;
		color: inherit;
		font: inherit;
		font-weight: 600;
		cursor: pointer;
		padding: 0;
		flex: 1;
		text-align: left;
	}
	.chip {
		font-size: 11px;
		padding: 2px 8px;
		border-radius: 10px;
		background: var(--hover, #2a2a2a);
		color: var(--muted);
	}
	.chip.on {
		background: rgb(48 209 88 / 0.16);
		color: var(--green);
	}
	.chip.installing,
	.chip.uninstalling {
		background: rgb(125 122 255 / 0.16);
		color: var(--indigo);
	}
	.chip.needs-auth {
		background: rgb(255 159 10 / 0.16);
		color: var(--orange);
	}
	.chip.failed {
		background: rgb(255 69 58 / 0.16);
		color: var(--red);
	}
	.switch {
		position: relative;
		width: 38px;
		height: 22px;
		flex: none;
	}
	.switch input {
		opacity: 0;
		width: 100%;
		height: 100%;
		margin: 0;
		cursor: pointer;
	}
	.slider {
		position: absolute;
		inset: 0;
		border-radius: 999px;
		background: var(--hover);
		pointer-events: none;
		transition: background 0.15s;
	}
	.slider::before {
		content: "";
		position: absolute;
		top: 2px;
		left: 2px;
		width: 18px;
		height: 18px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 1px 3px rgb(0 0 0 / 0.3);
		transition: transform 0.15s;
	}
	.switch input:checked + .slider {
		background: var(--accent);
	}
	.switch input:checked + .slider::before {
		transform: translateX(16px);
	}
	.switch input:disabled {
		cursor: default;
	}
	.skill-detail {
		padding: 4px 0 4px 2px;
	}
	.auth-hint {
		color: var(--orange);
	}
	.skill-log {
		max-height: 140px;
		overflow: auto;
		font-size: 11px;
		background: var(--hover, #1d1d1d);
		border-radius: 6px;
		padding: 8px;
		white-space: pre-wrap;
	}
	.remove-link {
		background: none;
		border: none;
		color: var(--muted);
		font-size: 11px;
		text-decoration: underline;
		cursor: pointer;
		padding: 0;
	}
	.remove-link:hover {
		color: var(--red);
	}
	.skill-install {
		font-size: 12px;
		color: var(--muted);
		margin: 0 0 6px;
	}
	.skill-prompt {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 8px 0;
	}
	.skill-prompt .pname {
		font-size: 12px;
		color: var(--fg);
	}
	.pname {
		font-size: 13px;
	}
	.hint-inline {
		color: var(--muted);
		font-weight: 400;
	}
	.skill-prompt textarea {
		width: 100%;
		box-sizing: border-box;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 8px;
		color: var(--fg);
		font: inherit;
		font-size: 12.5px;
		line-height: 1.45;
		padding: 8px 10px;
		resize: vertical;
	}
	.skill-prompt-actions {
		display: flex;
		gap: 8px;
		align-items: center;
	}
	.reset-right {
		margin-left: auto;
	}
	.subtle-btn,
	.danger-btn {
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 7px;
		padding: 5px 12px;
		font: inherit;
		font-size: 12px;
		cursor: pointer;
	}
	.subtle-btn:hover:not(:disabled),
	.danger-btn:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.subtle-btn:disabled,
	.danger-btn:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.cred-field {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin: 8px 0;
		font-size: 12px;
	}
	.cred-field input {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 6px;
		color: var(--fg);
		padding: 6px 9px;
		font: inherit;
	}
	.cred-field input:focus {
		outline: none;
		border-color: var(--accent);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 40%, transparent);
	}
	.subtle-btn {
		color: var(--muted);
	}
	.danger-btn {
		color: var(--red);
	}
</style>