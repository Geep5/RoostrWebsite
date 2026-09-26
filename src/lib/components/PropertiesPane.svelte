<script lang="ts">
	/**
	 * The right pane's properties section: the same properties FeaturedProps
	 * shows as badges under the title, laid out as vertical rows - a small
	 * icon + the property name on the left (muted), the value on the right.
	 * Clicking a row opens the property's PropertyValue editor in a popover
	 * anchored to the row; checkboxes toggle in place. Multi-value rows
	 * (tag/object/agent/install/requires) carry a hover × per value,
	 * single-value rows a hover × that removes the property.
	 */
	import type { ObjectJSON, RelationDefJSON, ValueJSON } from "$lib/types";
	import { note, fetchAllQuery } from "$lib/api";
	import { layoutOf, store } from "$lib/data.svelte";
	import { RESERVED_KEYS } from "$lib/relations";
	import { AGENTLESS_TYPES } from "$lib/agent-field";
	import { resolveServing, servingCopy, type MachineRow, type Serving } from "$lib/serving";
	import PropertyValue from "./PropertyValue.svelte";
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

	/** Same filter as FeaturedProps: featured order first, then the rest. */
	const shown = $derived.by(() => {
		const MACHINE_BOUND = ["agent", "capability", "install"].includes(object.typeKey);
		const AGENT_CONFIG = object.typeKey === "agent" ? ["prompt", "model", "responsible_types", "requires", "install", "served_by"] : [];
		const present = relations.filter((r) => {
			if (RESERVED_KEYS[r.key]) return false;
			if (AGENT_CONFIG.includes(r.key)) return true;
			if (r.key === "served_by") return MACHINE_BOUND || r.key in object.fields;
			if (["agent", "requires", "install"].includes(r.key)) return !AGENTLESS_TYPES[object.typeKey] || r.key in object.fields;
			if (["model", "responsible_types"].includes(r.key)) return object.typeKey === "agent" || r.key in object.fields;
			return !r.hidden && r.key in object.fields;
		});
		const rank = new Map(featuredKeys.map((k, i) => [k, i]));
		return present.toSorted((a, b) => (rank.get(a.key) ?? 999) - (rank.get(b.key) ?? 999));
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

	// ── Install + capability lookups so install/requires rows show live status ──
	let installsById = $state<Map<string, { key: string; account: string; status: string; auth: string; machine: string }>>(new Map());
	let capabilitiesById = $state<Map<string, { key: string; machine: string; machineName: string; status: string }>>(new Map());
	$effect(() => {
		void (async () => {
			try {
				const [rows, caps] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "capability" })]);
				const byId = new Map<string, { key: string; account: string; status: string; auth: string; machine: string }>();
				for (const r of rows) {
					const m = r.fields["machine_id"]?.stringValue ?? "";
					byId.set(r.id, {
						key: r.fields["key"]?.stringValue ?? "",
						account: r.fields["account"]?.stringValue ?? "",
						status: r.fields["status"]?.stringValue ?? "",
						auth: r.fields["auth"]?.stringValue ?? "",
						machine: m,
					});
				}
				installsById = byId;
				const byCapId = new Map<string, { key: string; machine: string; machineName: string; status: string }>();
				for (const c of caps) {
					const m = c.fields["served_by"]?.linkValue?.targetId ?? c.fields["served_by"]?.stringValue ?? "";
					const instId = c.fields["install"]?.linkValue?.targetId ?? c.fields["install"]?.stringValue ?? "";
					const inst = byId.get(instId);
					byCapId.set(c.id, {
						key: c.fields["key"]?.stringValue ?? "",
						machine: m,
						machineName: m ? (servingState?.machines.find((x) => x.machineId === m)?.name ?? `${m.slice(0, 8)}…`) : "",
						status: inst?.status ?? "missing",
					});
				}
				capabilitiesById = byCapId;
			} catch { /* credentials are optional context */ }
		})();
	});

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

	/** Compact display string, same as FeaturedProps' cells. */
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
		await note.setField(object.id, key, value);
		await onchanged();
	}

	/** Remove one value from a list-valued property, or the property when that was its last value. */
	async function removeValue(key: string, value: string) {
		editing = null;
		const items = (object.fields[key]?.valuesValue?.items ?? []).map((i) => i.stringValue ?? "");
		const next = items.filter((s) => s !== value);
		if (next.length > 0 && items.length > 1) await note.setField(object.id, key, { valuesValue: { items: next.map((s) => ({ stringValue: s })) } });
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

	/** Glyph + palette for a non-option property, as FeaturedProps. */
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
			case "install": return { emoji: "🔌" };
			case "agent": return { emoji: "🤖" };
			case "requires": return { emoji: "🧩" };
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

	/** Empty-state copy per row, matching FeaturedProps' empty badges. */
	function placeholderFor(rel: RelationDefJSON): string {
		switch (rel.key) {
			case "served_by": return "No machine yet";
			case "install": return "No credentials";
			case "agent": return "Add agent";
			case "requires": return "Nothing needed";
		}
		if (rel.format === "status") return "Select option";
		if (rel.format === "tag") return "Select options";
		if (rel.format === "date") return "Select a date";
		return "Empty";
	}
</script>

{#if shown.length > 0}
	<div class="props">
		{#each shown as rel (rel.key)}
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
							><CheckboxIcon checked={on} size={16} /></button>
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
						{:else if rel.key === "install"}
							{#each plain(v, "object") as string[] as id (id)}
								{@const row = installsById.get(id)}
								{@const live = row ? (servingState?.serving.machineId && row.machine && row.machine !== servingState.serving.machineId ? { ...row, status: "other machine" } : row) : null}
								{@const ok = live?.status === "active"}
								<span class="chip-wrap">
									<span class="chip" class:ok class:warn={!!live && !ok} title={live ? `${live.key}${live.account ? ` (${live.account})` : ""} · ${live.status}${live.auth ? ` · ${live.auth}` : ""}` : "Credentials"}>
										<span class="emoji">🔌</span>{live ? `${live.key}${live.account ? ` · ${live.account}` : ""}${ok ? "" : ` (${live.status.replaceAll("_", " ")})`}` : id.slice(0, 8)}
									</span>
									<button class="rm" aria-label={`Remove ${live?.key ?? "credential"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
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
						<PropertyValue {rel} value={v} onsave={(nv) => void saveValue(rel.key, nv)} />
					</div>
				{/if}
			</div>
		{/each}
	</div>
	{#if editing}
		<button class="backdrop" aria-label="Close" onclick={() => (editing = null)}></button>
	{/if}
{/if}

<style>
	.props {
		display: flex;
		flex-direction: column;
		font-size: 13px;
		padding: 4px 0;
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
	.li-check {
		display: inline-flex;
		color: var(--muted);
	}
	.li-check.on {
		color: var(--green);
	}
	.chk {
		display: inline-flex;
		border: none;
		background: none;
		padding: 0;
		cursor: pointer;
		color: var(--muted);
	}
	.chk.on {
		color: var(--green);
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
	/* Editor popover, same pattern as FeaturedProps. */
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
