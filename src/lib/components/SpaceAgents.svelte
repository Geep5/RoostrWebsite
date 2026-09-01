<script lang="ts">
	/**
	 * Agents section of space settings. Agents are space infrastructure
	 * (like members), not objects OF the space — they're hidden from every
	 * object surface and managed only here. You TALK to an agent through its
	 * chat or any object's discussion; you MANAGE it here. Presence comes
	 * from the synced heartbeat fields; "runs here" is this machine's
	 * harness roster.
	 */
	import { onMount } from "svelte";
	import EmojiPicker from "./EmojiPicker.svelte";
	import { goto } from "$app/navigation";
	import { fetchQuery, note } from "$lib/api";
	import { store } from "$lib/data.svelte";

	let { channelId }: { channelId: string } = $props();

	const HARNESS = "http://127.0.0.1:7334";
	const ONLINE_MS = 300_000; // two missed 120s heartbeats

	interface AgentRow {
		id: string;
		name: string;
		icon: string;
		model: string;
		seenAt: number;
		host: string;
		/** Responsible type keys; "*" = everything else. */
		types: string[];
	}

	let agents = $state<AgentRow[]>([]);
	let roster = $state<string[] | null>(null); // null = no local daemon
	let now = $state(Date.now());
	let assigning = $state(""); // agent id whose responsibility editor is open
	let avatarPick = $state(""); // agent id whose avatar picker is open

	async function setAvatar(id: string, emoji: string) {
		avatarPick = "";
		await note.setField(id, "iconEmoji", { stringValue: emoji });
		await load();
	}

	const defaultChannelId = $derived(store.channels[0]?.id ?? "");

	async function load() {
		const res = await fetchQuery({ type: "agent", limit: 100 });
		agents = res.records
			.filter((r) => {
				if (r.fields["spawn_parent"]?.stringValue) return false; // subagents are ephemeral
				const ch = r.fields["channel"]?.stringValue ?? "";
				return ch === channelId || (ch === "" && channelId === defaultChannelId);
			})
			.map((r) => ({
				id: r.id,
				name: r.fields["name"]?.stringValue || "Agent",
				icon: r.fields["iconEmoji"]?.stringValue ?? "",
				model: r.fields["model"]?.stringValue ?? "",
				seenAt: r.fields["harness_seen_at"]?.intValue ?? 0,
				host: r.fields["harness_host"]?.stringValue ?? "",
				types: (r.fields["responsible_types"]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "").filter(Boolean),
			}));
	}

	// ── Responsibility (one owner per type; one "everything else") ──
	//
	// A type key is claimable by exactly ONE agent in the space, and only
	// one agent may hold "*" (whatever isn't explicitly claimed). The
	// checkboxes below enforce it: claimed elsewhere = disabled, with the
	// claimant named.

	/** typeKey → claiming agent (excluding `except`). */
	function claimants(except: string): Map<string, AgentRow> {
		const m = new Map<string, AgentRow>();
		for (const a of agents) {
			if (a.id === except) continue;
			for (const t of a.types) m.set(t, a);
		}
		return m;
	}

	async function saveTypes(a: AgentRow, next: string[]): Promise<void> {
		await note.setField(a.id, "responsible_types", { valuesValue: { items: next.map((t) => ({ stringValue: t })) } });
		await load();
	}

	async function toggleType(a: AgentRow, typeKey: string): Promise<void> {
		const has = a.types.includes(typeKey);
		// Claiming a specific type while holding "*" drops "*" only if
		// explicit types are chosen alongside — "*" plus explicit is
		// redundant; keep the model clean: explicit set XOR "*".
		const base = a.types.filter((t) => t !== "*");
		await saveTypes(a, has ? base.filter((t) => t !== typeKey) : [...base, typeKey]);
	}

	async function toggleRest(a: AgentRow): Promise<void> {
		await saveTypes(a, a.types.includes("*") ? [] : ["*"]);
	}

	function describe(a: AgentRow): string {
		if (a.types.includes("*")) return "Everything else";
		if (a.types.length === 0) return agents.length === 1 ? "Everything (sole agent)" : "Nothing assigned";
		return a.types.map((t) => store.types.find((x) => x.key === t)?.name ?? t).join(", ");
	}

	async function loadRoster() {
		try {
			const res = await fetch(`${HARNESS}/agents`);
			roster = ((await res.json()) as { roster: string[] }).roster;
		} catch {
			roster = null;
		}
	}

	// ── Provider readiness ───────────────────────────────────────────
	//
	// "Run here" only means anything if the harness on this machine can
	// authenticate the agent's model, so the button reflects its
	// credential state rather than letting the first turn discover it.
	interface ProviderStatus {
		mode: string;
		ready?: boolean;
		expires?: number;
	}
	let auth = $state<{ anthropic: ProviderStatus; kimi: ProviderStatus } | null>(null);

	async function loadAuth() {
		try {
			const res = await fetch(`${HARNESS}/auth/status`);
			auth = (await res.json()) as { anthropic: ProviderStatus; kimi: ProviderStatus };
		} catch {
			auth = null;
		}
	}

	/** Which credential a model needs — mirrors the harness's callLLM dispatch. */
	function provider(model: string): "anthropic" | "kimi" | "none" {
		if (model === "mock") return "none";
		return model.startsWith("kimi") ? "kimi" : "anthropic";
	}

	/** Empty when the agent can run there; otherwise why it can't. */
	function authBlock(model: string): string {
		const need = provider(model);
		if (need === "none" || auth === null) return "";
		const p = auth[need];
		// This app ships separately from the harness, so only an explicit
		// `ready: false` warns — an older daemon that omits the field must
		// not be reported as broken.
		if (p?.ready !== false) return "";
		const label = need === "anthropic" ? "Claude" : "Kimi";
		if (p.mode === "none") return `No ${label} credentials on that machine`;
		if (p.mode === "claude_code") return "Claude Code login expired";
		return `${label} credentials expired`;
	}

	onMount(() => {
		void loadRoster();
		void loadAuth();
		// The same tick that ages the presence labels re-checks credentials,
		// so an expiry that lands while this page is open shows up.
		const t = setInterval(() => {
			now = Date.now();
			void loadAuth();
		}, 15_000);
		return () => clearInterval(t);
	});

	$effect(() => {
		void channelId;
		void load();
	});

	async function toggle(id: string, enabled: boolean) {
		await fetch(`${HARNESS}/agents/toggle`, { method: "POST", body: JSON.stringify({ id, enabled }) });
		await loadRoster();
	}

	async function openChat(id: string) {
		const res = await fetchQuery({
			filters: [
				{ key: "type", condition: "equal", value: "chat" },
				{ key: "agent", condition: "equal", value: id },
			],
			limit: 1,
		});
		const chat = res.records[0];
		if (chat) await goto(`/app/object/${chat.id}`);
		else alert("No chat yet — enable the agent on a machine running the harness first.");
	}

	let confirmRemove = $state("");

	/** Tombstone the agent and its chats. Two-click confirm. */
	async function removeAgent(a: AgentRow) {
		if (confirmRemove !== a.id) {
			confirmRemove = a.id;
			setTimeout(() => (confirmRemove = ""), 4000);
			return;
		}
		confirmRemove = "";
		if (roster?.includes(a.id)) await toggle(a.id, false);
		const chats = await fetchQuery({
			filters: [
				{ key: "type", condition: "equal", value: "chat" },
				{ key: "agent", condition: "equal", value: a.id },
			],
			limit: 10,
		});
		for (const c of chats.records) await note.del(c.id);
		await note.del(a.id);
		await load();
	}

	async function newAgent() {
		const name = prompt("Agent name:");
		if (!name?.trim()) return;
		await note.create(name.trim(), "agent", {
			channel: { stringValue: channelId },
			model: { stringValue: "claude-sonnet-4-5" },
		});
		await load();
	}

	function ago(ms: number): string {
		const s = Math.floor((now - ms) / 1000);
		if (s < 90) return `${s}s ago`;
		if (s < 5400) return `${Math.round(s / 60)}m ago`;
		return `${Math.round(s / 3600)}h ago`;
	}
