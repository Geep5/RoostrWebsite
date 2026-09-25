<script lang="ts">
	/**
	 * Featured properties render inline under the title as badges: a soft
	 * tint of the option's colour, a leading glyph that says what kind of
	 * state it is, the value as the label (name on hover). Click-to-edit in a
	 * popover. Order: the object's `featuredRelations` key list first, then
	 * the rest of the set fields. "+" appends a new property.
	 */
	import type { ObjectJSON, RelationDefJSON, ValueJSON } from "$lib/types";
	import { note } from "$lib/api";
	import { layoutOf, store } from "$lib/data.svelte";
	import { RESERVED_KEYS, emptyValueFor } from "$lib/relations";
	import { AGENTLESS_TYPES } from "$lib/agent-field";
	import { resolveServing, servingCopy, machineName, capabilityLabel, type MachineRow, type Serving } from "$lib/serving";
	import { fetchAllQuery } from "$lib/api";
	import PropertyValue from "./PropertyValue.svelte";
	import CheckboxIcon from "./CheckboxIcon.svelte";
	import { objectIcon } from "$lib/icons";
	import { badgeStyle, statusIcon, type BadgeIcon } from "$lib/options";
	import PropIcon from "./PropIcon.svelte";
	import { fetchBacklinks, type Backlink } from "$lib/backlinks";

	let {
		object,
		relations,
		onchanged,
	}: { object: ObjectJSON; relations: RelationDefJSON[]; onchanged: () => Promise<void> } = $props();

	const featuredKeys = $derived.by(() => {
		const items = object.fields["featuredRelations"]?.valuesValue?.items ?? [];
		return items.map((i) => i.stringValue).filter((s): s is string => typeof s === "string");
	});

	/**
	 * Present, editable properties: featured order first, then the rest. The
	 * guest list (`agent`) always shows on objects that can have one, even
	 * empty - it is how a person invites an agent here.
	 */
	const shown = $derived.by(() => {
		// served_by is a machine fact: it shows by default only on the objects
		// that are tied to a machine (agent, capability, install). agent /
		// requires / install stay always-on for objects that can take a guest.
		const MACHINE_BOUND = ["agent", "capability", "install"].includes(object.typeKey);
		const present = relations.filter((r) => {
			if (RESERVED_KEYS[r.key]) return false;
			if (r.key === "served_by") return MACHINE_BOUND || r.key in object.fields;
			if (["agent", "requires", "install"].includes(r.key)) return !AGENTLESS_TYPES[object.typeKey] || r.key in object.fields;
			return !r.hidden && r.key in object.fields;
		});
		const rank = new Map(featuredKeys.map((k, i) => [k, i]));
		return present.toSorted((a, b) => (rank.get(a.key) ?? 999) - (rank.get(b.key) ?? 999));
	});

	// ── Anytype's leading featured cells: object type + backlinks count ──
	const typeDef = $derived(store.types.find((t) => t.key === object.typeKey));
	const typeName = $derived(typeDef?.name || object.typeKey);

	// ── Serving (the retired chip): resolved once per object so the
	// served_by badge can name the machine and carry the warning, and
	// requires can label its keys. served_by/requires always show, even
	// empty - they are how a person moves this object or asks for more.
	let servingState = $state<{ serving: Serving; machines: MachineRow[] } | null>(null);
	$effect(() => {
		const current = object;
		void (async () => {
			try {
				servingState = await resolveServing(current);
			} catch {
				servingState = null;
			}
		})();
	});
	const serve = $derived.by(() => {
		if (!servingState) return null;
		const copy = servingCopy(servingState.serving, servingState.machines);
		return { text: copy.text, warning: copy.warning, machines: servingState.machines, requires: servingState.serving.requires };
	});

	// ── Credentials (the retired modal's login list): every install row in
	// the vault, keyed by machine, so an `install` property badge can read a
	// credential's live status on the machine that would run this object.
	let installs = $state<Map<string, Array<{ key: string; account: string; status: string; auth: string }>>>(new Map());
	let installsById = $state<Map<string, { key: string; account: string; status: string; auth: string; machine: string }>>(new Map());
	let capabilitiesById = $state<Map<string, { key: string; machine: string; machineName: string; status: string }>>(new Map());
	$effect(() => {
		void (async () => {
			try {
				const [rows, caps] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "capability" })]);
				const byMachine = new Map<string, Array<{ key: string; account: string; status: string; auth: string }>>();
				const byId = new Map<string, { key: string; account: string; status: string; auth: string; machine: string }>();
				for (const r of rows) {
					const m = r.fields["machine_id"]?.stringValue ?? "";
					const row = { key: r.fields["key"]?.stringValue ?? "", account: r.fields["account"]?.stringValue ?? "", status: r.fields["status"]?.stringValue ?? "", auth: r.fields["auth"]?.stringValue ?? "" };
					if (m) (byMachine.get(m) ?? byMachine.set(m, []).get(m)!).push(row);
					byId.set(r.id, { ...row, machine: m });
				}
				installs = byMachine;
				installsById = byId;
				const byCapId = new Map<string, { key: string; machine: string; machineName: string; status: string }>();
				for (const c of caps) {
					const m = c.fields["served_by"]?.linkValue?.targetId ?? c.fields["served_by"]?.stringValue ?? "";
					const instId = c.fields["install"]?.linkValue?.targetId ?? c.fields["install"]?.stringValue ?? "";
					const inst = byId.get(instId);
					byCapId.set(c.id, { key: c.fields["key"]?.stringValue ?? "", machine: m, machineName: m ? (servingState?.machines.find((x) => x.machineId === m)?.name ?? `${m.slice(0, 8)}…`) : "", status: inst?.status ?? "missing" });
				}
				capabilitiesById = byCapId;
			} catch { /* credentials are optional context */ }
		})();
	});
	/** The resolved machine's install rows, or every row when none is chosen. */
	const creds = $derived.by(() => {
		const chosen = servingState?.serving.machineId;
		if (chosen) return installs.get(chosen) ?? [];
		return [...installs.values()].flat();
	});
	const credState = (key: string, account: string): { status: string; auth: string } =>
		creds.find((c) => c.key === key && c.account === (account ?? "")) ?? { status: "missing", auth: "" };

	let backlinks = $state<Backlink[]>([]);
	let showBacklinks = $state(false);
	$effect(() => {
		const id = object.id;
		showBacklinks = false;
		void fetchBacklinks(id).then((b) => {
			if (object.id === id) backlinks = b;
		});
	});

	let editing = $state<string | null>(null);

	function plain(v: ValueJSON | undefined, format: string): string | number | boolean | string[] {
		if (!v) return format === "checkbox" ? false : format === "tag" || format === "object" ? [] : "";
		if (v.stringValue !== undefined) return v.stringValue;
		if (v.intValue !== undefined) return v.intValue;
		if (v.floatValue !== undefined) return v.floatValue;
		if (v.boolValue !== undefined) return v.boolValue;
		if (v.valuesValue) return v.valuesValue.items.map((i) => i.stringValue ?? i.linkValue?.targetId ?? "").filter(Boolean);
		if (v.listValue) return v.listValue.values;
		return "";
	}

	/** Compact display string for a cell, Anytype-style. */
	function display(rel: RelationDefJSON): string {
		const v = object.fields[rel.key];
		const p = plain(v, rel.format);
		if (rel.format === "checkbox") return p === true ? "✓" : "✗";
		if (rel.format === "date") {
			const ms = v?.intValue ?? v?.floatValue;
			return ms ? new Date(ms).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "";
		}
		if (rel.format === "object") {
			const ids = p as string[];
			return ids.map((id) => store.summaries.find((s) => s.id === id)?.name || store.agents.find((a) => a.id === id)?.name || id.slice(0, 6)).join(", ");
		}
		if (Array.isArray(p)) return p.join(", ");
		if (rel.format === "longtext") return String(p).slice(0, 60);
		return String(p);
	}

	async function saveValue(key: string, value: ValueJSON) {
		await note.setField(object.id, key, value);
		await onchanged();
	}

	/** Initialize a property so it appears (empty per-format default). */
	// ── New property (Anytype "create from scratch") ──────────────

	async function removeProp(key: string) {
		editing = null;
		await note.deleteField(object.id, key);
		await onchanged();
	}

	function closeAll() {
		editing = null;
	}

	/** Glyph + palette for a non-option property; options carry their own colour. Plain text has no glyph. */
	function badgeFor(rel: RelationDefJSON): { icon: BadgeIcon | null; color: string } {
		const v = object.fields[rel.key];
		switch (rel.format) {
			case "checkbox": return { icon: plain(v, "checkbox") === true ? "check" : "dashed", color: plain(v, "checkbox") === true ? "lime" : "" };
			case "date": {
				const ms = v?.intValue ?? v?.floatValue;
				const overdue = !!ms && ms < Date.now() && object.fields["done"]?.boolValue !== true && rel.key === "due_date";
				return { icon: "calendar", color: overdue ? "red" : "" };
			}
			case "url": return { icon: "link", color: "blue" };
			case "email": return { icon: "at", color: "blue" };
			case "phone": return { icon: "phone", color: "blue" };
			default: return { icon: null, color: "" };
		}
	}
