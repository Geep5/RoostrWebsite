<script lang="ts">
	// ── This machine ────────────────────────────────────────────────
	//
	// The machine-scoped panel: holdups (agents that needed a capability
	// and couldn't proceed) and integrations (device capabilities like
	// browserless and gws). Account-scoped things stay in Settings -
	// this surface describes the box the harness runs on.
	import { onMount } from "svelte";
	import { fetchAllQuery, mailbox, note } from "$lib/api";
	import Machines from "./Machines.svelte";
	import { goto } from "$app/navigation";
	import { harnessFetch, pairedSession, onPairingChange } from "$lib/local-transport";
	import { loadCards, type Card } from "$lib/cards";
	import PairGate from "./PairGate.svelte";

	let { onclose }: { onclose: () => void } = $props();

	let paired = $state(pairedSession() !== null);
	let harnessError = $state("");

	interface SkillRow {
		key: string;
		name: string;
		description: string;
		phase: "off" | "installing" | "needs-auth" | "on" | "failed" | "uninstalling";
		installed: boolean;
		log: string;
		authHint?: string;
		/** Live prompt body from the skill object - what agents actually read. */
		prompt: string;
		/** Catalog stock prompt - the reset target. */
		defaultPrompt: string;
	}
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

	let skillRows = $state<SkillRow[] | null>(null);
	let holdups = $state<Holdup[]>([]);

	// ── Credentials: service logins agents on this machine may use ──
	//
	// Two sources, deliberately split: the SHAPE (label, note, which inputs,
	// which are secret, where to log in) is a descriptor card in the vault, so
	// an unpaired browser or a phone can still describe what X needs; the
	// ACTIVE state is machine-local, and only that machine can answer it.
	// Secret values are written to the machine over the paired local API and
	// never enter the DAG.
	interface CredentialRow {
		key: string;
		label: string;
		note: string;
		loginUrl?: string;
		passwordFields?: Array<{ key: string; label: string; secret: boolean; format?: string; note?: string }>;
		active: { password: boolean; browser: boolean };
		updatedAt?: number;
	}
	let credentials = $state<CredentialRow[] | null>(null);
	let credSetupFor = $state("");
	let credDraft = $state<Record<string, string>>({});
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
	let credRemoveConfirm = $state("");
	let credError = $state("");
	let credBusy = $state(false);
	interface GoogleAccountRow {
		account: string;
		configured: boolean;
		authMethod: string;
		clientConfigExists: boolean;
		credentialsExists: boolean;
		storage: string;
		error?: string;
	}
	let googleAccounts = $state<GoogleAccountRow[] | null>(null);
	let googleAccountDraft = $state("");
	let googleAccountError = $state("");
	let googleAccountBusy = $state(false);
	let googleRemoveConfirm = $state("");

	/** A card's form, rendered generically: FieldSpec[] IS the form. */
	function cardRow(card: Card, active: { password: boolean; browser: boolean }): CredentialRow {
		const fields = card.fields.map((f) => ({
			key: f.key,
			label: f.label,
			secret: f.secret,
			format: f.format,
			note: f.note,
		}));
		return {
			key: card.key,
			label: card.name,
			note: card.description,
			...(card.install?.docsUrl ? { loginUrl: card.install.docsUrl } : {}),
			...(fields.length ? { passwordFields: fields } : {}),
			active,
		};
	}

	async function loadCredentials() {
		const integrations = (await loadCards()).filter((c) => c.kind === "integration");
		if (!pairedSession()) {
			// Unpaired: the cards still say what each login needs. Nothing is
			// claimed about whether it is set up here - that is not knowable.
			credentials = integrations.map((c) => cardRow(c, { password: false, browser: false }));
			credError = integrations.length ? "" : "No machine has published its logins yet.";
			return;
		}
		try {
			const res = await harnessFetch("/credentials");
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			const live = ((await res.json()) as { credentials: Array<{ key: string; active: { password: boolean; browser: boolean }; updatedAt?: number }> })
				.credentials;
			const activeOf = new Map(live.map((c) => [c.key, c]));
			// Cards first, so a login this machine has not heard of still
			// shows; then any key the machine reports without a card.
			const rows = integrations.map((c) => {
				const hit = activeOf.get(c.key);
				const row = cardRow(c, hit?.active ?? { password: false, browser: false });
				return hit?.updatedAt ? { ...row, updatedAt: hit.updatedAt } : row;
			});
			for (const c of live) {
				if (integrations.some((card) => card.key === c.key)) continue;
				rows.push({ key: c.key, label: c.key, note: "No card describes this login.", active: c.active });
			}
			credentials = rows;
			credError = "";
		} catch (error) {
			credentials = null;
			credError = error instanceof Error ? error.message : "Cannot load credentials.";
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
				await Promise.all([loadCredentials(), loadGoogleAccounts(), loadSkills()]);
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
			const [installs, machines] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "machine" })]);
			let installationId = installs.find((row) => row.fields["key"]?.stringValue === key && row.fields["machine_id"]?.stringValue === machine.id && (row.fields["account"]?.stringValue ?? "") === account)?.id;
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
			await Promise.all([loadCapabilityRequests(), loadCredentials(), loadGoogleAccounts(), loadSkills()]);
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
	let skillPromptDraft = $state<Record<string, string>>({});
	let skillPromptSaved = $state<string>("");
	let skillOpen = $state<string>("");
	let skillConfirm = $state<string>("");
	let skillResetConfirm = $state<string>("");
	let skillPoll: ReturnType<typeof setInterval> | undefined;

	async function loadGoogleAccounts() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/google/accounts");
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			googleAccounts = ((await res.json()) as { accounts: GoogleAccountRow[] }).accounts;
			googleAccountError = "";
		} catch (error) {
			googleAccounts = null;
			googleAccountError = error instanceof Error ? error.message : "Cannot load Google accounts.";
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

	async function loadSkills() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/skills");
			if (!res.ok) throw new Error(`Cannot load integrations (HTTP ${res.status}).`);
			const out = (await res.json()) as { skills: SkillRow[]; holdups?: Holdup[] };
			if (!pairedSession()) return;
			skillRows = out.skills;
			holdups = (out.holdups ?? []).sort((a, b) => b.updatedAt - a.updatedAt);
			harnessError = "";
			const busy = skillRows.some((s) => s.phase === "installing" || s.phase === "uninstalling");
			if (busy && !skillPoll) skillPoll = setInterval(() => void loadSkills(), 2000);
			if (!busy && skillPoll) {
				clearInterval(skillPoll);
				skillPoll = undefined;
			}
		} catch (error) {
			skillRows = null;
			harnessError = error instanceof Error ? error.message : "The paired harness is unreachable.";
			if (skillPoll) clearInterval(skillPoll);
			skillPoll = undefined;
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
	// The global set is hardcoded: exactly the catalog above, device
	// capabilities every agent lists. A hand-written skill belongs to one
	// agent, set in that agent's prompt panel — these are the ones with
	// no owner yet, so every agent still lists them.
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
		const operation = op === "recheck" ? "auth.check" : op === "enable" && !skillRows?.find((row) => row.key === key)?.installed ? "skill.install" : `skill.${op}`;
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
			await loadSkills();
			return true;
		} catch (error) {
			harnessError = error instanceof Error ? error.message : "The paired harness is unreachable.";
			return false;
		}
	}

	function refreshPairing() {
		paired = !!pairedSession();
		if (!paired) {
			skillRows = null;
			holdups = [];
			credentials = null;
			harnessError = "";
			capabilityRequests = [];
			credDraft = {};
			if (requestPoll) clearInterval(requestPoll);
			requestPoll = undefined;
			if (skillPoll) clearInterval(skillPoll);
			skillPoll = undefined;
			return;
		}
		void loadSkills();
		void loadCredentials();
		void loadGoogleAccounts();
		void loadCapabilityRequests();
		if (!requestPoll) requestPoll = setInterval(() => void loadCapabilityRequests(), 2000);
	}

	onMount(() => {
		refreshPairing();
		void loadGlobalSkills().catch((error) => {
			harnessError = error instanceof Error ? error.message : "Cannot load saved skills.";
		});
		const unsubscribe = onPairingChange(refreshPairing);
		return () => {
			unsubscribe();
			if (skillPoll) clearInterval(skillPoll);
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
		{#if paired && skillRows === null}
			<button class="subtle-btn" onclick={refreshPairing}>Retry harness connection</button>
		{/if}

		<section>
			<h3>Holdups</h3>
			{#if skillRows === null}
				<p class="hint">{paired ? "Machine holdups are unavailable until the harness responds." : "Pair to view machine holdups."}</p>
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
			{#if skillRows === null}
				<p class="hint">{paired ? "Machine integrations are unavailable until the harness responds." : "Pair to manage machine integrations."}</p>
			{:else}
				<p class="hint">
					Device-local capabilities, brokered by the harness: every agent can use an enabled one
					through its tools (web_fetch, …) without ever seeing this machine's credentials.
				</p>
				{#if credentials === null}
					<p class="hint" role="alert">{credError || "Credentials are unavailable until the harness responds."}</p>
				{:else if credentials}
					{#each credentials as c (c.key)}
						<div class="skill">
							<div class="skill-row">
								<span class="skill-name cred-label">{c.label}</span>
								{#if c.active.password}<span class="chip on">password ✓</span>{/if}
								{#if c.active.browser}<span class="chip on">browser ✓</span>{/if}
								{#if !c.active.password && !c.active.browser}<span class="chip">not set up</span>{/if}
								<span class="row-gap"></span>
								{#if c.passwordFields}
									<button class="subtle-btn" disabled={credBusy} onclick={() => { credSetupFor = credSetupFor === c.key ? "" : c.key; credError = ""; }}>{c.active.password ? "Replace" : "Enter keys"}</button>
								{/if}
								{#if c.loginUrl && !c.active.browser}
									<button class="subtle-btn" disabled={credBusy} onclick={() => void requestOperation(c.key, "auth.login")}>Request login</button>
								{/if}
								{#if c.active.password || c.active.browser}
									{#if credRemoveConfirm === c.key}
										<button class="subtle-btn reset-right" disabled={credBusy} onclick={async () => { if (await requestOperation(c.key, "auth.revoke")) credRemoveConfirm = ""; }}>Request removal?</button>
										<button class="subtle-btn" onclick={() => (credRemoveConfirm = "")}>Cancel</button>
									{:else}
										<button class="remove-link" onclick={() => (credRemoveConfirm = c.key)}>Remove</button>
									{/if}
								{/if}
							</div>
							<p class="hint cred-note">{c.note}</p>
							{#if credSetupFor === c.key && c.passwordFields}
								<div class="cred-form">
									{#each c.passwordFields as f (f.key)}
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
				{/if}
				<div class="gskills">
					<p class="hint">
						Google account selectors for <code>gws-as</code>. Secrets stay in each account's local config directory; agents choose the account explicitly.
					</p>
					{#if googleAccounts === null}
						<p class="hint" role="alert">{googleAccountError || "Google accounts are unavailable until the harness responds."}</p>
					{:else}
						{#each googleAccounts as account (account.account)}
							<div class="gskill">
								<span class="skill-name cred-label">{account.account}</span>
								<span class="chip {account.authMethod !== "none" && account.authMethod !== "" ? "on" : account.clientConfigExists ? "needs-auth" : "failed"}">
									{account.authMethod !== "none" && account.authMethod !== "" ? account.authMethod : account.clientConfigExists ? "auth needed" : "missing client"}
								</span>
								<button class="subtle-btn" disabled={googleAccountBusy} onclick={() => void requestOperation("google", "auth.login", account.account)}>Request login</button>
								<button class="subtle-btn" disabled={googleAccountBusy} onclick={() => void requestOperation("google", "auth.check", account.account)}>Request check</button>
								{#if googleRemoveConfirm === account.account}
									<button class="subtle-btn reset-right" disabled={googleAccountBusy} onclick={() => void removeGoogleAccount(account.account)}>Remove?</button>
									<button class="subtle-btn" onclick={() => (googleRemoveConfirm = "")}>Cancel</button>
								{:else}
									<button class="remove-link" onclick={() => (googleRemoveConfirm = account.account)}>Remove</button>
								{/if}
							</div>
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
						{#if googleAccountError}<p class="hint" role="alert">{googleAccountError}</p>{/if}
					{/if}
				</div>
				{#each skillRows as s (s.key)}
					<div class="skill">
						<div class="skill-row">
							<button
								class="skill-name"
								onclick={() => {
									skillOpen = skillOpen === s.key ? "" : s.key;
									skillConfirm = "";
								}}>{s.name}</button
							>
							<span class="chip {s.phase}">
								{s.phase === "on"
									? "on"
									: s.phase === "installing"
										? "installing…"
										: s.phase === "uninstalling"
											? "removing…"
											: s.phase === "needs-auth"
												? "auth needed"
												: s.phase === "failed"
													? "failed"
													: "off"}
							</span>
							{#if s.phase === "needs-auth" || s.phase === "failed"}
								<button class="subtle-btn" onclick={() => void skillOp(s.key, "recheck")}>Re-check</button>
							{/if}
							<label class="switch">
								<input
									type="checkbox"
									checked={s.phase === "on" || s.phase === "installing"}
									disabled={s.phase === "installing" || s.phase === "uninstalling"}
									onchange={(e) =>
										void skillOp(s.key, (e.currentTarget as HTMLInputElement).checked ? "enable" : "disable")}
								/>
								<span class="slider"></span>
							</label>
						</div>
						{#if skillOpen === s.key}
							<div class="skill-detail">
								<p class="hint">{s.description}</p>
								<p class="skill-install">Installation: {s.installed ? "done ✓" : s.phase === "installing" ? "running…" : "not installed"}</p>
								{#if s.phase === "needs-auth" && s.authHint}
									<p class="hint auth-hint">{s.authHint}</p>
								{/if}
								<div class="skill-prompt">
									<div class="pname">
										Prompt <span class="hint-inline">what the agent reads via skill_read</span>
									</div>
									<textarea
										rows="6"
										value={skillPromptDraft[s.key] ?? s.prompt}
										oninput={(e) => (skillPromptDraft[s.key] = (e.currentTarget as HTMLTextAreaElement).value)}
									></textarea>
									<div class="skill-prompt-actions">
										<button
											class="subtle-btn"
											disabled={(skillPromptDraft[s.key] ?? s.prompt) === s.prompt}
											onclick={() => void saveSkillPrompt(s.key)}
										>{skillPromptSaved === s.key ? "Saved" : "Save prompt"}</button>
										{#if (skillPromptDraft[s.key] ?? s.prompt) !== s.prompt}
											<button class="subtle-btn" onclick={() => { delete skillPromptDraft[s.key]; }}>Revert</button>
										{/if}
										{#if s.prompt.trim() !== s.defaultPrompt.trim()}
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
								{#if s.log}
									<pre class="skill-log">{s.log.slice(-2000)}</pre>
								{/if}
								{#if s.installed && s.phase !== "installing" && s.phase !== "uninstalling"}
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
			{/if}
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