<script lang="ts">
	/**
	 * The right pane's properties section: the object's properties
	 * laid out as vertical rows - a small
	 * icon + the property name on the left (muted), the value on the right.
	 * Clicking a row opens the property's PropertyValue editor in a popover
	 * anchored to the row; checkboxes toggle in place. Multi-value rows
	 * (tag/object/agent/credentials/skills/tools) carry a hover × per value,
	 * single-value rows a hover × that removes the property. A built-in
	 * Tool (the harness writes it) and read-only properties show, never edit.
	 */
	import { fieldStr, isBuiltinTool, isLockedTool, repeatOf, type ObjectJSON, type RelationDefJSON, type ValueJSON } from "$lib/types";
	import { note, fetchAllQuery, type QueryResultRow } from "$lib/api";
	import { layoutOf, store } from "$lib/data.svelte";
	import { RESERVED_KEYS } from "$lib/relations";
	import { AGENTLESS_TYPES } from "$lib/agent-field";
	import { machineName, resolveServing, servedByMachineId, servingCopy, type MachineRow, type Serving } from "$lib/serving";
	import { credentialStatusBadge, credentialStatusText, pollWhileConnecting } from "$lib/credential-actions";
	import { capabilityStatusBadge, capabilityStatusText } from "$lib/capability-actions";
	import CredentialStatus from "./CredentialStatus.svelte";
	import CapabilityStatus from "./CapabilityStatus.svelte";
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
	const AGENT_PRIORITY = ["served_by", "repo_path", "agent", "model", "prompt", "skills", "tools", "credentials", "capability"];
	const agentRank = new Map(AGENT_PRIORITY.map((k, i) => [k, i]));

	/** A template edits the properties of the type it stamps out, so an
	    Agent template offers Served by, System prompt, Model… as defaults. */
	const typeKey = $derived.by(() => {
		if (object.typeKey !== "template") return object.typeKey;
		const target = object.fields["target_type"]?.stringValue ?? "";
		return store.types.find((t) => t.id === target)?.key ?? object.typeKey;
	});

	/** A credential's / capability's own properties, always shown in this
	    order (a credential's then each `key_*` field it carries, one pasted
	    key each). The harness seeds their defs in every space; these names
	    and emoji stand in until it has. */
	const CREDENTIAL_PROPS: Array<[key: string, name: string, format: string, emoji: string]> = [
		["status", "Status", "status", ""],
		["error", "Error", "longtext", ""],
		["served_by", "Served by", "object", ""],
		["account", "Account", "shorttext", "🪪"],
		["service", "Service", "shorttext", "🧩"],
		["description", "Description", "longtext", ""],
		["login_url", "Login page", "url", "🔗"],
		["session_host", "Signed-in host", "shorttext", "🌐"],
		["session_cookie", "Signed-in cookie", "shorttext", "🍪"],
	];
	const CAPABILITY_PROPS: Array<[key: string, name: string, format: string, emoji: string]> = [
		["status", "Status", "status", ""],
		["error", "Error", "longtext", ""],
		["served_by", "Served by", "object", ""],
		["key", "Key", "shorttext", "🧩"],
		["checked_at", "Checked", "date", ""],
		["description", "Description", "longtext", ""],
	];
	const KEY_PREFIX = "key_";
	const isCredential = $derived(typeKey === "credential");
	const isCapability = $derived(typeKey === "capability");
	/** The serving computer's harness writes these: they show, never edit. */
	const OWN_READONLY = ["served_by", "key", "checked_at"];
	const ownRows = $derived.by((): RelationDefJSON[] => {
		if (!isCredential && !isCapability) return [];
		const def = (key: string, name: string, format: string, emoji: string): RelationDefJSON => {
			const rel = relations.find((r) => r.key === key)
				?? { id: `${typeKey}-${key}`, key, format, name, iconEmoji: emoji || undefined, hidden: false, readOnly: false, maxCount: format === "status" ? 1 : 0, options: [] };
			return isCapability && OWN_READONLY.includes(key) ? { ...rel, readOnly: true } : rel;
		};
		if (isCapability) return CAPABILITY_PROPS.map(([key, name, format, emoji]) => def(key, name, format, emoji));
		// `key_fields` is the pre-property recipe list the harness converts, not a pasted key.
		const keyFields = Object.keys(object.fields).filter((k) => k.startsWith(KEY_PREFIX) && k !== "key_fields");
		return [
			...CREDENTIAL_PROPS.map(([key, name, format, emoji]) => def(key, name, format, emoji)),
			...keyFields.map((key) => {
				const words = key.slice(KEY_PREFIX.length).replaceAll("_", " ");
				return def(key, words.charAt(0).toUpperCase() + words.slice(1), "shorttext", "🔑");
			}),
		];
	});
	const ownRowKeys = $derived(new Set(ownRows.map((r) => r.key)));

	const capabilityStatus = $derived(fieldStr(object.fields, "status") || "missing");

	/** Featured order first, then the rest. */
	const shown = $derived.by(() => {
		const MACHINE_BOUND = ["agent", "capability", "credential"].includes(typeKey);
		const AGENT_CONFIG = typeKey === "agent" ? ["prompt", "model", "skills", "tools", "credentials", "served_by", "repo_path"] : [];
		// A Tool's own: what the model is told, its inputs, whether it ships with Roostr, and which version of its code runs.
		const TOOL_CONFIG = typeKey === "tool" ? ["description", "tool_inputs", "tool_builtin", "tool_version"] : [];
		const repeating = !!repeatOf(object.fields);
		const present = relations.filter((r) => {
			// Legacy credential shapes (`key_fields` list, `secret` JSON) stay for old harnesses; never rows.
			if (ownRowKeys.has(r.key) || (isCredential && (r.key === "key_fields" || r.key === "secret"))) return false;
			if (RESERVED_KEYS[r.key]) return false;
			if (AGENT_CONFIG.includes(r.key) || TOOL_CONFIG.includes(r.key)) return true;
			// The error badge is how problems surface on any object (a failed
			// run, a check that failed, a tool that didn't load, a holdup):
			// shown everywhere even before one is written, so its absence reads as "ok".
			// A template's own Error is never copied, so it is no default to set.
			if (r.key === "error") return object.typeKey !== "template";
			// A space's one editable property is its guest list: who answers its chat.
			if (typeKey === "channel") return r.key === "agent";
			// A computer's "Keep every file" switch shows, unticked, before it is ever set.
			if (r.key === "keep_all_files") return typeKey === "machine" || r.key in object.fields;
			if (r.key === "served_by") return MACHINE_BOUND || r.key in object.fields;
			// Credentials, Skills and Tools are an agent's own (AGENT_CONFIG); elsewhere only when set.
			if (r.key === "credentials" || r.key === "skills" || r.key === "tools") return r.key in object.fields;
			// Check first belongs to the repeat: the tool run before each occurrence.
			if (r.key === "check_first") return repeating || r.key in object.fields;
			if (r.key === "agent") return !AGENTLESS_TYPES[typeKey] || r.key in object.fields;
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

	// ── Skill lookups so Skills rows say where each machine skill works ──
	let skillsById = $state<Map<string, { name: string; key: string; machines: string[] }>>(new Map());
	$effect(() => {
		void (async () => {
			try {
				const [skills, caps] = await Promise.all([fetchAllQuery({ type: "skill" }), fetchAllQuery({ type: "capability" })]);
				// key -> names of the machines that have it working
				const working = new Map<string, string[]>();
				for (const c of caps) {
					const m = servedByMachineId(c.fields, servingState?.machines ?? []);
					const key = c.fields["key"]?.stringValue ?? "";
					if (!m || !key || c.fields["status"]?.stringValue !== "active") continue;
					const name = machineName(servingState?.machines ?? [], m);
					working.set(key, [...new Set([...(working.get(key) ?? []), name])]);
				}
				skillsById = new Map(skills.map((sk) => {
					const key = sk.fields["key"]?.stringValue ?? "";
					return [sk.id, { name: sk.fields["name"]?.stringValue || key || "Skill", key, machines: key ? (working.get(key) ?? []) : [] }];
				}));
			} catch { /* skill status is optional context */ }
		})();
	});

	// ── The Tool objects the tools row links, so a broken one (its `error`) shows as such ──
	const toolIdsKey = $derived((plain(object.fields["tools"], "object") as string[]).join(","));
	let toolsById = $state<Map<string, { name: string; error: string; builtin: boolean }>>(new Map());
	$effect(() => {
		const ids = toolIdsKey ? toolIdsKey.split(",") : [];
		void (async () => {
			try {
				const rows = ids.length > 0 ? await fetchAllQuery({ filters: [{ key: "id", condition: "in", value: ids }] }) : [];
				toolsById = new Map(rows.map((r) => [r.id, { name: fieldStr(r.fields, "name") || "Tool", error: fieldStr(r.fields, "error"), builtin: isBuiltinTool(r.fields) }]));
			} catch { /* tool status is optional context */ }
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

	// While a credential's sign-in window is open its computer writes the status: follow it.
	const credentialStatus = $derived(isCredential ? fieldStr(object.fields, "status") : "");
	const objectId = $derived(object.id);
	let connectPollError = $state("");
	$effect(() => {
		if (credentialStatus !== "connecting") return;
		connectPollError = "";
		return pollWhileConnecting(objectId, () => object.fields, onchanged, () => {
			connectPollError = "Still not signed in after 5 minutes. Connect again when you are ready.";
		});
	});

	function plain(v: ValueJSON | undefined, format: string): string | number | boolean | string[] {
		if (!v) return format === "checkbox" ? false : format === "tag" || format === "object" ? [] : "";
		// A list property holding a single plain string (an id or tag written
		// before it became a list) is one item - never iterated as characters.
		if (v.stringValue !== undefined) return format === "tag" || format === "object" ? [v.stringValue].filter(Boolean) : v.stringValue;
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

	/** A built-in Tool an older harness wrote from its own handler is that harness's to write: nothing on it edits here, nor does a read-only property anywhere. */
	const locked = $derived(isLockedTool(object.fields));
	const editable = (rel: RelationDefJSON) => !locked && !rel.readOnly;

	function toggleEdit(rel: RelationDefJSON) {
		if (!editable(rel)) return;
		editing = editing === rel.key ? null : rel.key;
	}

	function onRowKey(e: KeyboardEvent, rel: RelationDefJSON) {
		if (e.key === "Enter" || e.key === " ") {
			e.preventDefault();
			toggleEdit(rel);
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

	/** The row's leading icon: the property's own emoji; a badge glyph only for properties that have none. */
	function leftIcon(rel: RelationDefJSON): { emoji: string } | { icon: BadgeIcon; color: string } {
		// A property's own emoji is its icon, here as in table headers and the
		// property picker; its current state belongs to the value on the right.
		if (rel.iconEmoji) return { emoji: rel.iconEmoji };
		switch (rel.key) {
			case "served_by": return { emoji: "🖥️" };
			case "repo_path": return { emoji: "📁" };
			case "credentials": return { emoji: "🔑" };
			case "agent": return { emoji: "🤖" };
			case "skills": return { emoji: "🛠️" };
			case "tools": return { emoji: "🧰" };
			case "check_first": return { emoji: "🔎" };
			case "prompt": return { emoji: "🧠" };
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
		// A credential's or capability's status is its computer's to write.
		if ((isCredential || isCapability) && rel.key === "status") return false;
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
			case "skills": return "No skills";
			case "tools": return "No tools";
			case "check_first": return "None";
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
			case "skills": return { valuesValue: { items: [] } };
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
	    plain objects that can recur. A Tool runs when called, never on a schedule. */
	const canRepeat = $derived(!["channel", "chat", "type", "relation", "template", "query", "set", "collection", "agent", "tool"].includes(object.typeKey));

	// ── Grouped display: System / Tool / Agent / Custom, each a labeled section ──
	const AGENT_KEYS = new Set(["served_by", "repo_path", "agent", "model", "prompt", "skills", "tools", "credentials", "capability"]);
	const SYSTEM_KEYS = new Set(["done", "due_date", "status", "tag", "description", "url", "email", "phone", "error", "created_date", "modified_date", "createdDate", "modifiedDate", "check_first"]);
	const TOOL_KEYS = new Set(["tool_inputs", "tool_builtin", "tool_version"]);
	type Group = "own" | "system" | "tool" | "agent" | "custom";
	const groupOf = (key: string): Group => (AGENT_KEYS.has(key) ? "agent" : SYSTEM_KEYS.has(key) ? "system" : TOOL_KEYS.has(key) ? "tool" : "custom");
	const groups = $derived.by(() => {
		const out: Array<{ id: Group; label: string; rows: typeof shown }> = [];
		// System first, and Repeat first within it: the one row every object shares sits at the very top.
		for (const [id, label] of [["system", "System"], ["own", isCapability ? "Capability" : "Credential"], ["tool", "Tool"], ["agent", "Agent"], ["custom", "Custom"]] as Array<[Group, string]>) {
			const rows = id === "own" ? ownRows : shown.filter((r) => groupOf(r.key) === id);
			if (id === "system" && canRepeat) {
				// Check first rides right under Repeat: it runs before each occurrence.
				const at = rows.findIndex((r) => r.key === "check_first");
				rows.unshift({ key: "__repeat__", name: "Repeat" } as unknown as (typeof shown)[number], ...(at >= 0 ? rows.splice(at, 1) : []));
			}
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
	<div class="row-wrap" class:ro={!editable(rel)}>
		<div
			class="row"
			role="button"
			tabindex={editable(rel) ? 0 : -1}
			aria-disabled={!editable(rel)}
			onclick={() => toggleEdit(rel)}
			onkeydown={(e) => onRowKey(e, rel)}
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
				{#if isCapability && rel.key === "status"}
					<span class="status-val" style={badgeStyle(capabilityStatusBadge(capabilityStatus).color)} data-testid="capability-status-value">
						<PropIcon icon={capabilityStatusBadge(capabilityStatus).icon} size={14} />{capabilityStatusText(capabilityStatus)}
					</span>
				{:else if isCredential && rel.key === "status"}
					<span class="status-val" style={badgeStyle(credentialStatusBadge(credentialStatus).color)} data-testid="credential-status-value">
						<PropIcon icon={credentialStatusBadge(credentialStatus).icon} size={14} />{credentialStatusText(credentialStatus)}
					</span>
				{:else if rel.format === "checkbox"}
					{@const on = plain(v, "checkbox") === true}
					<button
						class="chk"
						class:on
						aria-checked={on}
						role="checkbox"
						title={rel.name || rel.key}
						disabled={!editable(rel)}
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
				{:else if rel.key === "skills"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const sk = skillsById.get(id)}
						{@const ok = !!sk && (!sk.key || sk.machines.length > 0)}
						<span class="chip-wrap">
							<span class="chip" class:ok={!!sk?.key && ok} class:warn={!!sk?.key && !ok} title={!sk ? "Skill" : !sk.key ? `${sk.name} · instructions` : sk.machines.length ? `${sk.name} · working on ${sk.machines.join(", ")}` : `${sk.name} · not working on any computer yet`}>
								<span class="emoji">🛠️</span>{sk?.name ?? `${id.slice(0, 8)}…`}
							</span>
							<button class="rm" aria-label={`Remove ${sk?.name ?? "skill"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
						</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/each}
				{:else if rel.key === "tools"}
					{#each plain(v, "object") as string[] as id (id)}
						{@const tl = toolsById.get(id)}
						<span class="chip-wrap">
							<span class="chip" class:warn={!!tl?.error} title={!tl ? "Tool" : tl.error ? `${tl.name} · ${tl.error}` : `${tl.name} · ${tl.builtin ? "built-in tool" : "tool"}`}>
								<span class="emoji">🧰</span>{tl?.name ?? `${id.slice(0, 8)}…`}
							</span>
							<button class="rm" aria-label={`Remove ${tl?.name ?? "tool"}`} title="Remove" onclick={(e) => { e.stopPropagation(); void removeValue(rel.key, id); }}>×</button>
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
						<!-- A pasted key is a secret: the row says it is set; its editor shows it. -->
						<span class="val-text" class:wrap={rel.key === "error"}>{isCredential && rel.key.startsWith(KEY_PREFIX) ? "••••••••" : d}</span>
					{:else}
						<span class="placeholder">{placeholderFor(rel)}</span>
					{/if}
				{/if}
			</span>
		</div>
		{#if rowRemovable(rel) && editable(rel)}
			<button class="rm row-rm" aria-label={`Remove ${rel.name || rel.key}`} title="Remove property" onclick={(e) => { e.stopPropagation(); void removeProp(rel.key); }}>×</button>
		{/if}
		{#if editing === rel.key}
			<div class="pop">
				<div class="pop-head">
					<span class="pop-name">{rel.name || rel.key}</span>
					{#if rel.key !== "done" && !((isCredential || isCapability) && rel.key === "status")}
						<button class="pop-rm" title="Remove property" onclick={() => void removeProp(rel.key)}>Remove</button>
					{/if}
				</div>
				{#if isCapability && rel.key === "status"}
					<CapabilityStatus {object} {onchanged} />
				{:else if isCredential && rel.key === "status"}
					<CredentialStatus {object} {onchanged} pollError={connectPollError} />
				{:else}
					<PropertyValue {rel} value={v} spaceId={object.fields["channel"]?.stringValue ?? ""} onsave={(nv) => void saveValue(rel.key, nv)} />
				{/if}
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
{#if !locked}
	<button
		class="add-prop"
		onclick={(e) => {
			const r = e.currentTarget.getBoundingClientRect();
			addPos = { x: r.left, y: r.bottom + 4 };
		}}
	>＋ Add property</button>
{/if}
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
	/* Read-only: shown, not editable - no hover, no per-value ×. */
	.ro .row {
		cursor: default;
	}
	.ro .row:hover {
		background: none;
	}
	.ro .rm {
		display: none;
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
	/* Errors are read, not glanced at: show all of it. */
	.val-text.wrap {
		white-space: normal;
		overflow-wrap: anywhere;
		text-align: left;
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
	.chk:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.chk:disabled {
		cursor: default;
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
		/* Span the row, not a fixed width: a narrow pane would clip it. */
		left: 0;
		right: 0;
		z-index: 90;
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