</script>

{#if shown.length > 0 || typeName}
	<div class="featured">
		<span class="cell-wrap">
			<button
				class="badge"
				style={badgeStyle("")}
				title="Type"
				onclick={() => { if (typeDef) location.href = `/app/object/${typeDef.id}`; }}
			>{#if typeDef?.icon}<span class="emoji">{typeDef.icon}</span>{:else}<PropIcon icon="dot" />{/if}{typeName}</button>
		</span>
		{#if backlinks.length > 0}
			<span class="cell-wrap">
				<button class="badge" style={badgeStyle("")} title="Backlinks" onclick={() => { editing = null; showBacklinks = !showBacklinks; }}>
					<PropIcon icon="link" />{backlinks.length} backlink{backlinks.length === 1 ? "" : "s"}
				</button>
				{#if showBacklinks}
					<div class="pop">
						<div class="pop-head"><span class="pop-name">Linked from</span></div>
						{#each backlinks as b (b.id)}
							<a class="backlink" href="/app/object/{b.id}" onclick={() => (showBacklinks = false)}>
								<span class="bl-icon">{b.icon || "▨"}</span>{b.name}
								<span class="bl-kind">{b.typeKey}</span>
							</a>
						{/each}
					</div>
				{/if}
			</span>
		{/if}
		{#each shown as rel (rel.key)}
			{@const v = object.fields[rel.key]}
			{@const empty = display(rel) === ""}
			<span class="cell-wrap">
				{#if rel.format === "tag" && (plain(v, "tag") as string[]).length > 0}
					{#each plain(v, "tag") as string[] as t (t)}
						{@const opt = rel.options.find((o) => o.text === t)}
						<button class="badge" style={badgeStyle(opt?.color ?? "")} title={rel.name || rel.key} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
							<PropIcon icon="dot" />{t}
						</button>
					{/each}
				{:else if rel.format === "status" && !empty}
					{@const opt = rel.options.find((o) => o.text === display(rel))}
					<button class="badge" style={badgeStyle(opt?.color ?? "")} title={rel.name || rel.key} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
						<PropIcon icon={statusIcon(display(rel))} />{display(rel)}
					</button>
				{:else if rel.format === "checkbox"}
					{@const on = plain(v, "checkbox") === true}
					<button class="badge" style={badgeStyle(on ? "lime" : "")} title={rel.name || rel.key} onclick={() => void saveValue(rel.key, { boolValue: !on })}>
						<PropIcon icon={on ? "check" : "dashed"} />{rel.name || rel.key}
					</button>
				{:else if rel.key === "served_by"}
					{@const pinnedId = (plain(v, "object") as string[])[0] ?? ""}
					{@const warn = serve?.warning}
					<button class="badge" class:empty={!serve} class:plain={!pinnedId} style={warn ? badgeStyle("red") : badgeStyle("")} title={serve ? `Served by · ${serve.text}${warn ? " (cannot be honoured)" : ""}` : "Which machine serves this object"} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
						<span class="emoji">🖥️</span>{serve ? serve.text.replace(/^served by /, "") : "No machine yet"}
					</button>
				{:else if rel.key === "install"}
					{#if (plain(v, "object") as string[]).length > 0}
						{#each plain(v, "object") as string[] as id (id)}
							{@const row = installsById.get(id)}
							{@const live = row ? (servingState?.serving.machineId && row.machine && row.machine !== servingState.serving.machineId ? { ...row, status: "other machine" } : row) : null}
							{@const ok = live?.status === "active"}
							{@const warn = live && !ok}
							<button class="badge" style={badgeStyle(ok ? "lime" : warn ? "red" : "")} title={live ? `Credentials · ${live.key}${live.account ? ` (${live.account})` : ""} · ${live.status}${live.auth ? ` · ${live.auth}` : ""}` : "Credentials"} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
								<span class="emoji">🔌</span>{live ? `${live.key}${live.account ? ` · ${live.account}` : ""}${ok ? "" : ` (${live.status.replaceAll("_", " ")})`}` : id.slice(0, 8)}
							</button>
						{/each}
					{:else}
						<button class="badge empty plain" style={badgeStyle("")} title="Logins and accounts this object's work uses" onclick={() => (editing = editing === rel.key ? null : rel.key)}>
							<span class="emoji">🔌</span>No credentials
						</button>
					{/if}
				{:else if rel.key === "agent"}
					{#if (plain(v, "object") as string[]).length > 0}
						{#each plain(v, "object") as string[] as id (id)}
							{@const a = store.agents.find((x) => x.id === id)}
							<button class="badge" style={badgeStyle(a ? "blue" : "red")} title={a ? `${a.name} · agent` : `Agent ${id.slice(0, 8)}… (no longer exists — remove)`} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
								<span class="emoji">{a ? (a.icon || "🤖") : "⚠️"}</span>{a?.name || `${id.slice(0, 8)}…`}
							</button>
						{/each}
					{:else}
						<button class="badge empty plain" style={badgeStyle("")} title="Agents you can @-mention here" onclick={() => (editing = editing === rel.key ? null : rel.key)}>
							<span class="emoji">🤖</span>Add agent
						</button>
					{/if}
				{:else if rel.key === "requires"}
					{#if (plain(v, "object") as string[]).length > 0}
						{#each plain(v, "object") as string[] as id (id)}
							{@const cap = capabilitiesById.get(id)}
							{@const ok = cap?.status === "active" && !!cap?.machine}
							<button class="badge" style={badgeStyle(ok ? "lime" : cap ? "red" : "")} title={cap ? `Needs · ${cap.key} · ${cap.machine ? `${cap.machineName} · ` : ""}${cap.status ?? "missing install"}` : "Needs"} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
								<span class="emoji">🧩</span>{cap ? `${cap.key}${cap.machine ? ` · ${cap.machineName}` : ""}${ok ? "" : ` (${(cap.status ?? "not set up").replaceAll("_", " ")})`}` : id.slice(0, 8)}
							</button>
						{/each}
					{:else}
						<button class="badge empty plain" style={badgeStyle("")} title="Capabilities this object needs" onclick={() => (editing = editing === rel.key ? null : rel.key)}>
							<span class="emoji">🧩</span>Nothing needed
						</button>
					{/if}
				{:else if rel.format === "object" && (plain(v, "object") as string[]).length > 0}
					{#each plain(v, "object") as string[] as id (id)}
						{@const o = store.summaries.find((x) => x.id === id)}
						{@const a = o ? undefined : store.agents.find((x) => x.id === id)}
						<button class="badge" style={badgeStyle(a ? "blue" : "")} title={rel.name || rel.key} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
							{#if o && layoutOf(o.typeKey) === "task"}<span class="li-check" class:on={o.done === true}><CheckboxIcon checked={o.done === true} size={14} /></span>{:else}<span class="emoji">{a ? (a.icon || "🤖") : objectIcon(o?.icon, o?.typeKey ?? "")}</span>{/if}{o?.name || a?.name || "Untitled"}
						</button>
					{/each}
				{:else}
					{@const b = badgeFor(rel)}
					<button class="badge" class:empty class:plain={!b.icon} style={badgeStyle(b.color)} title={rel.name || rel.key} onclick={() => (editing = editing === rel.key ? null : rel.key)}>
						{#if b.icon}<PropIcon icon={b.icon} />{/if}{display(rel) || rel.name || rel.key}
					</button>
				{/if}
				{#if editing === rel.key}
					<div class="pop">
						<div class="pop-head">
							<span class="pop-name">{rel.name || rel.key}</span>
							{#if rel.key !== "done"}
								<button class="pop-rm" title="Remove property" onclick={() => void removeProp(rel.key)}>Remove</button>
							{/if}
						</div>
						<PropertyValue {rel} value={v} onsave={(nv) => void saveValue(rel.key, nv)} />
					</div>
				{/if}
			</span>
		{/each}
	</div>
	{#if editing}
		<button class="backdrop" aria-label="Close" onclick={closeAll}></button>
	{/if}
{/if}

<style>
	.badge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		padding: 0 10px 0 6px;
		border: none;
		border-radius: 8px;
		background: var(--badge-bg);
		color: var(--badge-fg);
		font: inherit;
		font-size: 13px;
		font-weight: 500;
		line-height: 1;
		white-space: nowrap;
		cursor: pointer;
		transition: filter 120ms;
	}
	.badge:hover {
		filter: brightness(1.18);
	}
	.badge.empty {
		opacity: 0.6;
		font-weight: 400;
	}
	.badge.plain {
		padding-left: 10px;
		font-weight: 400;
	}
	.emoji {
		font-size: 14px;
		line-height: 1;
		width: 16px;
		text-align: center;
	}
	.li-check {
		display: inline-flex;
		color: var(--badge-icon);
	}
	.li-check.on {
		color: var(--green);
	}
	.backlink {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 5px 8px;
		border-radius: 6px;
		color: var(--fg);
		text-decoration: none;
		font-size: 13px;
	}
	.backlink:hover {
		background: var(--hover);
	}
	.bl-icon {
		flex: none;
		width: 18px;
		text-align: center;
	}
	.bl-kind {
		margin-left: auto;
		color: var(--muted);
		font-size: 11px;
	}
	.featured {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 2px 0 14px 48px;
		font-size: 13px;
	}
	.cell-wrap {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.pop {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 90;
		min-width: 280px;
		max-width: 380px;
		/* No overflow clipping here: the object picker and calendar are
		   absolutely-positioned INSIDE - a scroll container amputated them
		   (the too-small-to-use modal). Long lists scroll themselves. */
		background: var(--panel, #1a1d23);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.pop-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
	}
	.pop-name {
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
	}
	.pop-rm {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 11px;
		cursor: pointer;
	}
	.pop-rm:hover {
		color: var(--red);
	}
	.pop :global(.opts) {
		max-height: 300px;
		overflow-y: auto;
	}
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 80;
		background: none;
		border: none;
		cursor: default;
	}
	@media (max-width: 720px) {
		.featured {
			margin: 0 16px 10px;
		}
	}
</style>
