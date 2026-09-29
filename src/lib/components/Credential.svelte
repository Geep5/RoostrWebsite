<script lang="ts">
	/**
	 * A credential object's page: one login for one service. The credential
	 * carries its own sign-in recipe as plain fields (`service`,
	 * `description`, `login_url`, `session_host` + `session_cookie`,
	 * `key_fields`) and its secret (pasted keys in `secret`, a browser
	 * sign-in's cookies in `session`), so agents on any computer can use it -
	 * and anyone in its space can read it. A blank credential starts from one
	 * of its space's credential templates, which the harness seeds. The
	 * computer its Served by names opens the sign-in window and keeps the
	 * status true; Connect, Check and Disconnect run only when this tab is
	 * paired with that computer.
	 */
	import { onMount } from "svelte";
	import { fetchAllQuery, fetchObject, note, type QueryResultRow } from "$lib/api";
	import { thisMachineId } from "$lib/capability-actions";
	import { agoShort } from "$lib/conversations";
	import { applyTemplate } from "$lib/create";
	import { store } from "$lib/data.svelte";
	import { harnessFetch, onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, servedByMachineId, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON, type ValueJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	/** One `key_fields` item: a key a person pastes, stored under `key` in `secret`. */
	interface KeyField {
		key: string;
		label: string;
		secret: boolean;
	}
	interface CredentialTemplate {
		id: string;
		name: string;
		description: string;
	}
	interface HarnessResult {
		status?: string;
		error?: string;
	}

	let { object, onchanged }: { object: ObjectJSON; onchanged: () => Promise<void> } = $props();

	const DEFAULT_NAME = "New credential";
	/** The fields the owning harness rewrites; a change in any of them re-renders the page. */
	const HARNESS_FIELDS = ["status", "error", "checked_at", "auth"];
	const CONNECT_POLL_MS = 3_000;
	const CONNECT_TIMEOUT_MS = 5 * 60_000;

	let machines = $state<MachineRow[]>([]);
	let templates = $state<CredentialTemplate[]>([]);
	let templatesLoaded = $state(false);
	let templatesError = $state("");
	/** "Start blank" chosen: show the empty recipe editor instead of the template list. */
	let startBlank = $state(false);
	let keyRows = $state<KeyField[]>([]);
	let keyRowsJson = "";
	let paired = $state(pairedSession() !== null);
	let thisMachine = $state("");
	let busy = $state("");
	let actionError = $state("");
	let saveError = $state("");
	let waiting = $state(false);
	let draft = $state<Record<string, string>>({});
	let now = $state(Date.now());
	let connectPoll: ReturnType<typeof setInterval> | undefined;
	let connectDeadline = 0;
	let polling = false;

	const id = $derived(object.id);
	const serviceKey = $derived(fieldStr(object.fields, "service"));
	const description = $derived(fieldStr(object.fields, "description"));
	const loginUrl = $derived(fieldStr(object.fields, "login_url"));
	const sessionHost = $derived(fieldStr(object.fields, "session_host"));
	const sessionCookie = $derived(fieldStr(object.fields, "session_cookie"));
	const keyFields = $derived(parseKeyFields(object.fields["key_fields"]));
	const channel = $derived(fieldStr(object.fields, "channel"));
	/** Sign-in through a browser window needs the page to open and the cookie that proves it worked. */
	const browserLogin = $derived(!!loginUrl.trim() && !!sessionHost.trim() && !!sessionCookie.trim());
	/** No recipe at all yet: offer the space's credential templates. */
	const blank = $derived(!serviceKey && !loginUrl && keyFields.length === 0);
	const account = $derived(fieldStr(object.fields, "account"));
	const status = $derived(fieldStr(object.fields, "status") || "missing");
	const error = $derived(fieldStr(object.fields, "error"));
	const auth = $derived(fieldStr(object.fields, "auth"));
	const checkedAt = $derived(object.fields["checked_at"]?.intValue ?? 0);
	const servedBy = $derived(servedByMachineId(object.fields, machines));
	const machine = $derived(machines.find((m) => m.machineId === servedBy));
	const keeperName = $derived(servedBy ? machineName(machines, servedBy) : "");
	const secretFields = $derived(keyFields.filter((f) => f.key.trim()));
	/** Actions run on the keeping computer's harness, so only a tab paired with it offers them. */
	const canAct = $derived(paired && !!thisMachine && thisMachine === servedBy);
	const statusText = $derived(
		status === "active" ? "Connected"
		: status === "connecting" ? "Waiting for sign-in"
		: status === "needs_auth" ? "Signed out - reconnect"
		: status === "broken" ? "Broken"
		: status === "missing" ? "Not connected"
		: status.replaceAll("_", " "),
	);
	const statusClass = $derived(
		status === "active" ? "st-active" : status === "connecting" || status === "needs_auth" ? "st-needs" : status === "broken" ? "st-broken" : "st-missing",
	);
	const checked = $derived.by(() => {
		void now; // agoShort reads the clock; the tick re-derives it
		if (!checkedAt) return "";
		const ago = agoShort(checkedAt);
		return ago === "now" ? "checked just now" : `checked ${ago} ago`;
	});

	function parseKeyFields(v: ValueJSON | undefined): KeyField[] {
		return (v?.valuesValue?.items ?? []).flatMap((item) => {
			const e = item.mapValue?.entries;
			// No flag means secret, as the harness reads it (credentials.ts recipeOf): mask by default.
			return e ? [{ key: e["key"]?.stringValue ?? "", label: e["label"]?.stringValue ?? "", secret: e["secret"]?.boolValue ?? true }] : [];
		});
	}

	/** Credential templates of this credential's space; every credential template when that space has none. */
	async function loadTemplates(space: string) {
		try {
			const credentialTypes = new Set(store.types.filter((t) => t.key === "credential").map((t) => t.id));
			const isCredential = (r: QueryResultRow) => {
				const target = fieldStr(r.fields, "target_type");
				return credentialTypes.has(target) || target.startsWith("bundled-type-credential-");
			};
			const all = (await fetchAllQuery({ type: "template" })).filter(isCredential);
			const spaceType = `bundled-type-credential-${space.slice(0, 8)}`;
			const own = all.filter((r) => fieldStr(r.fields, "target_type") === spaceType && fieldStr(r.fields, "channel") === space);
			// Other spaces carry their own copies of the same seeds: list each once.
			const seen = new Set<string>();
			templates = (own.length ? own : all)
				.filter((r) => {
					const k = fieldStr(r.fields, "seed_key") || r.id;
					return !seen.has(k) && !!seen.add(k);
				})
				.map((r) => ({ id: r.id, name: fieldStr(r.fields, "name"), description: fieldStr(r.fields, "description") }));
			templatesError = "";
		} catch (e) {
			templatesError = `Cannot load credential templates: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			templatesLoaded = true;
		}
	}

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
	}

	/** The harness writes status onto the object; re-render when it did. */
	async function watch() {
		now = Date.now();
		const fresh = await fetchObject(id).catch(() => null);
		if (fresh && HARNESS_FIELDS.some((k) => JSON.stringify(fresh.fields[k]) !== JSON.stringify(object.fields[k]))) await onchanged();
	}

	/** POST to the paired harness. A body with `error` and no `status` is a refusal (409 included). */
	async function post(path: string, body: Record<string, unknown>): Promise<HarnessResult> {
		const res = await harnessFetch(path, { method: "POST", body: JSON.stringify({ id, ...body }) });
		const result = (await res.json().catch(() => ({}))) as HarnessResult;
		if (!res.ok || (result.error && !result.status)) throw new Error(result.error ?? `HTTP ${res.status}`);
		return result;
	}

	async function act(name: string, path: string, body: Record<string, unknown> = {}): Promise<HarnessResult | null> {
		if (busy) return null;
		busy = name;
		actionError = "";
		try {
			return await post(path, body);
		} catch (e) {
			actionError = e instanceof Error ? e.message : String(e);
			return null;
		} finally {
			busy = "";
			await onchanged().catch(() => {});
		}
	}

	function stopWaiting() {
		if (connectPoll) clearInterval(connectPoll);
		connectPoll = undefined;
		waiting = false;
	}

	async function connect() {
		stopWaiting();
		if (!(await act("connect", "/credentials/connect"))) return;
		waiting = true;
		connectDeadline = Date.now() + CONNECT_TIMEOUT_MS;
		connectPoll = setInterval(() => void pollConnect(), CONNECT_POLL_MS);
	}

	async function pollConnect() {
		if (polling) return;
		if (Date.now() > connectDeadline) {
			stopWaiting();
			actionError = "Still not signed in after 5 minutes. Connect again when you are ready.";
			return;
		}
		polling = true;
		try {
			const result = await post("/credentials/check", {});
			if (result.status === "active") stopWaiting();
		} catch (e) {
			stopWaiting();
			actionError = e instanceof Error ? e.message : String(e);
		} finally {
			polling = false;
			await onchanged().catch(() => {});
		}
	}

	async function disconnect() {
		if (!confirm(`Disconnect ${fieldStr(object.fields, "name") || "this credential"}? This removes the sign-in from ${keeperName || "the computer that keeps it"}.`)) return;
		stopWaiting();
		await act("disconnect", "/credentials/disconnect");
	}

	/** Keys go straight onto the credential; its computer sees the change and marks it connected. */
	async function saveKeys() {
		const fields = Object.fromEntries(secretFields.map((f) => [f.key, (draft[f.key] ?? "").trim()]));
		draft = {};
		await setField("secret", JSON.stringify(fields));
		await onchanged();
	}

	async function setValue(key: string, value: ValueJSON): Promise<boolean> {
		saveError = "";
		try {
			await note.setField(id, key, value);
			return true;
		} catch (e) {
			saveError = e instanceof Error ? e.message : String(e);
			return false;
		}
	}

	const setField = (key: string, value: string) => setValue(key, { stringValue: value });

	/** A plain text field of the credential, saved when it changed. */
	async function saveText(key: string, value: string) {
		const next = value.trim();
		if (next === fieldStr(object.fields, key)) return;
		await setField(key, next);
		await onchanged();
	}

	function blurOnEnter(e: KeyboardEvent & { currentTarget: HTMLInputElement }) {
		if (e.key === "Enter") e.currentTarget.blur();
	}

	/** `key_fields` is saved whole: every row edit writes the full list. */
	async function saveKeyRows() {
		const items = keyRows.map((r) => ({
			mapValue: { entries: { key: { stringValue: r.key.trim() }, label: { stringValue: r.label.trim() }, secret: { boolValue: r.secret } } },
		}));
		await setValue("key_fields", { valuesValue: { items } });
		await onchanged();
	}

	function editKeyRow(i: number, patch: Partial<KeyField>) {
		keyRows[i] = { ...keyRows[i], ...patch };
		void saveKeyRows();
	}

	function addKeyRow() {
		keyRows.push({ key: "", label: "", secret: true });
		void saveKeyRows();
	}

	function removeKeyRow(i: number) {
		keyRows.splice(i, 1);
		void saveKeyRows();
	}

	/** Copy a template's recipe onto this credential; a default name takes the template's. */
	async function startFrom(t: CredentialTemplate) {
		if (busy) return;
		busy = "template";
		saveError = "";
		try {
			await applyTemplate(id, t.id);
			const name = fieldStr(object.fields, "name").trim();
			if ((!name || name === DEFAULT_NAME) && t.name) await note.setField(id, "name", { stringValue: t.name });
		} catch (e) {
			saveError = `Cannot start from ${t.name || "that template"}: ${e instanceof Error ? e.message : String(e)}`;
		} finally {
			busy = "";
			await onchanged().catch(() => {});
		}
	}

	// A different credential in the same page instance starts clean: no
	// connect poll or typed keys carry over, and none outlive the page.
	$effect(() => {
		void id;
		draft = {};
		actionError = "";
		startBlank = false;
		return stopWaiting;
	});

	// Key rows follow the saved list; an unchanged list leaves rows being typed in alone.
	$effect(() => {
		const json = JSON.stringify(keyFields);
		if (json === keyRowsJson) return;
		keyRowsJson = json;
		keyRows = keyFields.map((f) => ({ ...f }));
	});

	$effect(() => {
		if (blank) void loadTemplates(channel);
	});

	onMount(() => {
		void fetchMachines().then(({ machines: roster }) => (machines = roster)).catch(() => {});
		void loadPairing();
		const unsubscribe = onPairingChange(() => void loadPairing());
		const timer = setInterval(() => void watch(), 5_000);
		return () => {
			unsubscribe();
			clearInterval(timer);
		};
	});
</script>

<section class="credential" data-testid="credential">
	<div class="headline">
		<p class="status-big {statusClass}" data-testid="credential-status">{statusText}</p>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		{#if checked}<p class="muted">{checked}</p>{/if}
	</div>

	<div class="sec" data-testid="credential-recipe">
		<div class="sec-name">Service</div>
		{#if blank && !startBlank}
			{#if templates.length > 0}
				<p class="muted">Start from</p>
				<div class="choices">
					{#each templates as t (t.id)}
						<button class="choice" disabled={!!busy} data-testid="credential-template" onclick={() => void startFrom(t)}>
							<span class="choice-name">{t.name || "Untitled template"}</span>
							{#if t.description}<span class="choice-sub">{t.description}</span>{/if}
						</button>
					{/each}
				</div>
			{:else if templatesLoaded && !templatesError}
				<p class="muted">No credential templates in this space yet - the Roostr harness seeds them when it runs on a computer.</p>
			{/if}
			{#if templatesError}<p class="error" role="alert">{templatesError}</p>{/if}
			<div class="actions">
				<button class="subtle-btn" disabled={!!busy} data-testid="credential-start-blank" onclick={() => (startBlank = true)}>Start blank</button>
			</div>
		{:else}
			<div class="recipe">
				<label class="cred-field">
					<span>Service key</span>
					<input value={serviceKey} placeholder="x, discord-bot, …" autocomplete="off" onchange={(e) => void saveText("service", e.currentTarget.value)} onkeydown={blurOnEnter} />
				</label>
				<label class="cred-field">
					<span>Description</span>
					<input value={description} placeholder="What this login is for" autocomplete="off" onchange={(e) => void saveText("description", e.currentTarget.value)} onkeydown={blurOnEnter} />
				</label>
				<label class="cred-field">
					<span>Login page URL</span>
					<input value={loginUrl} placeholder="https://x.com/i/flow/login" autocomplete="off" onchange={(e) => void saveText("login_url", e.currentTarget.value)} onkeydown={blurOnEnter} />
				</label>
				<div class="cred-field">
					<span>Signed-in cookie</span>
					<div class="pair">
						<input value={sessionHost} placeholder="Host, e.g. x.com" aria-label="Cookie host" autocomplete="off" onchange={(e) => void saveText("session_host", e.currentTarget.value)} onkeydown={blurOnEnter} />
						<input value={sessionCookie} placeholder="Cookie, e.g. auth_token" aria-label="Cookie name" autocomplete="off" onchange={(e) => void saveText("session_cookie", e.currentTarget.value)} onkeydown={blurOnEnter} />
					</div>
				</div>
				<div class="cred-field">
					<span>Key fields</span>
					{#each keyRows as row, i (i)}
						<div class="key-row" data-testid="credential-key-field">
							<input value={row.label} placeholder="Label, e.g. Bot token" aria-label="Key field label" autocomplete="off" onchange={(e) => editKeyRow(i, { label: e.currentTarget.value })} onkeydown={blurOnEnter} />
							<input value={row.key} placeholder="Key, e.g. token" aria-label="Key field key" autocomplete="off" onchange={(e) => editKeyRow(i, { key: e.currentTarget.value })} onkeydown={blurOnEnter} />
							<label class="secret-toggle"><input type="checkbox" checked={row.secret} onchange={(e) => editKeyRow(i, { secret: e.currentTarget.checked })} /> Secret</label>
							<button class="subtle-btn" aria-label="Remove key field" onclick={() => removeKeyRow(i)}>×</button>
						</div>
					{/each}
					<div class="actions">
						<button class="subtle-btn" data-testid="credential-add-key-field" onclick={addKeyRow}>Add key field</button>
					</div>
				</div>
				{#if !browserLogin && (loginUrl || sessionHost || sessionCookie)}
					<p class="muted">Browser sign-in needs the login page, the cookie host and the cookie name.</p>
				{/if}
			</div>
		{/if}
	</div>

	<div class="sec">
		<div class="sec-name">Account</div>
		<input
			class="text-input"
			value={account}
			placeholder="@handle or email"
			autocomplete="off"
			onchange={(e) => void saveText("account", e.currentTarget.value)}
			onkeydown={blurOnEnter}
		/>
	</div>

	<div class="sec">
		<div class="sec-name">Signs in on</div>
		{#if machine}
			<p class="value"><a href="/app/object/{machine.id}">{keeperName}</a></p>
		{:else if servedBy}
			<p class="value">{keeperName}</p>
		{:else}
			<p class="muted">No computer yet - set Served by.</p>
		{/if}
		{#if auth}<p class="muted">{auth === "browser_profile" ? "Chrome sign-in" : auth === "api_key" ? "API keys" : auth.replaceAll("_", " ")}, carried by this credential: agents on any computer can use it, and anyone in this space can read it.</p>{/if}
	</div>

	{#if canAct}
		<div class="sec">
			<div class="sec-name">Actions</div>
			<div class="actions">
				{#if browserLogin}
					<button class="subtle-btn" disabled={!!busy} data-testid="credential-connect" onclick={() => void connect()}>{status === "active" || status === "needs_auth" ? "Reconnect" : "Connect"}</button>
				{/if}
				<button class="subtle-btn" disabled={!!busy} data-testid="credential-check" onclick={() => void act("check", "/credentials/check")}>Check now</button>
				<button class="subtle-btn" disabled={!!busy} data-testid="credential-disconnect" onclick={() => void disconnect()}>Disconnect</button>
			</div>
			{#if waiting}<p class="muted" role="status">Sign in in the Chrome window that just opened, then come back.</p>{/if}
		</div>
	{:else if browserLogin || !paired}
		<div class="sec">
			{#if browserLogin}<p class="muted">Open Roostr on {keeperName || "the computer in Served by"} to sign in - the sign-in window opens on that computer.</p>{/if}
			{#if !paired}<PairGate compact onready={() => void loadPairing()} />{/if}
		</div>
	{/if}

	{#if secretFields.length > 0}
		<form
			class="keys"
			onsubmit={(e) => {
				e.preventDefault();
				void saveKeys();
			}}
		>
			<div class="sec-name">{browserLogin ? "Or API keys" : "Keys"}</div>
			{#each secretFields as field (field.key)}
				<label class="cred-field">
					<span>{field.label}</span>
					<input type={field.secret ? "password" : "text"} autocomplete="off" value={draft[field.key] ?? ""} oninput={(e) => (draft[field.key] = e.currentTarget.value)} />
				</label>
			{/each}
			<div class="actions">
				<button type="submit" class="subtle-btn" disabled={!!busy || secretFields.some((f) => !(draft[f.key] ?? "").trim())}>{auth === "api_key" ? "Replace keys" : "Save keys"}</button>
			</div>
			<p class="muted">Saved on this credential. Anyone in this space can read them.</p>
		</form>
	{/if}

	{#if actionError || saveError}<p class="error" role="alert" data-testid="credential-error">{actionError || saveError}</p>{/if}
</section>

<style>
	.credential { display: flex; flex-direction: column; gap: 20px; margin: 16px 0 0 48px; max-width: 560px; }
	@media (max-width: 720px) { .credential { margin: 16px 16px 0; } }
	.headline { display: flex; flex-direction: column; gap: 4px; }
	.status-big { margin: 0; font-size: 22px; font-weight: 600; }
	.st-active { color: #30d158; }
	.st-needs { color: #ff9f0a; }
	.st-broken { color: #ff6961; }
	.st-missing { color: var(--muted); }
	.sec { display: flex; flex-direction: column; gap: 8px; }
	.sec-name { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }
	.value { margin: 0; font-size: 14px; color: var(--fg); }
	a { color: var(--fg); }
	a:hover { color: var(--accent); }
	.muted { margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; }
	.error { margin: 0; color: #ff6961; font-size: 13px; line-height: 1.5; white-space: pre-wrap; }
	.choices { display: flex; flex-direction: column; gap: 6px; }
	.choice { text-align: left; display: flex; flex-direction: column; gap: 2px; padding: 9px 12px; border-radius: 10px; border: 1px solid var(--border); background: none; color: var(--fg); cursor: pointer; font: inherit; }
	.choice:hover { border-color: var(--muted); }
	.choice-name { font-weight: 600; font-size: 14px; }
	.choice-sub { color: var(--muted); font-size: 12.5px; line-height: 1.4; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.subtle-btn { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 5px 12px; font: inherit; font-size: 12px; cursor: pointer; color: var(--muted); }
	.subtle-btn:hover:not(:disabled) { border-color: var(--accent); }
	.subtle-btn:disabled { opacity: 0.5; cursor: default; }
	.keys { display: flex; flex-direction: column; gap: 8px; }
	.text-input,
	.cred-field input:not([type="checkbox"]) { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 6px 10px; color: var(--fg); font: inherit; font-size: 14px; }
	.cred-field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); }
	.recipe { display: flex; flex-direction: column; gap: 10px; }
	.pair,
	.key-row { display: flex; gap: 6px; align-items: center; }
	.pair input,
	.key-row input:not([type="checkbox"]) { flex: 1; min-width: 0; }
	.secret-toggle { display: flex; gap: 4px; align-items: center; font-size: 12px; color: var(--muted); white-space: nowrap; cursor: pointer; }
	.choice:disabled { opacity: 0.5; cursor: default; }
</style>
