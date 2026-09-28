<script lang="ts">
	/**
	 * The right pane's properties section: the object's properties
	 * laid out as vertical rows - a small
	 * icon + the property name on the left (muted), the value on the right.
	 * Clicking a row opens the property's PropertyValue editor in a popover
	 * anchored to the row; checkboxes toggle in place. Multi-value rows
	 * (tag/object/agent/credentials/requires) carry a hover × per value,
	 * single-value rows a hover × that removes the property.
	 */
	import { fieldStr, type ObjectJSON, type RelationDefJSON, type ValueJSON } from "$lib/types";
	import { note, fetchAllQuery, type QueryResultRow } from "$lib/api";
	import { layoutOf, store } from "$lib/data.svelte";
	import { RESERVED_KEYS } from "$lib/relations";
	import { AGENTLESS_TYPES } from "$lib/agent-field";
	import { machineName, resolveServing, servingCopy, type MachineRow, type Serving } from "$lib/serving";
	import PropertyValue from "./PropertyValue.svelte";
	import PropertySuggest from "./PropertySuggest.svelte";
	import Repeat from "./Repeat.svelte";
	import CheckboxIcon from "./CheckboxIcon.svelte";
	import { objectIcon } from "$lib/icons";
	import { badgeStyle, statusIcon, tagStyle, type BadgeIcon } from "$lib/options";
	import PropIcon from "./PropIcon.svelte";

	let {
		object,
		relations,
		onchanged,
	}: { object: ObjectJSON; relations: RelationDefJSON[]; onchanged: () => Promise<void> } = $props();

	const featuredKeys = $derived.by(() => {
		const items = object.fields["featuredRelations"]?.valuesValue?.items ?? [];
		return items.map((i) => i.stringValue).filter((s): s is string => typeof s === "string");
	});

	/** Agent-related properties group first (after any featured order): the
	    machine and the project folder on it, the agent, its config, then its
	    credentials and what it needs - the "how this object runs" block ahead
	    of ordinary fields. */
	const AGENT_PRIORITY = ["served_by", "repo_path", "agent", "model", "prompt", "requires", "credentials", "capability"];
	const agentRank = new Map(AGENT_PRIORITY.map((k, i) => [k, i]));

	/** A template edits the properties of the type it stamps out, so an
	    Agent template offers Served by, System prompt, Model… as defaults. */
	const typeKey = $derived.by(() => {
		if (object.typeKey !== "template") return object.typeKey;
		const target = object.fields["target_type"]?.stringValue ?? "";
		return store.types.find((t) => t.id === target)?.key ?? object.typeKey;
	});

	/** Featured order first, then the rest. */
	const shown = $derived.by(() => {
		const MACHINE_BOUND = ["agent", "capability", "install", "credential"].includes(typeKey);
		const AGENT_CONFIG = typeKey === "agent" ? ["prompt", "model", "requires", "credentials", "served_by", "repo_path"] : [];
		const present = relations.filter((r) => {
			if (RESERVED_KEYS[r.key]) return false;
			if (AGENT_CONFIG.includes(r.key)) return true;
			// The error badge is how the harness surfaces a problem on a
			// machine-bound object (no server, a holdup, a failed run); show it
			// there even before one is written, so its absence reads as "ok".
			// A space's one editable property is its guest list: who answers its chat.
			if (typeKey === "channel") return r.key === "agent";
			if (r.key === "error") return (MACHINE_BOUND && object.typeKey !== "template") || r.key in object.fields;
			if (r.key === "served_by") return MACHINE_BOUND || r.key in object.fields;
			if (["agent", "requires", "credentials"].includes(r.key)) return !AGENTLESS_TYPES[typeKey] || r.key in object.fields;
			if (r.key === "model") return typeKey === "agent" || r.key in object.fields;
			return !r.hidden && r.key in object.fields;
		});
		const rank = new Map(featuredKeys.map((k, i) => [k, i]));
		return present.toSorted((a, b) =>
			(rank.get(a.key) ?? 999) - (rank.get(b.key) ?? 999)
			|| (agentRank.get(a.key) ?? 999) - (agentRank.get(b.key) ?? 999));
	});

	// ── Serving: served_by names the machine and carries the warning ──
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
		return { text: copy.text, warning: copy.warning };
	});

	// ── Capability lookups so requires rows show live status (from the install each points at) ──
	let capabilitiesById = $state<Map<string, { key: string; machine: string; machineName: string; status: string }>>(new Map());
	$effect(() => {
		void (async () => {
			try {
				const [rows, caps] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "capability" })]);
				const installStatus = new Map(rows.map((r) => [r.id, r.fields["status"]?.stringValue ?? ""]));
				const byCapId = new Map<string, { key: string; machine: string; machineName: string; status: string }>();
				for (const c of caps) {
					const m = c.fields["served_by"]?.linkValue?.targetId ?? c.fields["served_by"]?.stringValue ?? "";
					const instId = c.fields["install"]?.linkValue?.targetId ?? c.fields["install"]?.stringValue ?? "";
					byCapId.set(c.id, {
						key: c.fields["key"]?.stringValue ?? "",
						machine: m,
						machineName: m ? (servingState?.machines.find((x) => x.machineId === m)?.name ?? `${m.slice(0, 8)}…`) : "",
						status: installStatus.get(instId) ?? "missing",
					});
				}
				capabilitiesById = byCapId;
			} catch { /* capability status is optional context */ }
		})();
	});

	// ── The credential objects the credentials row links, for their status and keeping computer ──
	const credentialIdsKey = $derived((plain(object.fields["credentials"], "object") as string[]).join(","));
	let credentialsById = $state<Map<string, QueryResultRow>>(new Map());
	$effect(() => {
		const ids = credentialIdsKey ? credentialIdsKey.split(",") : [];
		void (async () => {
			try {
				const rows = ids.length > 0 ? await fetchAllQuery({ filters: [{ key: "id", condition: "in", value: ids }] }) : [];
				credentialsById = new Map(rows.map((r) => [r.id, r]));
			} catch { /* credential status is optional context */ }
		})();
	});
	/** A linked credential's chip: its label and status colour. It carries its secret, so it works wherever this object's work runs. */
	function credentialChip(id: string): { label: string; tone: "ok" | "pending" | "warn" | ""; title: string } {
		const row = credentialsById.get(id);
		if (!row) return { label: store.summaries.find((s) => s.id === id)?.name || `${id.slice(0, 8)}…`, tone: "", title: "Credential" };
		const status = fieldStr(row.fields, "status") || "missing";
		return {
			label: fieldStr(row.fields, "name") || fieldStr(row.fields, "service") || "Credential",
			tone: status === "active" ? "ok" : status === "broken" ? "warn" : "pending",
			title: `${fieldStr(row.fields, "account") || fieldStr(row.fields, "service")} · ${status.replaceAll("_", " ")}`,
		};
	}

	let editing = $state<string | null>(null);

	function plain(v: ValueJSON | undefined, format: string): string | number | boolean | string[] {
		if (!v) return format === "checkbox" ? false : format === "tag" || format === "object" ? [] : "";
		if (v.stringValue !== undefined) return v.stringValue;
		if (v.intValue !== undefined) return v.intValue;
		if (v.floatValue !== undefined) return v.floatValue;
		if (v.boolValue !== undefined) return v.boolValue;
		if (v.valuesValue) return v.valuesValue.items.map((i) => i.stringValue ?? i.linkValue?.targetId ?? "").filter(Boolean);
		if (v.linkValue) return [v.linkValue.targetId ?? ""].filter(Boolean);
		if (v.listValue) return v.listValue.values;
		return "";
	}

	/** Compact display string for a row. */
	function display(rel: RelationDefJSON): string {
		const v = object.fields[rel.key];
		const p = plain(v, rel.format);
		if (rel.format === "checkbox") return p === true ? "✓" : "✗";
		if (rel.format === "date") {
			const ms = v?.intValue ?? v?.floatValue;
			return ms ? new Date(ms).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "";
		}
		if (rel.format === "object") {
			const ids = (Array.isArray(p) ? p : p ? [String(p)] : []).filter(Boolean);
			return ids.map((id) => store.summaries.find((s) => s.id === id)?.name || store.agents.find((a) => a.id === id)?.name || id.slice(0, 6)).join(", ");
		}
		if (Array.isArray(p)) return p.join(", ");
		if (rel.format === "longtext") return String(p).slice(0, 60);
		return String(p);
	}

	async function saveValue(key: string, value: ValueJSON) {
		// served_by is a pin to a machine_id string, not a link to the machine
		// object: the engine resolves the id against the roster, and a link to
		// the object never matches, so the harness would refuse to serve.
		if (key === "served_by") {
			const target = value.linkValue?.targetId ?? value.valuesValue?.items?.[0]?.linkValue?.targetId ?? "";
			if (target) {
				const machine = servingState?.machines.find((m) => m.id === target);
				value = machine ? { stringValue: machine.machineId } : value;
			}
		}
		await note.setField(object.id, key, value);
		await onchanged();
	}

	/** Remove one value from a list-valued property, or the property when that was its last value. Items keep their shape (string or link). */
	async function removeValue(key: string, value: string) {
		editing = null;
		const items = object.fields[key]?.valuesValue?.items ?? [];
		const next = items.filter((i) => (i.stringValue ?? i.linkValue?.targetId ?? "") !== value);
		if (next.length > 0 && items.length > 1) await note.setField(object.id, key, { valuesValue: { items: $state.snapshot(next) } });
		else await note.deleteField(object.id, key);
		await onchanged();
	}

	async function removeProp(key: string) {
		editing = null;
		await note.deleteField(object.id, key);
		await onchanged();
	}

	function toggleEdit(key: string) {
		editing = editing === key ? null : key;
	}

	function onRowKey(e: KeyboardEvent, key: string) {
		if (e.key === "Enter" || e.key === " ") {
			e.preventDefault();
			toggleEdit(key);
		}
	}

	/** Glyph + palette for a non-option property\. */
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

	/** The row's leading icon: an emoji for the machine-bound keys, a badge glyph otherwise. */
	function leftIcon(rel: RelationDefJSON): { emoji: string } | { icon: BadgeIcon; color: string } {
		switch (rel.key) {
			case "served_by": return { emoji: "🖥️" };
			case "repo_path": return { emoji: "📁" };
			case "credentials": return { emoji: "🔑" };
			case "agent": return { emoji: "🤖" };
			case "requires": return { emoji: "🧩" };
			case "prompt": return { emoji: "📜" };
			case "model": return { emoji: "🧬" };
		}
		if (rel.format === "status") {
			const d = display(rel);
			const opt = d ? rel.options.find((o) => o.text === d) : undefined;
			return { icon: d ? statusIcon(d) : "dot", color: opt?.color ?? "" };
		}
		if (rel.format === "object") return { icon: "link", color: "" };
		const b = badgeFor(rel);
		return { icon: b.icon ?? "dot", color: b.color };
	}

	/** Whether the row itself carries a hover × (single-value rows; multi rows put the × on each value). */
	function rowRemovable(rel: RelationDefJSON): boolean {
		if (rel.format === "tag" || rel.format === "object") {
			return rel.key === "served_by" && (plain(object.fields[rel.key], "object") as string[]).length > 0;
		}
		return display(rel) !== "";
	}

	/** Empty-state copy per row\. */
	function placeholderFor(rel: RelationDefJSON): string {
		switch (rel.key) {
			case "served_by": return "No machine yet";
			case "repo_path": return "No project folder";
			case "credentials": return "No credentials";
			case "agent": return "Add agent";
			case "requires": return "Nothing needed";
		}
		if (rel.format === "status") return "Select option";
		if (rel.format === "tag") return "Select options";
		if (rel.format === "date") return "Select a date";
		return "Empty";
	}

	/** ＋ Add property: pick an existing property to put on this object, or
	    create a new one - same surface as the object's right-click add. */
	let addPos = $state<{ x: number; y: number } | null>(null);

	/** The empty value for a format, so a new property appears as an empty row. */
	function emptyValue(format: string): ValueJSON {
		switch (format) {
			case "checkbox": return { boolValue: false };
			case "number": return { floatValue: 0 };
			case "date": return { intValue: 0 };
			case "tag":
			case "object":
			case "agent":
			case "credentials":
			case "requires": return { valuesValue: { items: [] } };
			default: return { stringValue: "" };
		}
	}

	async function addProperty(rel: RelationDefJSON) {
		addPos = null;
		await note.setField(object.id, rel.key, emptyValue(rel.format));
		await onchanged();
		// Open the new row's editor so the value can be set straight away.
		editing = rel.key;
	}

	/** Repeat is a property of the object too: it rows first, only for the
	    plain objects that can recur. */
	const canRepeat = $derived(!["channel", "chat", "type", "relation", "template", "query", "set", "collection", "agent"].includes(object.typeKey));

	// ── Grouped display: System / Agent / Custom, each a labeled section ──
	const AGENT_KEYS = new Set(["served_by", "repo_path", "agent", "model", "prompt", "requires", "credentials", "capability"]);
	const SYSTEM_KEYS = new Set(["done", "due_date", "status", "tag", "description", "url", "email", "phone", "error", "created_date", "modified_date", "createdDate", "modifiedDate"]);
	type Group = "system" | "agent" | "custom";
	const groupOf = (key: string): Group => (AGENT_KEYS.has(key) ? "agent" : SYSTEM_KEYS.has(key) ? "system" : "custom");
	const groups = $derived.by(() => {
		const out: Array<{ id: Group; label: string; rows: typeof shown }> = [];
		for (const [id, label] of [["system", "System"], ["agent", "Agent"], ["custom", "Custom"]] as Array<[Group, string]>) {
			const rows = shown.filter((r) => groupOf(r.key) === id);
			if (id === "system" && canRepeat) rows.unshift({ key: "__repeat__", name: "Repeat" } as unknown as (typeof shown)[number]);
			if (rows.length) out.push({ id, label, rows });
		}
		return out;
	});
