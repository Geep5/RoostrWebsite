<script lang="ts">
	/**
	 * Requirements checklist for one agent kind on one machine.
	 *
	 * Truth comes from the DAG: one `install` row per (descriptor × machine),
	 * the same rows the harness publishes (`publishInstallations`) and the
	 * machine modal renders. Actions are the same two-phase path Machine.svelte
	 * uses - a mailbox request to the installation object carries the intent,
	 * and a paired click on the owning machine grants execution - so nothing
	 * here can install or store a secret on its own. Secret values ride only
	 * in the paired approval body, never in the request or on an object.
	 */
	import { onMount } from "svelte";
	import { fetchAllQuery, mailbox, type QueryResultRow } from "$lib/api";
	import { harnessFetch, pairedSession } from "$lib/local-transport";
	import type { MachineRow } from "$lib/serving";
	import type { Card } from "$lib/cards";

	let {
		machine,
		requires,
		cards,
		localMachineId,
		satisfied = $bindable(false),
	}: {
		machine: MachineRow;
		requires: string[];
		cards: Card[];
		/** machine_id of the harness this tab is paired with; "" when unpaired. */
		localMachineId: string;
		satisfied: boolean;
	} = $props();

	interface CapabilityRequest {
		objectId: string;
		messageId: string;
		key: string;
		account: string;
		operation: string;
		status: string;
		error: string;
		canApprove: boolean;
		fields?: Array<{ key: string; label: string; secret: boolean }>;
	}
	interface Row {
		key: string;
		label: string;
		card?: Card;
		installId: string;
		/** InstallStatus from the harness: active | needs_auth | needs_approval | processing | missing | broken | disabled. */
		status: string;
		error: string;
		request?: CapabilityRequest;
	}

	let installs = $state<QueryResultRow[]>([]);
	let requests = $state<CapabilityRequest[]>([]);
	let credDraft = $state<Record<string, string>>({});
	let credOpen = $state("");
	let busy = $state("");
	let error = $state("");
	let notice = $state("");
	let loaded = $state(false);

	/** Only the owning machine's harness can approve; requests may be staged from anywhere. */
	const canApproveHere = $derived(!!localMachineId && localMachineId === machine.machineId);

	const rows = $derived.by((): Row[] =>
		requires.map((key) => {
			const card = cards.find((c) => c.key === key);
			const install = installs.find((r) => r.fields["key"]?.stringValue === key && (r.fields["account"]?.stringValue ?? "") === "");
			const installId = install?.id ?? "";
			const request = installId ? requests.find((q) => q.objectId === installId && ["pending", "awaiting_approval", "processing"].includes(q.status)) : undefined;
			return {
				key,
				label: card?.name ?? key,
				card,
				installId,
				status: install?.fields["status"]?.stringValue || "missing",
				error: install?.fields["error"]?.stringValue ?? "",
				request,
			};
		}),
	);

	$effect(() => {
		satisfied = loaded && rows.every((r) => r.status === "active");
	});

	async function load() {
		try {
			const [all, live] = await Promise.all([
				fetchAllQuery({ type: "install" }),
				canApproveHere && pairedSession() ? harnessFetch("/capability-requests").then(async (res) => (res.ok ? ((await res.json()) as { requests: CapabilityRequest[] }).requests : [])) : Promise.resolve([]),
			]);
			installs = all.filter((r) => r.fields["machine_id"]?.stringValue === machine.machineId);
			requests = live;
			loaded = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	onMount(() => {
		void load();
		// Install rows change as the owning harness runs approved work; the
		// checklist has to notice without a reload to be worth anything.
		const timer = setInterval(() => void load(), 2_000);
		return () => clearInterval(timer);
	});

	/** Mirrors Machine.svelte `requestOperation`: the DAG carries only the intent. */
	async function request(row: Row, operation: string): Promise<boolean> {
		if (busy) return false;
		busy = `${row.key}:${operation}`;
		error = "";
		notice = "";
		try {
			if (!row.installId) throw new Error(`${machine.name || "That machine"} has not published an installation row for ${row.label} yet.`);
			if (row.request) {
				notice = "A request for this is already waiting.";
				return true;
			}
			await mailbox.send({
				id: crypto.randomUUID(),
				exchangeId: crypto.randomUUID(),
				sender: { objectId: machine.id, agentId: "" },
				recipients: [{ objectId: row.installId, agentId: "" }],
				text: `Request ${operation} for ${row.key}.`,
				replyTo: "",
				sentAt: Date.now(),
				title: "Capability request",
				requestReply: true,
				historical: false,
				operation,
				author: "",
			});
			notice = canApproveHere ? "Request sent. Approve it below when it arrives." : `Request sent. Approve it in the machine panel on ${machine.name || "that machine"}.`;
			await load();
			return true;
		} catch (e) {
			error = e instanceof Error ? e.message : "Cannot stage the request.";
			return false;
		} finally {
			busy = "";
		}
	}

	/** Mirrors Machine.svelte `resolveRequest`: the paired click that grants execution. */
	async function resolve(row: Row, action: "approve" | "reject") {
		const req = row.request;
		if (!req) return;
		busy = req.messageId;
		error = "";
		try {
			const body: { objectId: string; messageId: string; fields?: Record<string, string> } = { objectId: req.objectId, messageId: req.messageId };
			if (action === "approve" && req.operation === "auth.save") {
				body.fields = Object.fromEntries((req.fields ?? []).map((f) => [f.key, credDraft[`${req.key}:${f.key}`] ?? ""]));
			}
			const res = await harnessFetch(`/capability-requests/${action}`, { method: "POST", body: JSON.stringify(body) });
			const result = (await res.json()) as { error?: string };
			if (!res.ok || result.error) throw new Error(result.error ?? `HTTP ${res.status}`);
			if (body.fields) for (const f of req.fields ?? []) delete credDraft[`${req.key}:${f.key}`];
			notice = "";
			window.dispatchEvent(new Event("roostr:machines-changed"));
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : "Approval failed.";
		} finally {
			busy = "";
		}
	}

	async function saveCredential(row: Row) {
		if (await request(row, "auth.save")) credOpen = "";
	}

	const inputType = (f: { secret: boolean; format: string }) => (f.secret || f.format === "password" ? "password" : f.format === "email" ? "email" : f.format === "url" ? "url" : "text");
	const statusText = (s: string) => (s === "active" ? "ready" : s === "needs_auth" ? "auth needed" : s === "needs_approval" ? "awaiting approval" : s === "processing" ? "working…" : s === "broken" ? "failed" : s === "disabled" ? "disabled" : "missing");
</script>

<div class="reqs" data-testid="setup-requirements">
	{#if requires.length === 0}
		<p class="hint">This kind needs nothing beyond the computer itself.</p>
	{/if}
	{#if error}<p class="err" role="alert" data-testid="setup-req-error">{error}</p>{/if}
	{#if notice}<p class="hint" role="status">{notice}</p>{/if}
	{#each rows as row (row.key)}
		{@const isSkill = row.card?.kind === "skill"}
		{@const hasKeys = !!row.card?.fields.length}
		{@const hasLogin = !!row.card?.install?.docsUrl}
		<div class="req" data-testid={`setup-req-${row.key}`}>
			<div class="req-row">
				<span class="req-name">{row.label}</span>
				<span class="chip {row.status}" data-testid={`setup-req-${row.key}-status`}>{statusText(row.status)}</span>
				<span class="gap"></span>
				{#if row.status !== "active" && !row.request}
					{#if isSkill}
						<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-request`} onclick={() => void request(row, row.status === "disabled" ? "skill.enable" : "skill.install")}>{row.status === "disabled" ? "Request enable" : "Request install"}</button>
					{/if}
					{#if hasKeys}
						<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-enter`} onclick={() => { credOpen = credOpen === row.key ? "" : row.key; error = ""; }}>Enter keys</button>
					{/if}
					{#if hasLogin}
						<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-login`} onclick={() => void request(row, "auth.login")}>Request login</button>
					{/if}
					{#if row.status === "needs_auth" || row.status === "broken"}
						<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-recheck`} onclick={() => void request(row, "auth.check")}>Re-check</button>
					{/if}
				{/if}
			</div>
			{#if row.card?.description}<p class="hint">{row.card.description}</p>{/if}
			{#if !row.card}<p class="hint">No card describes this key; the machine must publish it before it can be checked.</p>{/if}
			{#if row.error && row.status !== "active"}<p class="hint req-err">{row.error}</p>{/if}

			{#if credOpen === row.key && row.card?.fields.length}
				<div class="form">
					<p class="hint">Values go only to {machine.name || "that machine"}'s local store, never into an object.</p>
					{#each row.card.fields as f (f.key)}
						<label class="field">
							<span>{f.label}</span>
							<input
								type={inputType(f)}
								placeholder={f.note}
								autocomplete="off"
								data-testid={`setup-req-${row.key}-field-${f.key}`}
								value={credDraft[`${row.key}:${f.key}`] ?? ""}
								oninput={(e) => (credDraft = { ...credDraft, [`${row.key}:${f.key}`]: e.currentTarget.value })}
							/>
						</label>
					{/each}
					<div class="actions">
						<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-save`} onclick={() => void saveCredential(row)}>Request save to that machine</button>
						<button class="subtle" onclick={() => (credOpen = "")}>Cancel</button>
					</div>
				</div>
			{/if}

			{#if row.request}
				{@const req = row.request}
				<div class="pending" data-testid={`setup-req-${row.key}-pending`}>
					<span class="chip">{req.operation} · {req.status.replaceAll("_", " ")}</span>
					{#if req.error}<p class="hint">{req.error}</p>{/if}
					{#if canApproveHere && (req.status === "pending" || req.status === "awaiting_approval")}
						{#if req.operation === "auth.save" && req.canApprove}
							{#each req.fields ?? [] as f (f.key)}
								<label class="field">
									<span>{f.label}</span>
									<input type="password" autocomplete="off" data-testid={`setup-req-${row.key}-approve-field-${f.key}`} value={credDraft[`${req.key}:${f.key}`] ?? ""} oninput={(e) => (credDraft = { ...credDraft, [`${req.key}:${f.key}`]: e.currentTarget.value })} />
								</label>
							{/each}
						{/if}
						<div class="actions">
							<button class="subtle" disabled={!!busy || !req.canApprove} data-testid={`setup-req-${row.key}-approve`} onclick={() => void resolve(row, "approve")}>{req.operation === "auth.save" ? "Approve & save locally" : "Approve on this machine"}</button>
							<button class="subtle" disabled={!!busy} data-testid={`setup-req-${row.key}-reject`} onclick={() => void resolve(row, "reject")}>Reject</button>
						</div>
					{:else if !canApproveHere}
						<p class="hint">Waiting for approval in the machine panel on {machine.name || "that machine"}.</p>
					{/if}
				</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	.reqs { display: flex; flex-direction: column; gap: 10px; width: 100%; text-align: left; }
	.req { border: 1px solid #45454a; border-radius: 10px; padding: 12px 14px; display: flex; flex-direction: column; gap: 6px; }
	.req-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
	.req-name { font-weight: 600; }
	.gap { flex: 1; }
	.chip { font-size: 12px; padding: 2px 8px; border-radius: 999px; background: #3a3a3e; color: #d0d0d5; }
	.chip.active { background: #1f4d2b; color: #8fe0a2; }
	.chip.needs_auth, .chip.needs_approval, .chip.processing { background: #4d3d1f; color: #ffd48a; }
	.chip.broken { background: #4d1f1f; color: #ff9a94; }
	.hint { margin: 0; color: #98989d; font-size: 13px; line-height: 1.5; }
	.req-err { color: #ffb4ae; }
	.err { margin: 0; color: #ff6961; font-size: 13.5px; }
	.form, .pending { display: flex; flex-direction: column; gap: 8px; padding-top: 4px; }
	.field { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: #d0d0d5; }
	.field input { padding: 8px 10px; border-radius: 6px; border: 1px solid #45454a; background: #1e1e20; color: #f5f5f7; font-size: 14px; }
	.actions { display: flex; gap: 8px; flex-wrap: wrap; }
	.subtle { padding: 6px 12px; border-radius: 6px; border: 1px solid #45454a; background: #2b2b2e; color: #f5f5f7; font-size: 13px; cursor: pointer; }
	.subtle:hover { background: #3a3a3e; }
	.subtle:disabled { opacity: 0.5; cursor: default; }
</style>
