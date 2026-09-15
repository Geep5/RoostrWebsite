<script lang="ts">
	// ── This machine ────────────────────────────────────────────────
	//
	// The machine-scoped panel: holdups (agents that needed a capability
	// and couldn't proceed) and integrations (device capabilities like
	// browserless and gws). Account-scoped things stay in Settings -
	// this surface describes the box the harness runs on.
	import { onMount } from "svelte";
	import { fetchAllQuery, type QueryResultRow } from "$lib/api";
	import { UNSERVED_TYPES, capabilityLabel, fetchMachines, resolveMany, type MachineRow } from "$lib/serving";
	import { goto } from "$app/navigation";
	import { harnessFetch, pairedSession, onPairingChange } from "$lib/local-transport";
	import PairGate from "./PairGate.svelte";

	let { onclose }: { onclose: () => void } = $props();

	let paired = $state(false);
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
	interface CredentialRow {
		key: string;
		label: string;
		note: string;
		loginUrl?: string;
		passwordFields?: Array<{ key: string; label: string; secret: boolean }>;
		active: { password: boolean; browser: boolean };
		updatedAt?: number;
	}
	let credentials = $state<CredentialRow[] | null>(null);
	let credSetupFor = $state("");
	let credDraft = $state<Record<string, string>>({});
	let credBrowserPending = $state("");
	let credRemoveConfirm = $state("");
	let credError = $state("");
	let credBusy = $state(false);

	async function loadCredentials() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/credentials");
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			credentials = ((await res.json()) as { credentials: CredentialRow[] }).credentials;
			credError = "";
		} catch (error) {
			credentials = null;
			credError = error instanceof Error ? error.message : "Cannot load credentials.";
		}
	}

	async function credCall(path: string, body: Record<string, unknown>): Promise<string> {
		credBusy = true;
		credError = "";
		try {
			const res = await harnessFetch(path, { method: "POST", body: JSON.stringify(body) });
			const out = (await res.json()) as { error?: string; active?: boolean };
			if (!res.ok || out.error) return (credError = out.error ?? `HTTP ${res.status}`);
			await loadCredentials();
			await loadMachines();
			return "";
		} catch (error) {
			return (credError = error instanceof Error ? error.message : "Request failed.");
		} finally {
			credBusy = false;
		}
	}

	async function savePasswordCredential(key: string, fields: Array<{ key: string; label: string; secret: boolean }>) {
		const values: Record<string, string> = {};
		for (const f of fields) values[f.key] = credDraft[`${key}:${f.key}`] ?? "";
		if (!(await credCall("/credentials/password", { key, fields: values }))) credSetupFor = "";
	}

	async function startBrowserLogin(key: string) {
		if (!(await credCall("/credentials/browser/start", { key }))) credBrowserPending = key;
	}

	async function finishBrowserLogin(key: string) {
		const res = await harnessFetch("/credentials/browser/finish", { method: "POST", body: JSON.stringify({ key }) });
		const out = (await res.json()) as { active?: boolean; error?: string };
		if (!out.active) {
			credError = "No login cookies yet - finish signing in in the Chrome window, then click Done.";
			return;
		}
		credBrowserPending = "";
		await loadCredentials();
		await loadMachines();
	}
	let skillPromptDraft = $state<Record<string, string>>({});
	let skillPromptSaved = $state<string>("");
	let skillOpen = $state<string>("");
	let skillConfirm = $state<string>("");
	let skillResetConfirm = $state<string>("");
	let skillPoll: ReturnType<typeof setInterval> | undefined;

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
		await changeSkill(`/skills/${op}`, { key });
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

	// ── Machines: the roster from the DAG, what each can do, what it serves ──
	//
	// Serving is resolved client-side by the engine, one call per candidate:
	// every space (its default) plus every object carrying a pin or a
	// capability need. Anything else follows its space and is not listed.
	const SERVES_SHOWN = 20;
	interface MachineView extends MachineRow {
		serves: string[];
	}
	let machines = $state<MachineView[] | null>(null);
	let machinesError = $state("");
	async function loadMachines() {
		try {
			const [{ rows, machines: roster }, spaces, pinned, needing] = await Promise.all([
				fetchMachines(),
				fetchAllQuery({ type: "channel" }),
				fetchAllQuery({ filters: [{ key: "served_by", condition: "exists" }] }),
				fetchAllQuery({ filters: [{ key: "requires", condition: "exists" }] }),
			]);
			// Oldest first: the first channel is the default space owning unstamped objects.
			spaces.sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
			const seen = new Set<string>();
			const objects: QueryResultRow[] = [];
			for (const r of [...spaces, ...pinned, ...needing]) {
				if (seen.has(r.id) || (r.typeKey !== "channel" && UNSERVED_TYPES[r.typeKey])) continue;
				seen.add(r.id);
				objects.push(r);
			}
			const resolved = await resolveMany(objects, spaces, rows);
			machines = roster.map((m) => ({
				...m,
				serves: objects.filter((_, i) => resolved[i].machineId === m.machineId).map((o) => o.fields["name"]?.stringValue || "Untitled"),
			}));
			machinesError = "";
		} catch (error) {
			machines = null;
			machinesError = error instanceof Error ? error.message : "Cannot load machines.";
		}
	}

	function refreshPairing() {
		paired = !!pairedSession();
		if (!paired) {
			skillRows = null;
			holdups = [];
			credentials = null;
			harnessError = "";
			if (skillPoll) clearInterval(skillPoll);
			skillPoll = undefined;
			return;
		}
		void loadSkills();
		void loadCredentials();
	}

	onMount(() => {
		refreshPairing();
		void loadMachines();
		void loadGlobalSkills().catch((error) => {
			harnessError = error instanceof Error ? error.message : "Cannot load saved skills.";
		});
		const unsubscribe = onPairingChange(refreshPairing);
		return () => {
			unsubscribe();
			if (skillPoll) clearInterval(skillPoll);
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

		<section>
			<h3>Credentials</h3>
			{#if !paired}
				<p class="hint">Pair to manage this machine's credentials.</p>
			{:else if credentials === null}
				<p class="hint" role="alert">{credError || "Credentials are unavailable until the harness responds."}</p>
			{:else}
				<p class="hint">
					Service logins agents on this machine may use. Passwords live only on this machine (owner-only file); browser
					logins open a Chrome you sign into once, and agents reuse that profile. An active credential becomes a machine
					capability, so work that needs it is routed here.
				</p>
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
								<button class="subtle-btn" disabled={credBusy} onclick={() => void startBrowserLogin(c.key)}>Open login window</button>
							{/if}
							{#if c.active.password || c.active.browser}
								{#if credRemoveConfirm === c.key}
									<button class="subtle-btn reset-right" disabled={credBusy} onclick={async () => { if (!(await credCall("/credentials/remove", { key: c.key }))) credRemoveConfirm = ""; }}>Remove?</button>
									<button class="subtle-btn" onclick={() => (credRemoveConfirm = "")}>Cancel</button>
								{:else}
									<button class="remove-link" onclick={() => (credRemoveConfirm = c.key)}>Remove</button>
								{/if}
							{/if}
						</div>
						<p class="hint cred-note">{c.note}</p>
						{#if credBrowserPending === c.key}
							<p class="hint cred-browser-note">A Chrome window opened on this Mac - sign in there, then come back and <button class="subtle-btn" onclick={() => void finishBrowserLogin(c.key)}>Done</button></p>
						{/if}
						{#if credSetupFor === c.key && c.passwordFields}
							<div class="cred-form">
								{#each c.passwordFields as f (f.key)}
									<label class="cred-field">
										<span>{f.label}</span>
										<input
											type={f.secret ? "password" : "text"}
											autocomplete="off"
											value={credDraft[`${c.key}:${f.key}`] ?? ""}
											oninput={(e) => (credDraft = { ...credDraft, [`${c.key}:${f.key}`]: e.currentTarget.value })}
										/>
									</label>
								{/each}
								<div class="cred-actions">
									<button class="subtle-btn" disabled={credBusy} onclick={() => void savePasswordCredential(c.key, c.passwordFields!)}>Save to this machine</button>
									<button class="subtle-btn" onclick={() => (credSetupFor = "")}>Cancel</button>
								</div>
							</div>
						{/if}
					</div>
				{/each}
				{#if credError}<p class="hint" role="alert">{credError}</p>{/if}
			{/if}
		</section>

		<section>
			<h3>Machines</h3>
			{#if machinesError}
				<p class="hint" role="alert">{machinesError}</p>
			{:else if machines === null}
				<p class="hint">Loading machines…</p>
			{:else if machines.length === 0}
				<p class="hint">No machine has published itself yet — run the harness on a device to add one.</p>
			{:else}
				<p class="hint">Every device running a harness, what it can do, and what the engine routes to it: space defaults, pinned objects, and capability needs. Pin or edit needs from any object's header.</p>
				{#each machines as m (m.id)}
					<div class="machine">
						<div class="machine-row">
							<span class="machine-name">🖥️ {m.name || `${m.machineId.slice(0, 8)}…`}</span>
							{#each m.capabilities as c (c)}
								<span class="chip on">{capabilityLabel(c)}</span>
							{/each}
							{#if m.capabilities.length === 0}
								<span class="chip">no capabilities</span>
							{/if}
						</div>
						<p class="machine-serves">
							{#if m.serves.length === 0}
								serves nothing
							{:else}
								serves {m.serves.length}: {m.serves.slice(0, SERVES_SHOWN).join(", ")}{#if m.serves.length > SERVES_SHOWN} +{m.serves.length - SERVES_SHOWN} more{/if}
							{/if}
						</p>
					</div>
				{/each}
			{/if}
		</section>

		<section>
			<h3>Integrations</h3>
			{#if skillRows === null}
				<p class="hint">{paired ? "Machine integrations are unavailable until the harness responds." : "Pair to manage machine integrations."}</p>
			{:else}
				<p class="hint">
					Device-local capabilities, brokered by the harness: every agent can use an enabled one
					through its tools (web_fetch, …) without ever seeing this machine's credentials.
				</p>
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
	.subtle-btn {
		color: var(--muted);
	}
	.danger-btn {
		color: var(--red);
	}
	.machine {
		border-top: 1px solid var(--border);
		padding: 8px 0 6px;
	}
	.machine:first-of-type {
		border-top: none;
	}
	.machine-row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 6px;
	}
	.machine-name {
		font-size: 13px;
		font-weight: 600;
		margin-right: 4px;
	}
	.machine-serves {
		margin: 3px 0 0;
		font-size: 12px;
		color: var(--muted);
		line-height: 1.45;
	}
</style>