</script>

<h3>Agents</h3>
<p class="hint">
	Agents serve this space: one holistic chat each, reachable from any object's discussion. Enable an
	agent on the machine whose harness should run it.
</p>

{#each agents as a (a.id)}
	{@const online = a.seenAt > 0 && now - a.seenAt < ONLINE_MS}
	{@const runsHere = roster?.includes(a.id) ?? false}
	{@const blocked = roster === null ? "" : authBlock(a.model)}
	<div class="agent-wrap">
		<div class="agent">
			<span class="dot" class:online></span>
			<!-- The agent's avatar: shows in chat next to its messages. -->
			<button class="avatar-btn" title="Set avatar" onclick={() => (avatarPick = avatarPick === a.id ? "" : a.id)}>
				{#if a.icon}{a.icon}{:else}🤖{/if}
			</button>
			<span class="name">{a.name}</span>
			<span class="meta">
				{#if online}{a.host || "online"} · {ago(a.seenAt)}{:else if a.seenAt > 0}last seen {ago(a.seenAt)}{:else}never ran{/if}
			</span>
			<button class="resp" class:unset={a.types.length === 0 && agents.length > 1} onclick={() => (assigning = assigning === a.id ? "" : a.id)}>
				{describe(a)}
			</button>
			<button onclick={() => void openChat(a.id)}>💬 Chat</button>
			{#if roster !== null}
				<button
					class:active={runsHere}
					class:warn={blocked !== ""}
					title={blocked ? `${blocked} — this agent's turns will fail until you sign in` : ""}
					onclick={() => void toggle(a.id, !runsHere)}
				>
					{blocked ? "⚠ " : ""}{runsHere ? "Runs here" : "Run here"}
				</button>
			{/if}
			<button class="danger" onclick={() => void removeAgent(a)}>
				{confirmRemove === a.id ? "Confirm remove" : "Remove"}
			</button>
		</div>
		{#if blocked}
			<p class="auth-warn">
				⚠ {blocked}. {runsHere
					? "This agent runs there but its turns will fail"
					: "Running it there will fail"} until you sign in under Settings → Agent.
			</p>
		{/if}
		{#if avatarPick === a.id}
			<div class="avatar-pop">
				<EmojiPicker onpick={(e) => void setAvatar(a.id, e)} onclose={() => (avatarPick = "")} />
			</div>
		{/if}
		{#if assigning === a.id}
			{@const claimed = claimants(a.id)}
			{@const restHolder = [...claimed.entries()].find(([k]) => k === "*")?.[1]}
			<div class="assign">
				<div class="assign-title">Responsible for</div>
				<label class="opt" class:disabled={!!restHolder}>
					<input type="checkbox" checked={a.types.includes("*")} disabled={!!restHolder} onchange={() => void toggleRest(a)} />
					Everything else <span class="opt-hint">{restHolder ? `— ${restHolder.name} has it` : "(whatever isn't explicitly assigned)"}</span>
				</label>
				<div class="assign-sep"></div>
				{#each store.types as t (t.id)}
					{@const owner = claimed.get(t.key)}
					<label class="opt" class:disabled={!!owner || a.types.includes("*")}>
						<input
							type="checkbox"
							checked={a.types.includes(t.key)}
							disabled={!!owner || a.types.includes("*")}
							onchange={() => void toggleType(a, t.key)}
						/>
						{t.name}
						{#if owner}<span class="opt-hint">— {owner.name}</span>{/if}
					</label>
				{/each}
			</div>
		{/if}
	</div>
{/each}
{#if agents.length === 0}
	<p class="hint">No agents in this space yet.</p>
{/if}

<button class="new-agent" onclick={() => void newAgent()}>＋ New agent</button>

<style>
	h3 {
		font-size: 13px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
		margin: 24px 0 6px;
	}
	.hint {
		color: var(--muted);
		font-size: 12px;
		margin: 0 0 10px;
	}
	.agent-wrap {
		border: 1px solid var(--border);
		border-radius: 10px;
		margin-bottom: 6px;
	}
	.agent {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 10px;
		font-size: 13px;
	}
	.resp {
		max-width: 240px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		color: var(--muted);
	}
	.resp.unset {
		color: var(--accent);
		border-color: var(--accent);
	}
	.assign {
		border-top: 1px solid var(--border);
		padding: 8px 12px 10px;
	}
	.assign-title {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
		margin-bottom: 6px;
	}
	.assign-sep {
		height: 1px;
		background: var(--border);
		margin: 6px 0;
	}
	.opt {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		padding: 3px 0;
		cursor: pointer;
	}
	.opt.disabled {
		opacity: 0.45;
		cursor: default;
	}
	.opt-hint {
		color: var(--muted);
		font-size: 11px;
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--muted);
		flex: none;
	}
	.dot.online {
		background: #4caf78;
	}
	.name {
		color: var(--fg);
		font-weight: 600;
	}
	button.danger:hover {
		border-color: #e05555;
		color: #e05555;
	}
	.meta {
		color: var(--muted);
		font-size: 12px;
		flex: 1;
	}
	button {
		background: none;
		border: 1px solid var(--border);
		border-radius: 8px;
		color: var(--fg);
		font-size: 12px;
		padding: 4px 10px;
		cursor: pointer;
	}
	button:hover {
		border-color: var(--accent);
	}
	button.active {
		border-color: var(--accent);
		color: var(--accent);
	}
	/* After .active so a warned agent that already runs there reads as a
	   warning, not as healthy. */
	button.warn {
		border-color: var(--orange);
		color: var(--orange);
		background: rgb(255 159 10 / 0.12);
	}
	.auth-warn {
		color: var(--orange);
		font-size: 11.5px;
		margin: 2px 0 8px 30px;
	}
	.new-agent {
		margin-top: 2px;
	}
	.avatar-btn {
		background: var(--hl-light, rgba(255, 255, 255, 0.06));
		border: 1px solid transparent;
		border-radius: 50%;
		width: 28px;
		height: 28px;
		font-size: 16px;
		line-height: 1;
		cursor: pointer;
		padding: 0;
		flex: none;
	}
	.avatar-btn:hover {
		border-color: var(--accent);
	}
	.avatar-pop {
		position: relative;
		margin: 4px 0 8px 24px;
	}
</style>
