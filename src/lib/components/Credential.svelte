<script lang="ts">
	/**
	 * A credential object's page: one login for one service. The credential
	 * carries its secret (pasted keys in `secret`, a browser sign-in's
	 * cookies in `session`), so agents on any computer can use it - and
	 * anyone in its space can read it. The computer its Served by names
	 * opens the sign-in window and keeps the status true; Connect, Check and
	 * Disconnect run only when this tab is paired with that computer.
	 */
	import { onMount } from "svelte";
	import { fetchObject, note } from "$lib/api";
	import { thisMachineId } from "$lib/capability-actions";
	import { agoShort } from "$lib/conversations";
	import { harnessFetch, onPairingChange, pairedSession } from "$lib/local-transport";
	import { fetchMachines, machineName, servedByMachineId, type MachineRow } from "$lib/serving";
	import { fieldStr, type ObjectJSON } from "$lib/types";
	import PairGate from "./PairGate.svelte";

	interface ServiceField {
		key: string;
		label: string;
		secret: boolean;
	}
	interface Service {
		key: string;
		label: string;
		note: string;
		loginUrl?: string;
		fields?: ServiceField[];
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
	let services = $state<Service[]>([]);
	let servicesError = $state("");
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
	const account = $derived(fieldStr(object.fields, "account"));
	const status = $derived(fieldStr(object.fields, "status") || "missing");
	const error = $derived(fieldStr(object.fields, "error"));
	const auth = $derived(fieldStr(object.fields, "auth"));
	const checkedAt = $derived(object.fields["checked_at"]?.intValue ?? 0);
	const servedBy = $derived(servedByMachineId(object.fields, machines));
	const machine = $derived(machines.find((m) => m.machineId === servedBy));
	const keeperName = $derived(servedBy ? machineName(machines, servedBy) : "");
	const service = $derived(services.find((s) => s.key === serviceKey));
	const secretFields = $derived(service?.fields ?? []);
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

	async function loadServices() {
		if (!pairedSession()) return;
		try {
			const res = await harnessFetch("/credentials/services");
			if (!res.ok) throw new Error(`Cannot load services (HTTP ${res.status}).`);
			services = ((await res.json()) as { services: Service[] }).services;
			servicesError = "";
		} catch (e) {
			servicesError = e instanceof Error ? e.message : String(e);
		}
	}

	async function loadPairing() {
		paired = pairedSession() !== null;
		thisMachine = paired ? await thisMachineId() : "";
		await loadServices();
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

	async function setField(key: string, value: string) {
		saveError = "";
		try {
			await note.setField(id, key, { stringValue: value });
		} catch (e) {
			saveError = e instanceof Error ? e.message : String(e);
		}
	}

	async function pickService(picked: Service) {
		await setField("service", picked.key);
		const name = fieldStr(object.fields, "name").trim();
		if (!name || name === DEFAULT_NAME) await setField("name", picked.label);
		await onchanged();
	}

	async function saveAccount(value: string) {
		const next = value.trim();
		if (next === account) return;
		await setField("account", next);
		await onchanged();
	}

	// A different credential in the same page instance starts clean: no
	// connect poll or typed keys carry over, and none outlive the page.
	$effect(() => {
		void id;
		draft = {};
		actionError = "";
		return stopWaiting;
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

	<div class="sec">
		<div class="sec-name">Service</div>
		{#if serviceKey}
			<p class="value">{service?.label ?? serviceKey}</p>
			{#if service?.note}<p class="muted">{service.note}</p>{/if}
		{:else if paired && services.length > 0}
			<div class="choices">
				{#each services as s (s.key)}
					<button class="choice" data-testid={`credential-service-${s.key}`} onclick={() => void pickService(s)}>
						<span class="choice-name">{s.label}</span>
						{#if s.note}<span class="choice-sub">{s.note}</span>{/if}
					</button>
				{/each}
			</div>
		{:else}
			<p class="muted">No service chosen yet{paired ? "." : " - pair this tab with a Roostr computer to choose one."}</p>
		{/if}
		{#if servicesError && !serviceKey}<p class="error">{servicesError}</p>{/if}
	</div>

	<div class="sec">
		<div class="sec-name">Account</div>
		<input
			class="text-input"
			value={account}
			placeholder="@handle or email"
			autocomplete="off"
			onchange={(e) => void saveAccount(e.currentTarget.value)}
			onkeydown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); }}
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
				{#if service?.loginUrl}
					<button class="subtle-btn" disabled={!!busy} data-testid="credential-connect" onclick={() => void connect()}>{status === "active" || status === "needs_auth" ? "Reconnect" : "Connect"}</button>
				{/if}
				<button class="subtle-btn" disabled={!!busy} data-testid="credential-check" onclick={() => void act("check", "/credentials/check")}>Check now</button>
				<button class="subtle-btn" disabled={!!busy} data-testid="credential-disconnect" onclick={() => void disconnect()}>Disconnect</button>
			</div>
			{#if waiting}<p class="muted" role="status">Sign in in the Chrome window that just opened, then come back.</p>{/if}
		</div>
	{:else if service?.loginUrl || !paired}
		<div class="sec">
			{#if service?.loginUrl}<p class="muted">Open Roostr on {keeperName || "the computer in Served by"} to sign in - the sign-in window opens on that computer.</p>{/if}
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
			<div class="sec-name">{service?.loginUrl ? "Or API keys" : "Keys"}</div>
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
	.cred-field input { background: var(--bg); border: 1px solid var(--border); border-radius: 7px; padding: 6px 10px; color: var(--fg); font: inherit; font-size: 14px; }
	.cred-field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); }
</style>