</script>

{#snippet propRow(rel: (typeof shown)[number])}
	{#if rel.key === "__repeat__"}
		<div class="repeat-row">
			<Repeat {object} {onchanged} />
		</div>
	{:else}
	{@const v = object.fields[rel.key]}
	{@const li = leftIcon(rel)}
	<div class="row-wrap">
		<div
			class="row"
			role="button"
			tabindex="0"
			onclick={() => toggleEdit(rel.key)}
			onkeydown={(e) => onRowKey(e, rel.key)}
		>
			<span class="row-label">
				{#if "emoji" in li}
					<span class="emoji">{li.emoji}</span>
				{:else}
					<span class="row-icon" style={badgeStyle(li.color)}><PropIcon icon={li.icon} size={14} /></span>
				{/if}
				<span class="row-name">{rel.name || rel.key}</span>
			</span>
			<span class="row-value">
				{#if rel.format === "checkbox"}
					{@const on = plain(v, "checkbox") === true}
					<button
						class="chk"
						class:on
						aria-checked={on}
						role="checkbox"
						title={rel.name || rel.key}
						onclick={(e) => { e.stopPropagation(); void saveValue(rel.key, { boolValue: !on }); }}
					>{on ? "✓" : ""}</button>
				{:else if rel.format === "tag"}
					{#each plain(v, "tag") as string[] as t (t)}
						{@const opt = rel.options.find((o) => o.text === t)}
						<span class="chip-wrap">
							<span class="pill" style={tagStyle(opt?.color ?? "")}>{t}</span>
							<button class="rm" aria-label={`Remove ${t}`} title={`Remove ${t}`} onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, t); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else if rel.format === "status"}
					{@const d = display(rel)}
					{#if d}
						{@const opt = rel.options.find((o) => o.text === d)}
						<span class="status-val" style={badgeStyle(opt?.color ?? "")}>
							<PropIcon icon={statusIcon(d)} size={14} />{d}
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/if}
				{:else if rel.key === "served_by"}
					{#if serve}
						<span class="val-text" class:warn={serve.warning} title={serve.warning ? "Cannot be honoured" : ""}>{serve.text.replace(/^served by /, "")}</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/if}
				{:else if rel.key === "credentials"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const cred = credentialChip(id)}
						<span class="chip-wrap">
							<a class="chip" class:ok={cred.tone === "ok"} class:pending={cred.tone === "pending"} class:warn={cred.tone === "warn"} href="/app/object/{id}" title={cred.title} onclick={(e) => e.stopPropagation()}>
								<span class="emoji">🔑</span>{cred.label}
							</a>
							<button class="rm" aria-label={`Remove ${cred.label}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else if rel.key === "agent"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const a = store.agents.find((x) => x.id === id)}
						<span class="chip-wrap">
							<span class="chip" class:warn={!a} title={a ? `${a.name} · agent` : `Agent ${id.slice(0, 8)}… (no longer exists — remove)`}>
								<span class="emoji">{a ? (a.icon || "🤖") : "⚠️"}</span>{a?.name || `${id.slice(0, 8)}…`}
							</span>
							<button class="rm" aria-label={`Remove ${a?.name ?? "agent"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else if rel.key === "requires"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const cap = capabilitiesById.get(id)}
						{@const ok = cap?.status === "active" && !!cap?.machine}
						<span class="chip-wrap">
							<span class="chip" class:ok class:warn={!!cap && !ok} title={cap ? `${cap.key} · ${cap.machine ? `${cap.machineName} · ` : ""}${cap.status ?? "missing install"}` : "Needs"}>
								<span class="emoji">🧩</span>{cap ? `${cap.key}${cap.machine ? ` · ${cap.machineName}` : ""}${ok ? "" : ` (${(cap.status ?? "not set up").replaceAll("_", " ")})`}` : id.slice(0, 8)}
							</span>
							<button class="rm" aria-label={`Remove ${cap?.key ?? "capability"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else if rel.format === "object"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const o = store.summaries.find((x) => x.id === id)}
						{@const a = o ? undefined : store.agents.find((x) => x.id === id)}
						<span class="chip-wrap">
							<span class="chip">
								{#if o && layoutOf(o.typeKey) === "task"}
									<span class="li-check" class:on={o.done === true}><CheckboxIcon checked={o.done === true} size={13} /></span>
								{:else}
									<span class="emoji">{a ? (a.icon || "🤖") : objectIcon(o?.icon, o?.typeKey ?? "")}</span>
								{/if}{o?.name || a?.name || "Untitled"}
							</span>
							<button class="rm" aria-label={`Remove ${o?.name || a?.name || "link"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else}
					{@const d = display(rel)}
					{#if d}
						<span class="val-text">{d}</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/if}
				{/if}
			</span>
		</div>
		{#if rowRemovable(rel)}
			<button class="rm row-rm" aria-label={`Remove ${rel.name || rel.key}`} title="Remove property" onclick={(e) => { e.stopPropagation(); void removeProp(rel.key); }}>×</button>
		{/if}
		{#if editing === rel.key}
			<div class="pop">
				<div class="pop-head">
					<span class="pop-name">{rel.name || rel.key}</span>
					{#if rel.key !== "done"}
						<button class="pop-rm" title="Remove property" onclick={() => void removeProp(rel.key)}>Remove</button>
					{/if}
				</div>
				<PropertyValue {rel} value={v} spaceId={object.fields["channel"]?.stringValue ?? ""} onsave={(nv) => void saveValue(rel.key, nv)} />
			</div>
		{/if}
	</div>
	{/if}
{/snippet}

{#each groups as g (g.id)}
	<div class="prop-group">
		<span class="group-label">{g.label} Properties</span>
		{#each g.rows as rel (rel.key)}
			{@render propRow(rel)}
		{/each}
	</div>
{/each}
{#if editing}
	<button class="backdrop" aria-label="Close" onclick={() => (editing = null)}></button>
{/if}
<button
	class="add-prop"
	onclick={(e) => {
		const r = e.currentTarget.getBoundingClientRect();
		addPos = { x: r.left, y: r.bottom + 4 };
	}}
>＋ Add property</button>
{#if addPos}
	<PropertySuggest
		x={addPos.x}
		y={addPos.y}
		exclude={shown.map((r) => r.key)}
		onpick={(rel) => void addProperty(rel)}
		onclose={() => (addPos = null)}
	/>
{/if}

<style>
	.prop-group {
		display: flex;
		flex-direction: column;
		padding: 4px 0 6px;
	}
	.group-label {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--muted);
		padding: 4px 4px 2px;
	}
	.add-prop {
		display: block;
		width: 100%;
		background: none;
		border: none;
		border-radius: 6px;
		padding: 6px 4px;
		font-size: 12.5px;
		color: var(--muted);
		text-align: left;
		cursor: pointer;
	}
	.add-prop:hover {
		background: var(--hover);
		color: var(--fg);
	}
	.row-wrap {
		position: relative;
	}
	/* Inset divider: the line starts at the label text, not full-bleed. */
	.row-wrap:not(:first-child)::before {
		content: "";
		display: block;
		height: 1px;
		margin-left: 24px;
		background: var(--border);
		opacity: 0.5;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 10px;
		min-height: 28px;
		padding: 3px 22px 3px 4px;
		border-radius: 6px;
		cursor: pointer;
	}
	.row:hover {
		background: var(--hover);
	}
	.row-label {
		display: flex;
		align-items: center;
		gap: 7px;
		flex: none;
		min-width: 0;
		color: var(--muted);
	}
	.row-icon {
		display: inline-flex;
		width: 16px;
		justify-content: center;
		flex: none;
	}
	.row-name {
		font-size: 13px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.row-value {
		margin-left: auto;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		flex-wrap: wrap;
		gap: 4px 6px;
		min-width: 0;
		text-align: right;
	}
	.val-text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.val-text.warn {
		color: var(--red);
	}
	.placeholder {
		color: var(--muted);
		opacity: 0.65;
	}
	.emoji {
		font-size: 13px;
		line-height: 1;
		width: 16px;
		text-align: center;
		flex: none;
	}
	.pill {
		padding: 1px 8px;
		border-radius: 6px;
		font-size: 12px;
		line-height: 16px;
		white-space: nowrap;
	}
	.status-val {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		color: var(--badge-fg);
	}
	.chip-wrap {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 2px;
		min-width: 0;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.chip.ok {
		color: var(--green);
	}
	.chip.warn {
		color: var(--red);
	}
	a.chip {
		color: inherit;
		text-decoration: none;
	}
	a.chip:hover {
		text-decoration: underline;
	}
	.chip.pending {
		color: var(--orange);
	}
	.li-check {
		display: inline-flex;
		color: var(--muted);
	}
	.li-check.on {
		color: var(--green);
	}
	.chk {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border: 1px solid var(--border);
		border-radius: 4px;
		background: var(--panel);
		color: #fff;
		font-size: 11px;
		line-height: 1;
		padding: 0;
		cursor: pointer;
		flex: none;
	}
	.chk:hover {
		border-color: var(--accent);
	}
	.chk.on {
		background: var(--accent);
		border-color: var(--accent);
	}
	/* Hover ×: per value on multi rows, one on the row for single values. */
	.rm {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border: none;
		border-radius: 50%;
		background: none;
		color: var(--muted);
		font-size: 13px;
		line-height: 1;
		cursor: pointer;
		opacity: 0;
		transition: opacity 100ms;
		flex: none;
	}
	.chip-wrap:hover .rm,
	.row-wrap:hover .row-rm {
		opacity: 1;
	}
	.rm:hover {
		color: var(--red);
		background: var(--hover);
	}
	.row-rm {
		position: absolute;
		right: 2px;
		top: 50%;
		transform: translateY(-50%);
	}
	/* Editor popover, anchored under its row. */
	.pop {
		position: absolute;
		top: calc(100% + 4px);
		right: 0;
		z-index: 90;
		min-width: 280px;
		max-width: 380px;
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
</style>
