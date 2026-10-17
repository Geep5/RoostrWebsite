<script lang="ts">
	import { goto } from "$app/navigation";
	import { showDate } from "$lib/dates";
	/**
	 * Anytype dataview grid: a view carries an ordered relation list
	 * (model/view.ts getVisibleRelations) and each visible relation is a
	 * column - Name first, then relation columns, then "+" to add one.
	 * We persist the list as a `viewRelations` field on the query/collection
	 * object itself, same as viewFilters/viewSorts. Hover a header for "×"
	 * (hide column); click a header to sort.
	 */
	import PropertyFlow from "./PropertyFlow.svelte";
	import RowContextMenu from "./RowContextMenu.svelte";
	import PropertyValue from "./PropertyValue.svelte";
	import { badgeStyle, tagStyle } from "$lib/options";
	import { credentialCellBadge, credentialStatusBadge, credentialStatusText, reconnectTarget } from "$lib/credential-actions";
	import { checking, reconnectState } from "$lib/credential-reconnect.svelte";
	import CredentialCell from "./CredentialCell.svelte";
	import PropIcon from "./PropIcon.svelte";
	import CheckboxIcon from "./CheckboxIcon.svelte";
	import { fetchQuery, note, type QueryResultRow } from "$lib/api";
	import { blockersFor } from "$lib/protected";
	import { fetchMachines, machineName, resolveMany, servingCopy, type MachineRow } from "$lib/serving";
	import { store, layoutOf } from "$lib/data.svelte";
	import type { ObjectJSON, RelationDefJSON, ValueJSON } from "$lib/types";
	import { fieldStr, isLockedTool, repeatOf } from "$lib/types";
	import { describeRepeat } from "$lib/repeat";
	import { kindName, objectIcon, typeIcon } from "$lib/icons";
	import { seedsFromView } from "$lib/view-seeds";

	let {
		body,
		object,
		relations,
		defaultSorts = [],
		oncreate,
		onremove,
		onchanged,
	}: {
		/** /api/query request body (setId, filters, …). */
		body: Record<string, unknown>;
		/** The query/collection object owning the view config. */
		object: ObjectJSON;
		relations: RelationDefJSON[];
		defaultSorts?: Array<{ key: string; type: "asc" | "desc" }>;
		/** Renders the Anytype "+ New Object" footer row when provided.
		 *  Committing the entry row calls it with the typed name. */
		oncreate?: (name: string) => Promise<void>;
		/** Collections: the row context menu offers "Remove from
		 *  collection", severing membership without touching the object. */
		onremove?: (ids: string[]) => Promise<void>;
		onchanged: () => Promise<void>;
	} = $props();

	let rows = $state<QueryResultRow[]>([]);

	// ── Row multi-select (Anytype SelectType.Record) ────────────────
	// Cmd/ctrl-click toggles, shift-click ranges from the anchor; a
	// selection bar offers bulk delete; Escape or plain click clears.
	let selectedRows = $state<string[]>([]);
	let rowAnchor = "";

	function toggleRow(id: string) {
		rowAnchor = id;
		selectedRows = selectedRows.includes(id) ? selectedRows.filter((x) => x !== id) : [...selectedRows, id];
	}

	function rangeRows(id: string) {
		const a = rows.findIndex((r) => r.id === (rowAnchor || selectedRows[0]));
		const b = rows.findIndex((r) => r.id === id);
		if (a === -1 || b === -1) {
			toggleRow(id);
			return;
		}
		selectedRows = rows.slice(Math.min(a, b), Math.max(a, b) + 1).map((r) => r.id);
	}

	// ── Marquee: drag anywhere in the set area rubber-bands records ──
	// Anytype's selection/provider.tsx idiom: press on a row OR the
	// whitespace around the table, drag a visible rectangle, and every
	// row it touches joins the selection. Drag-vs-click is a 4px
	// threshold, so plain clicks keep navigating and editing.
	let setEl = $state<HTMLElement>();
	let marquee = $state<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
	let dragMoved = $state(false);

	let marqueeFromOutside = false;

	function onSetMouseDown(e: MouseEvent) {
		if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
		const t = e.target as HTMLElement;
		if (!setEl) return;
		// Anytype's SelectionProvider wraps ALL app content (selection/
		// provider.tsx:1174): the rubber band arms anywhere except chrome and
		// interactive surfaces - nav, side panels, headers, tab strip, menus,
		// dialogs. Page margins, gaps between blocks, and the area below the
		// table all arm; what is protected is the same as theirs (links,
		// buttons, inputs, editors, the header row, the entry row).
		const inSet = setEl.contains(t);
		if (!inSet && t.closest("nav, aside, header, .tab-strip, .ctx-menu, .cell-pop, [role='dialog'], [role='menu']")) return;
		// Interactive targets own their gestures; the header row owns
		// sort/resize/reorder; the entry row owns its input; text
		// editors (title, description) keep native text selection.
		if (t.closest("a, button, input, textarea, select, [contenteditable], .cell-pop, thead, tr.new-row")) return;
		marqueeFromOutside = !inSet;
		marquee = { x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY };
		dragMoved = false;
	}

	function onMarqueeMove(e: MouseEvent) {
		if (!marquee) return;
		marquee = { ...marquee, x1: e.clientX, y1: e.clientY };
		if (Math.abs(marquee.x1 - marquee.x0) + Math.abs(marquee.y1 - marquee.y0) < 4) return;
		dragMoved = true;
		const left = Math.min(marquee.x0, marquee.x1);
		const right = Math.max(marquee.x0, marquee.x1);
		const top = Math.min(marquee.y0, marquee.y1);
		const bottom = Math.max(marquee.y0, marquee.y1);
		const hit: string[] = [];
		for (const tr of setEl?.querySelectorAll<HTMLElement>("tbody tr[data-id]") ?? []) {
			const r = tr.getBoundingClientRect();
			if (r.bottom >= top && r.top <= bottom && r.right >= left && r.left <= right) hit.push(tr.dataset.id ?? "");
		}
		selectedRows = hit;
	}

	function endRowDrag() {
		// A plain click on surrounding whitespace clears, mirroring the
		// plain-click-on-a-row behavior.
		if (marquee && marqueeFromOutside && !dragMoved && selectedRows.length) selectedRows = [];
		marquee = null;
	}

	function onRowClick(e: MouseEvent, id: string) {
		if (e.metaKey || e.ctrlKey) {
			e.preventDefault();
			toggleRow(id);
			return;
		}
		if (e.shiftKey) {
			e.preventDefault();
			rangeRows(id);
			return;
		}
		// The click that ends a drag must not clear what the drag just
		// selected — mouseup fires a click on the row under the cursor.
		if (dragMoved) {
			dragMoved = false;
			return;
		}
		if (selectedRows.length) {
			selectedRows = [];
			return;
		}
		// In-app navigation: a full page load re-opened the whole vault (engine boot) on every row click.
		void goto(`/app/object/${id}`);
	}

	// ── Inline cell editing (Anytype dataview.tsx onCellClick + cell/*:
	// only the name cell opens the object; every relation cell edits in
	// place - checkbox toggles instantly, everything else anchors the
	// per-format editor at the cell) ─────────────────────────────────
	let cellEdit = $state<{ recordId: string; key: string; x: number; y: number } | null>(null);

	/** Any real relation column edits inline; specials stay read-only. */
	function isEditable(key: string): boolean {
		// Repeat is a rule edited in the object's Repeat row, not a cell value.
		if (key === "repeat") return false;
		const rel = relations.find((r) => r.key === key);
		return !!rel && !rel.readOnly;
	}

	function cellValues(recordId: string, key: string): string[] {
		const r = rows.find((x) => x.id === recordId);
		const v = r?.fields[key];
		if (v?.valuesValue) return v.valuesValue.items.map((i) => i.stringValue ?? "").filter(Boolean);
		return v?.stringValue ? [v.stringValue] : [];
	}

	function onCellClick(e: MouseEvent, recordId: string, key: string) {
		// The mouseup ending a row-drag fires a click on the cell under the
		// cursor - that must not open an editor. Bubble on (without the
		// stopPropagation) so onRowClick clears the dragMoved flag.
		if (dragMoved) return;
		e.stopPropagation();
		if (formatOf(key) === "checkbox") {
			// Anytype checkbox cells toggle on click, no menu.
			const cur = rows.find((x) => x.id === recordId)?.fields[key]?.boolValue === true;
			void (async () => {
				await note.setField(recordId, key, { boolValue: !cur });
				await reload();
			})();
			return;
		}
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		cellEdit = cellEdit?.recordId === recordId && cellEdit.key === key ? null : { recordId, key, x: rect.left, y: rect.bottom + 4 };
	}

	/** The Served by column shows where a row runs; editing it pins the row to a computer (its `served_by`). */
	const SERVING_KEY = "served_by";
	/** The relation a cell's editor edits: the virtual Served by column edits `served_by`. */
	const editKey = (key: string): string => (key === "serving" ? SERVING_KEY : key);

	async function cellSave(v: ValueJSON) {
		if (!cellEdit) return;
		const { recordId, key } = cellEdit;
		const f = formatOf(key);
		// Single-value editors close on save; tag/object stay open for more picks.
		if (f !== "tag" && f !== "object") cellEdit = null;
		if (key === "serving") {
			// The picker links a Computer object; served_by holds its machine_id
			// (what the engine resolves and harnesses compare), never the link.
			const target = v.linkValue?.targetId ?? v.valuesValue?.items?.[0]?.linkValue?.targetId ?? "";
			const machineId = machinesNow.find((m) => m.id === target)?.machineId ?? "";
			if (machineId) await note.setField(recordId, SERVING_KEY, { stringValue: machineId });
			else await note.deleteField(recordId, SERVING_KEY);
		} else {
			await note.setField(recordId, key, v);
		}
		await reload();
	}

	/** Protected types/properties among the selected rows (id → why); bulk delete keeps them. */
	let selBlocked = $state(new Map<string, string>());
	$effect(() => {
		const ids = [...selectedRows];
		void blockersFor(ids).then((m) => (selBlocked = m));
	});
	const selDeletable = $derived(selectedRows.filter((id) => !selBlocked.has(id)));

	async function deleteSelected() {
		const ids = [...selDeletable];
		selectedRows = [];
		for (const id of ids) await note.del(id);
		await reload();
	}

	// ── Row context menu (right-click): open / remove-from-collection /
	// bin. A right-click inside the selection acts on the whole
	// selection; outside it, on just that row. ───────────────────────
	let ctxMenu = $state<{ x: number; y: number; id: string } | null>(null);

	function onRowContext(e: MouseEvent, id: string) {
		e.preventDefault();
		ctxMenu = { x: e.clientX, y: e.clientY, id };
	}

	function ctxTargets(): string[] {
		if (!ctxMenu) return [];
		return selectedRows.includes(ctxMenu.id) ? [...selectedRows] : [ctxMenu.id];
	}

	let override = $state<{ key: string; dir: "asc" | "desc" } | null>(null);
	const sortKey = $derived(override?.key ?? defaultSorts[0]?.key ?? "updatedAt");
	const sortDir = $derived(override?.dir ?? defaultSorts[0]?.type ?? "desc");

	/** Built-in columns that aren't stored fields. */
	const SPECIALS: Array<{ key: string; name: string }> = [
		{ key: "type", name: "Type" },
		{ key: "createdAt", name: "Created" },
		{ key: "updatedAt", name: "Updated" },
		// The object's resolved computer (its own pin, else its agent's), warning when it can't be honoured.
		{ key: "serving", name: "Served by" },
		// The object's recurrence, in words ("Every week on Wed at 9:00 AM · next Oct 8"); empty when it is one-off.
		{ key: "repeat", name: "Repeat" },
	];

	/** Anytype default grid columns for a fresh view. */
	const DEFAULT_COLUMNS = ["type", "updatedAt"];
	/** Icons for the built-in columns, matching the filter and sort menus. */
	const SPECIAL_EMOJI: Record<string, string> = { type: "▦", createdAt: "🗓️", updatedAt: "🗓️", serving: "🖥️", repeat: "↻" };
	const DEFAULT_WIDTH = 150;
	const MIN_WIDTH = 60;

	interface Col {
		key: string;
		width: number;
	}

	/** Anytype viewRelation = {relationKey, width, isVisible}; ours is a
	 * {key, width} map per item. Plain-string items (pre-width format)
	 * still parse. */
	const stored = $derived.by((): Col[] => {
		const items = object.fields["viewRelations"]?.valuesValue?.items ?? [];
		const out: Col[] = [];
		for (const i of items) {
			if (typeof i.stringValue === "string") {
				out.push({ key: i.stringValue, width: DEFAULT_WIDTH });
			} else if (i.mapValue) {
				const k = i.mapValue.entries["key"]?.stringValue;
				// "name" is the reserved entry carrying the Name column's width -
				// it is view state, not a relation column (GalleryView skips it too).
				if (k && k !== "name") out.push({ key: k, width: i.mapValue.entries["width"]?.intValue ?? DEFAULT_WIDTH });
			}
		}
		return out.length > 0 ? out : DEFAULT_COLUMNS.map((k) => ({ key: k, width: DEFAULT_WIDTH }));
	});

	/** The Name column's persisted width; unset means "auto, take the slack". */
	const storedNameWidth = $derived.by((): number | undefined => {
		const items = object.fields["viewRelations"]?.valuesValue?.items ?? [];
		for (const i of items) {
			if (i.mapValue?.entries["key"]?.stringValue === "name") {
				const w = i.mapValue.entries["width"]?.intValue;
				if (typeof w === "number" && w >= MIN_WIDTH) return w;
			}
		}
		return undefined;
	});
	let nameLocal = $state<number | null>(null);
	const nameWidth = $derived(nameLocal ?? storedNameWidth);

	/** Uncommitted state during a resize/reorder gesture. */
	let local = $state<Col[] | null>(null);
	const cols = $derived(local ?? stored);
	const columns = $derived(cols.map((c) => c.key));

	let adding = $state<{ x: number; y: number } | null>(null);

	async function persistView(next: Col[], nameW: number | undefined) {
		const items = next.map((c) => ({
			mapValue: { entries: { key: { stringValue: c.key }, width: { intValue: c.width } } },
		}));
		if (nameW !== undefined) {
			items.push({ mapValue: { entries: { key: { stringValue: "name" }, width: { intValue: nameW } } } });
		}
		await note.setField(object.id, "viewRelations", { valuesValue: { items } });
		await onchanged();
	}

	async function saveColumns(next: Col[]) {
		local = next;
		await persistView(next, storedNameWidth);
		local = null;
	}

	// ── Column resize (drag the header's right edge) ──────────────
	function startResize(e: PointerEvent, idx: number) {
		e.preventDefault();
		e.stopPropagation();
		const startX = e.clientX;
		const startW = cols[idx].width;
		const snapshot = cols.map((c) => ({ ...c }));
		const move = (ev: PointerEvent) => {
			snapshot[idx] = { ...snapshot[idx], width: Math.max(MIN_WIDTH, startW + (ev.clientX - startX)) };
			local = [...snapshot];
		};
		const up = () => {
			window.removeEventListener("pointermove", move);
			window.removeEventListener("pointerup", up);
			void saveColumns(snapshot);
		};
		window.addEventListener("pointermove", move);
		window.addEventListener("pointerup", up);
	}

	function startNameResize(e: PointerEvent) {
		e.preventDefault();
		e.stopPropagation();
		const startX = e.clientX;
		const th = (e.currentTarget as HTMLElement).closest("th");
		const startW = nameWidth ?? th?.getBoundingClientRect().width ?? 240;
		const move = (ev: PointerEvent) => {
			nameLocal = Math.max(MIN_WIDTH, Math.round(startW + (ev.clientX - startX)));
		};
		const up = () => {
			window.removeEventListener("pointermove", move);
			window.removeEventListener("pointerup", up);
			void persistView(cols, nameLocal ?? undefined).then(() => (nameLocal = null));
		};
		window.addEventListener("pointermove", move);
		window.addEventListener("pointerup", up);
	}

	// ── Column reorder (drag a header into a gap between columns) ──
	// The dragged column is tinted top to bottom, and one bar the table's full
	// height marks the gap it lands in: left or right of the header under the
	// pointer, by which half the pointer is over.
	let dragIdx = $state(-1);
	/** The gap the column lands in: 0 = before the first column, cols.length = after the last; -1 = nowhere (or where it already is). */
	let gapIdx = $state(-1);
	let gapBar = $state<{ left: number; top: number; height: number } | null>(null);

	function onColumnDragOver(e: DragEvent, i: number, forceAfter = false) {
		if (dragIdx < 0) return;
		e.preventDefault();
		const th = e.currentTarget as HTMLTableCellElement;
		const r = th.getBoundingClientRect();
		const after = forceAfter || e.clientX > r.left + r.width / 2;
		const gap = forceAfter ? cols.length : after ? i + 1 : i;
		if (gap === dragIdx || gap === dragIdx + 1) {
			gapIdx = -1;
			gapBar = null;
			return;
		}
		gapIdx = gap;
		const table = th.closest("table") as HTMLTableElement;
		const edge = forceAfter ? th.offsetLeft : th.offsetLeft + (after ? th.offsetWidth : 0);
		gapBar = { left: table.offsetLeft + edge, top: table.offsetTop, height: table.offsetHeight };
	}

	function endColumnDrag() {
		dragIdx = gapIdx = -1;
		gapBar = null;
	}

	function dropColumn() {
		const from = dragIdx;
		const gap = gapIdx;
		endColumnDrag();
		if (from < 0 || gap < 0) return;
		const next = cols.map((c) => ({ ...c }));
		const [moved] = next.splice(from, 1);
		next.splice(gap > from ? gap - 1 : gap, 0, moved);
		void saveColumns(next);
	}

	// ── Virtual "Served by" column (resolved serving) ───────────────
	// The value is resolved per row by the engine's serving resolver
	// ($lib/serving), never stored: a serving column shows it, serving
	// filter rules (peeled from the body, which never sends them to the
	// engine) and a serving sort are applied here after the query.
	interface ServingInfo {
		text: string;
		/** What the cell shows: the computer's name, or "No machine". */
		label: string;
		warning: boolean;
		reason: string;
		machineId: string;
		/** The computer's own emoji, else the Computer type's; empty when nothing serves it. */
		icon: string;
	}
	let servingById = $state<Map<string, ServingInfo>>(new Map());
	/** The computers the Served by column names, for turning a picked Computer into its machine_id. */
	let machinesNow = $state<MachineRow[]>([]);
	/** What this view's filters give an agent made from one of its cells ("+ New agent" in Team Tasks → Customer: Team). */
	const viewSeeds = $derived(seedsFromView(object.fields, relations));

	/** Needs a person: the resolution cannot be honoured, or nothing serves the object at all. */
	function servingAttention(info: ServingInfo | undefined): boolean {
		return !!info && (info.warning || info.reason === "unserved");
	}

	function servingMatch(rule: { condition: string; value: string }, info: ServingInfo | undefined): boolean {
		const hit = rule.value === "attention" ? servingAttention(info) : rule.value === "ok" ? !!info && !servingAttention(info) : info?.reason === rule.value;
		return rule.condition === "notEqual" ? !hit : hit;
	}

	/** Cannot be honoured (unsatisfied / pinned-uncapable) first, then unserved, then fine. */
	function servingRank(info: ServingInfo | undefined): number {
		if (!info) return 1;
		return info.warning ? 0 : info.machineId ? 2 : 1;
	}

	/** Rows per page: the first page opens the view, scrolling near the end fetches the next. */
	const PAGE = 100;
	/** What every row draws whatever the columns: name, icon, the task checkbox, a credential's status cell (and whether a check of it is underway). */
	const ROW_FIELDS = ["name", "iconEmoji", "done", "status", "error", "service", "served_by", "check_requested", "checked_at"];
	/** Everything that matched the query, not just the rows loaded. */
	let total = $state(0);
	/** The query a paged view continues with (offset = rows loaded); null while every row is loaded at once. */
	let pageQuery = $state.raw<Record<string, unknown> | null>(null);
	let loadingMore = false;
	/** Bumped by every load: a response for an older one (a sort clicked meanwhile) is dropped. */
	let generation = 0;

	/**
	 * The engine sorts and filters every matching object; the table fetches
	 * the first page of that order - only the properties it draws - and the
	 * next page as you scroll. A new sort or filter runs the query again.
	 * Only what the engine can't compute (the Served by column, its filter or
	 * sort, and a sort by next repeat) needs every row, so those views load
	 * all of them, as before. `keepCount`: a refresh after a change keeps
	 * the rows already scrolled into view.
	 */
	async function load(keepCount = false) {
		const gen = ++generation;
		const { servingFilters, ...engineBody } = body as Record<string, unknown> & { servingFilters?: Array<{ condition: string; value: string }> };
		const rules = servingFilters ?? [];
		const effectiveKey = override?.key ?? defaultSorts[0]?.key ?? "updatedAt";
		const effectiveType = override?.dir ?? defaultSorts[0]?.type ?? "desc";
		const servingSort = effectiveKey === "serving" ? { type: effectiveType } : null;
		// Repeat is a map the engine can't order: sort by the next occurrence here.
		const repeatSort = effectiveKey === "repeat" ? { type: effectiveType } : null;
		const base = servingSort || repeatSort ? [{ key: "updatedAt", type: "desc" }] : override ? [{ key: effectiveKey, type: effectiveType }] : defaultSorts.length > 0 ? defaultSorts : [{ key: "updatedAt", type: "desc" }];
		const sorts = base.map((s) => ({
			key: s.key,
			type: s.type,
			emptyPlacement: ("empty" in s ? s.empty : undefined) ?? "end",
		}));
		const needsEveryRow = !!servingSort || !!repeatSort || rules.length > 0 || columns.includes("serving");
		if (!needsEveryRow) {
			const query = { ...engineBody, sorts, fields: [...new Set([...ROW_FIELDS, ...columns])] };
			const res = await fetchQuery({ ...query, offset: 0, limit: keepCount ? Math.max(PAGE, rows.length) : PAGE });
			if (gen !== generation) return;
			pageQuery = query;
			rows = res.records;
			total = res.total;
			return;
		}
		pageQuery = null;
		const res = await fetchQuery({ ...engineBody, sorts });
		if (gen !== generation) return;
		let all = res.records;
		if (repeatSort) {
			// One-off objects last either way, like an empty cell under any engine sort.
			const dir = repeatSort.type === "desc" ? -1 : 1;
			const next = (r: QueryResultRow) => repeatOf(r.fields)?.next;
			all = [...all].sort((a, b) => {
				const x = next(a), y = next(b);
				if (x === undefined || y === undefined) return x === undefined ? (y === undefined ? 0 : 1) : -1;
				return dir * (x - y);
			});
		}
		if (columns.includes("serving") || rules.length > 0 || servingSort) {
			const { rows: machineRows, machines } = await fetchMachines();
			if (gen !== generation) return;
			machinesNow = machines;
			const resolved = await resolveMany(all, machineRows);
			const map = new Map<string, ServingInfo>();
			all.forEach((r, i) => {
				const s = resolved[i];
				const label = s.machineId ? machineName(machines, s.machineId) : "No machine";
				const machine = s.machineId ? machineRows.find((m) => m.fields["machine_id"]?.stringValue === s.machineId) : undefined;
				const icon = s.machineId ? objectIcon(machine?.fields["iconEmoji"]?.stringValue, "machine") : "";
				map.set(r.id, { ...servingCopy(s, machines), label, reason: s.reason, machineId: s.machineId, icon });
			});
			servingById = map;
			if (rules.length > 0) all = all.filter((r) => rules.every((rule) => servingMatch(rule, servingById.get(r.id))));
			if (servingSort) {
				const dir = servingSort.type === "desc" ? -1 : 1;
				all = [...all].sort((a, b) => dir * (servingRank(servingById.get(a.id)) - servingRank(servingById.get(b.id))) || (a.updatedAt - b.updatedAt) * -1);
			}
		}
		if (gen !== generation) return;
		rows = all;
		total = all.length;
	}

	/** The next page of a paged view, in the same order. */
	async function loadMore() {
		const query = pageQuery;
		if (!query || loadingMore || rows.length >= total) return;
		loadingMore = true;
		const gen = generation;
		try {
			const res = await fetchQuery({ ...query, offset: rows.length, limit: PAGE });
			if (gen !== generation) return;
			const have = new Set(rows.map((r) => r.id));
			rows = [...rows, ...res.records.filter((r) => !have.has(r.id))];
			total = res.total;
		} finally {
			loadingMore = false;
		}
	}

	/** On the sentinel row after the last one: coming within a screen of it loads the next page. */
	function nearEnd(node: HTMLElement) {
		const io = new IntersectionObserver((entries) => {
			if (entries.some((e) => e.isIntersecting)) void loadMore();
		}, { rootMargin: "800px 0px" });
		io.observe(node);
		return { destroy: () => io.disconnect() };
	}

	$effect(() => {
		void body;
		void override;
		void defaultSorts;
		void load();
	});

	/** Task-layout rows show a checkbox instead of the icon (Anytype task
	 * layout): clicking toggles the bundled `done` relation in place. */
	async function toggleDone(r: QueryResultRow, e: MouseEvent) {
		e.stopPropagation();
		await note.setField(r.id, "done", { boolValue: !(r.fields["done"]?.boolValue === true) });
		await reload();
	}

	/** After a change: the same view, the same rows scrolled into view. */
	export function reload(): Promise<void> {
		return load(true);
	}

	// ── Pending entry row (Anytype: "+ New Object" shows an "Enter
	// name" row in place; the record exists only once committed) ──────
	let pendingNew = $state(false);
	let newDraft = $state("");
	let committing = $state(false);

	export function beginNew() {
		pendingNew = true;
		newDraft = "";
	}

	async function commitNew(cancel = false) {
		if (committing) return;
		const name = newDraft.trim();
		pendingNew = false;
		newDraft = "";
		if (cancel || !oncreate) return;
		committing = true;
		try {
			await oncreate(name);
			await load();
		} finally {
			committing = false;
		}
	}

	function renameInput(node: HTMLInputElement) {
		requestAnimationFrame(() => node.focus());
	}

	function colName(key: string): string {
		return SPECIALS.find((s) => s.key === key)?.name ?? relations.find((r) => r.key === key)?.name ?? key;
	}

	/** The property's own emoji, when it has one (specials have none). */
	function colIcon(key: string): string {
		return relations.find((r) => r.key === key)?.iconEmoji ?? "";
	}

	function formatOf(key: string): string {
		return relations.find((r) => r.key === key)?.format ?? "";
	}

	function cell(r: QueryResultRow, key: string): string {
		if (key === "serving") return servingById.get(r.id)?.label ?? "";
		if (key === "repeat") {
			const rule = repeatOf(r.fields);
			return rule ? describeRepeat(rule) : "";
		}
		if (key === "type") {
			// Display name, never the raw key: "finance_task" reads as its
			// type's name (or at worst the key with spaces).
			const t = store.types.find((x) => x.key === r.typeKey);
			return t?.name || kindName(r.typeKey);
		}
		if (key === "createdAt") return r.createdAt ? showDate(r.createdAt) : "";
		if (key === "updatedAt") return r.updatedAt ? showDate(r.updatedAt) : "";
		const v: ValueJSON | undefined = r.fields[key];
		const format = formatOf(key);
		if (!v) return "";
		// Object values come as a list, a bare link, or a bare id: all read as names.
		if (format === "object" || v.linkValue) {
			const ids = v.valuesValue
				? v.valuesValue.items.map((i) => i.linkValue?.targetId ?? i.stringValue ?? "")
				: [v.linkValue?.targetId ?? v.stringValue ?? ""];
			return ids
				.filter(Boolean)
				.map((id) => store.summaries.find((s) => s.id === id)?.name || id.slice(0, 8))
				.join(", ");
		}
		if (v.stringValue !== undefined) return v.stringValue;
		if (v.boolValue !== undefined) return v.boolValue ? "✓" : "";
		if (v.intValue !== undefined) return format === "date" ? showDate(v.intValue) : String(v.intValue);
		if (v.floatValue !== undefined) return String(v.floatValue);
		if (v.valuesValue) return v.valuesValue.items.map((i) => i.stringValue ?? "").join(", ");
		if (v.listValue) return v.listValue.values.join(", ");
		return "";
	}

	function toggleSort(key: string) {
		if (sortKey === key) override = { key, dir: sortDir === "asc" ? "desc" : "asc" };
		else override = { key, dir: "asc" };
	}
</script>

<svelte:window onkeydown={(e) => { if (e.key === "Escape") { cellEdit = null; selectedRows = []; ctxMenu = null; } }} onmousedown={(e) => { if (cellEdit && !(e.target as HTMLElement).closest(".cell-pop, td.editable")) cellEdit = null; onSetMouseDown(e); }} onmousemove={onMarqueeMove} onmouseup={endRowDrag} />

<div class="set-table" bind:this={setEl}>
	{#if gapBar}
		<div class="col-gap" style="left: {gapBar.left}px; top: {gapBar.top}px; height: {gapBar.height}px"></div>
	{/if}
	{#if marquee && dragMoved}
		<div
			class="marquee"
			style="left: {Math.min(marquee.x0, marquee.x1)}px; top: {Math.min(marquee.y0, marquee.y1)}px; width: {Math.abs(marquee.x1 - marquee.x0)}px; height: {Math.abs(marquee.y1 - marquee.y0)}px"
		></div>
	{/if}
	<table>
		<colgroup>
			<col style={nameWidth ? `width:${nameWidth}px` : ""} />
			{#each cols as c, i (c.key)}
				<col style="width: {c.width}px" class:col-dragging={dragIdx === i} />
			{/each}
			<col style="width: 32px" />
		</colgroup>
		<thead>
			<tr>
				<th>
					<button class="head" onclick={() => toggleSort("name")}>Name {sortKey === "name" ? (sortDir === "asc" ? "↑" : "↓") : ""}</button>
					<span
						class="resize"
						role="separator"
						aria-orientation="vertical"
						title="Drag to resize"
						onpointerdown={startNameResize}
					></span>
				</th>
				{#each cols as c, i (c.key)}
					<th
						class:dragging={dragIdx === i}
						draggable="true"
						ondragstart={(e) => {
							dragIdx = i;
							e.dataTransfer?.setData("text/plain", c.key);
						}}
						ondragover={(e) => onColumnDragOver(e, i)}
						ondrop={(e) => {
							e.preventDefault();
							dropColumn();
						}}
						ondragend={endColumnDrag}
					>
						<button class="head" onclick={() => toggleSort(c.key)}>{#if colIcon(c.key)}<span class="head-icon">{colIcon(c.key)}</span>{/if}{colName(c.key)} {sortKey === c.key ? (sortDir === "asc" ? "↑" : "↓") : ""}</button>
						<button class="hide" title="Hide column" onclick={() => void saveColumns(cols.filter((x) => x.key !== c.key))}>×</button>
						<span
							class="resize"
							role="separator"
							aria-orientation="vertical"
							title="Drag to resize"
							onpointerdown={(e) => startResize(e, i)}
						></span>
					</th>
				{/each}
				<th class="plus-col" ondragover={(e) => onColumnDragOver(e, cols.length - 1, true)} ondrop={(e) => { e.preventDefault(); dropColumn(); }}>
					<button
						class="head plus"
						title="Add column"
						onclick={(e) => {
							const r = e.currentTarget.getBoundingClientRect();
							adding = adding ? null : { x: r.left, y: r.bottom + 4 };
						}}>+</button
					>
				</th>
			</tr>
		</thead>
		<tbody>
			{#each rows as r (r.id)}
				<tr data-id={r.id} class:selected={selectedRows.includes(r.id)} onclick={(e) => onRowClick(e, r.id)} onauxclick={(e) => { if (e.button === 1) { e.preventDefault(); window.open(`/app/object/${r.id}`, "_blank"); } }} oncontextmenu={(e) => onRowContext(e, r.id)} ondragstart={(e) => e.preventDefault()}>
					<td class="name">
						{#if layoutOf(r.typeKey) === "task"}
							<button class="task-check" class:on={r.fields["done"]?.boolValue === true} aria-label="done" onclick={(e) => void toggleDone(r, e)}>
								<CheckboxIcon checked={r.fields["done"]?.boolValue === true} size={18} />
							</button>
						{:else}
							<span class="row-icon">{objectIcon(r.fields["iconEmoji"]?.stringValue, r.typeKey)}</span>
						{/if}
						{fieldStr(r.fields, "name") || "Untitled"}</td>
					{#each columns as c (c)}
						{@const target = reconnectTarget(r, c)}
						{#if r.typeKey === "credential" && c === "status"}
							<!-- The harness writes a credential's status: shown in words like its Properties row, never hand-edited here. Not connected, it reconnects from here. -->
							{@const st = fieldStr(r.fields, "status")}
							{#if target}
								{@const badge = credentialCellBadge(st, reconnectState(r.id), checking(r.id, r.fields))}
								<td><CredentialCell row={r} {target} onchanged={reload} label={`Reconnect ${fieldStr(r.fields, "name") || "credential"}: ${badge.text}`}><span class="cred-status" style={badgeStyle(badge.color)}><PropIcon icon={badge.icon} size={14} />{badge.text}</span></CredentialCell></td>
							{:else}
								<td><span class="cred-status" style={badgeStyle(credentialStatusBadge(st).color)}><PropIcon icon={credentialStatusBadge(st).icon} size={14} />{credentialStatusText(st)}</span></td>
							{/if}
						{:else if target && c === "error"}
							<!-- A signed-out credential's Error (its own, or one a task/agent run hit) reconnects instead of editing. -->
							<td><CredentialCell row={r} {target} onchanged={reload} label={`Reconnect: ${cell(r, c)}`}><span class="cell-warn">{cell(r, c)}</span></CredentialCell></td>
						{:else if isEditable(c) && !isLockedTool(r.fields)}
							{@const rel = relations.find((x) => x.key === c)}
							{@const fmt = formatOf(c)}
							<td class="editable" onclick={(e) => onCellClick(e, r.id, c)}>
								{#if fmt === "tag" || fmt === "status"}
									{#each cellValues(r.id, c) as t (t)}
										{@const opt = rel?.options.find((o) => o.text === t)}
										<span class="cell-tag" style={tagStyle(opt?.color ?? "")}>{t}</span>
									{/each}
								{:else if fmt === "checkbox"}
									<span class="cell-check" class:on={r.fields[c]?.boolValue === true}><CheckboxIcon checked={r.fields[c]?.boolValue === true} size={20} /></span>
								{:else}
									<span class:cell-warn={c === "error" && cell(r, c) !== ""}>{cell(r, c)}</span>
								{/if}
							</td>
						{:else}
							<td class:editable={c === "serving"} class:muted={c === "type" || c === "createdAt" || c === "updatedAt"} class:cell-warn={c === "serving" && (servingById.get(r.id)?.warning ?? false)} title={c === "serving" ? servingById.get(r.id)?.text : undefined} onclick={c === "serving" && !isLockedTool(r.fields) ? (e) => onCellClick(e, r.id, c) : undefined}>{#if c === "type" && typeIcon(r.typeKey)}<span class="type-icon">{typeIcon(r.typeKey)}</span>{:else if c === "serving" && servingById.get(r.id)?.icon}<span class="type-icon">{servingById.get(r.id)?.icon}</span>{/if}{cell(r, c)}</td>
						{/if}
					{/each}
					<td></td>
				</tr>
			{/each}
			{#if pageQuery && rows.length < total}
				<!-- The next page loads as this comes within a screen of view. -->
				<tr class="more-row" use:nearEnd><td colspan={cols.length + 2}>Loading more… {rows.length} of {total}</td></tr>
			{/if}
			{#if oncreate}
			{#if pendingNew}
				<tr class="entry-row">
					<!-- colspan must match the REAL column count: an oversized
					     colspan in a fixed-layout table fabricates phantom
					     columns and crushes the visible ones. -->
					<td colspan={cols.length + 2}>
						<span class="entry-wrap">
							<input
								class="rename"
								placeholder="Enter name"
								bind:value={newDraft}
								use:renameInput
								onblur={() => void commitNew(newDraft.trim() === "")}
								onkeydown={(e) => {
									if (e.key === "Enter") {
										e.preventDefault();
										void commitNew();
									}
									if (e.key === "Escape") {
										e.preventDefault();
										void commitNew(true);
									}
								}}
							/>
						</span>
					</td>
				</tr>
			{/if}
			<tr class="new-row">
				<td colspan={cols.length + 2}>
					<button class="new-object" onclick={() => beginNew()}>＋ New Object</button>
				</td>
			</tr>
		{/if}
	</tbody>
	</table>
	{#if cellEdit}
		{@const rel = relations.find((x) => x.key === editKey(cellEdit!.key))}
		{@const row = rows.find((x) => x.id === cellEdit!.recordId)}
		{#if rel && row}
			{@const wide = rel.format === "longtext" || rel.key === "error"}
			<!-- Long text gets room to read all of it: wider, and its box grows into the space below the cell. -->
			<div class="cell-pop" class:wide style="left:{Math.min(cellEdit.x, (document.querySelector('.main-col')?.getBoundingClientRect().right ?? window.innerWidth) - (wide ? 460 : 320))}px; top:{cellEdit.y}px; --room:{Math.max(120, window.innerHeight - cellEdit.y - 40)}px" role="dialog">
				<PropertyValue {rel} value={row.fields[rel.key]} spaceId={row.fields["channel"]?.stringValue ?? ""} seeds={viewSeeds} autoOpen onsave={(v) => void cellSave(v)} />
			</div>
		{/if}
	{/if}
	{#if selectedRows.length}
		<div class="sel-bar">
			<span>{selectedRows.length} selected</span>
			{#if selDeletable.length > 0}
				<button class="del" onclick={() => void deleteSelected()}>Delete{selBlocked.size > 0 ? ` ${selDeletable.length} (${selBlocked.size} protected kept)` : ""}</button>
			{:else}
				<span class="locked" title="Types and properties Roostr or your queries rely on can't be deleted">Can't delete: {[...selBlocked.values()][0] ?? ""}</span>
			{/if}
			<button class="clear" onclick={() => (selectedRows = [])}>✕</button>
		</div>
	{/if}
	{#if rows.length === 0}
		<p class="muted empty">No objects match.</p>
	{/if}
</div>
{#if adding}
	<!-- Anytype's property flow: toggle list -> Add property -> search all /
	     create new -> name + type. Fixed-position, viewport-clamped. -->
	<PropertyFlow
		x={adding.x}
		exclude={["served_by"]}
		y={adding.y}
		items={[
			...cols.map((c) => {
				const sp = SPECIALS.find((x) => x.key === c.key);
				const rel = relations.find((r) => r.key === c.key);
				return { key: c.key, name: sp?.name ?? rel?.name ?? c.key, format: rel?.format ?? "shorttext", on: true, emoji: SPECIAL_EMOJI[c.key] ?? rel?.iconEmoji };
			}),
			...SPECIALS.filter((sp) => !columns.includes(sp.key)).map((sp) => ({ key: sp.key, name: sp.name, format: "shorttext", on: false, emoji: SPECIAL_EMOJI[sp.key] })),
		]}
		ontoggle={(key, on) =>
			void saveColumns(on ? [...cols, { key, width: DEFAULT_WIDTH }] : cols.filter((c) => c.key !== key))}
		onadd={(rel) => void saveColumns([...cols, { key: rel.key, width: DEFAULT_WIDTH }])}
		onclose={() => (adding = null)}
	/>
{/if}

{#if ctxMenu}
	<RowContextMenu
		x={ctxMenu.x}
		y={ctxMenu.y}
		ids={ctxTargets()}
		spaceId={fieldStr(object.fields, "channel")}
		relationKeys={columns}
		{onremove}
		onchanged={reload}
		onclose={() => {
			ctxMenu = null;
			selectedRows = [];
		}}
	/>
{/if}
<style>
	.type-icon {
		margin-right: 6px;
	}
	.set-table {
		/* The column-move bar is placed inside it and scrolls with the table. */
		position: relative;
		overflow-x: auto;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		table-layout: fixed;
		font-size: 13px;
	}
	th {
		text-align: left;
		border-bottom: 1px solid var(--border);
		padding: 0;
		white-space: nowrap;
		position: relative;
		/* The cell clips, because the label cannot shrink below its own
		   padding: a fixed-layout column collapses to zero when no value has
		   loaded yet (a phone mid-sync), and the label then painted "Name"
		   straight under the next header's "Type". Body cells already clip;
		   headers now do too. */
		overflow: hidden;
	}
	th .head {
		display: block;
		box-sizing: border-box;
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		border: none;
		background: none;
		color: var(--muted);
		font-weight: 500;
		font-size: 13px;
		padding: 6px 10px;
		cursor: pointer;
		user-select: none;
	}
	/* The column being moved: tinted from header to last row, its label dimmed. */
	col.col-dragging {
		background: color-mix(in srgb, var(--accent) 14%, transparent);
	}
	th.dragging .head {
		opacity: 0.55;
	}
	/* Where it lands: one bar the table's full height, in the gap between columns. */
	.col-gap {
		position: absolute;
		width: 3px;
		margin-left: -1.5px;
		border-radius: 2px;
		background: var(--accent);
		box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
		pointer-events: none;
		z-index: 5;
	}
	.resize {
		position: absolute;
		top: 0;
		/* Inside the cell: the header clips its overflow now, so a grip
		   hanging past the edge would be half invisible and unusable. */
		right: 0;
		width: 11px;
		height: 100%;
		cursor: col-resize;
		z-index: 2;
	}
	/* Grips reveal when the header is hovered - otherwise they are invisible
	   11px slivers nobody can find. */
	th:hover .resize {
		background: var(--accent);
		opacity: 0.25;
	}
	.resize:hover {
		background: var(--accent);
		opacity: 0.6;
	}
	th .head:hover {
		color: inherit;
	}
	/* Pinned to the header's right edge, centred on the label - never
	   wrapped onto a line of its own under a long name. */
	th .hide {
		position: absolute;
		right: 8px;
		top: 50%;
		transform: translateY(-50%);
		border: none;
		background: var(--bg);
		border-radius: 4px;
		color: var(--muted);
		cursor: pointer;
		padding: 2px 5px;
		line-height: 1;
		opacity: 0;
		font-size: 13px;
	}
	th:hover .hide {
		opacity: 1;
	}
	th .hide:hover {
		color: var(--red);
	}
	/* Pinned to the right edge: reachable however many columns scroll past. */
	.plus-col {
		width: 40px;
		position: sticky;
		right: 0;
		z-index: 2;
		background: var(--bg, #1b1b1b);
		box-shadow: -8px 0 8px -8px rgba(0, 0, 0, 0.6);
	}
	th .head.plus {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 26px;
		height: 26px;
		margin: 4px auto;
		padding: 0;
		border: 1px solid var(--border);
		border-radius: 7px;
		font-size: 17px;
		line-height: 1;
		color: var(--fg);
	}
	th .head.plus:hover {
		border-color: var(--accent);
		color: var(--accent);
		background: rgba(55, 122, 255, 0.12);
	}
	td {
		border-bottom: 1px solid var(--border);
		padding: 7px 10px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 280px;
	}
	tbody tr {
		cursor: pointer;
	}
	tbody tr:hover {
		background: var(--hover);
	}
	.name {
		font-weight: 550;
	}
	.muted {
		color: var(--muted);
	}
	.empty {
		padding: 16px 10px;
	}
	/* Records are the selectable unit, so a drag must not leave native
	   text highlighting behind. The cell editor popover lives outside the
	   table, so it keeps normal text selection. */
	tbody td {
		user-select: none;
		-webkit-user-select: none;
	}
	.marquee {
		position: fixed;
		z-index: 80;
		pointer-events: none;
		background: rgba(55, 122, 255, 0.12);
		border: 1px solid rgba(55, 122, 255, 0.55);
		border-radius: 2px;
	}
	tr.selected td {
		background: rgba(55, 122, 255, 0.25);
	}
	.sel-bar {
		position: sticky;
		bottom: 8px;
		display: flex;
		align-items: center;
		gap: 12px;
		width: fit-content;
		margin: 8px auto 0;
		background: var(--panel, #1a1d23);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 6px 12px;
		font-size: 13px;
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
	}
	.sel-bar .del {
		background: none;
		border: none;
		color: #e05555;
		cursor: pointer;
		font-size: 13px;
	}
	.sel-bar .locked {
		color: var(--muted);
		font-size: 12px;
	}
	.sel-bar .clear {
		background: none;
		border: none;
		color: var(--muted);
		cursor: pointer;
	}
	td.editable {
		cursor: pointer;
	}
	td.editable:hover {
		background: var(--hl-light);
	}
	/* Anytype cellContent.c-checkbox: 20px icon, secondary until checked. */
	.cell-warn {
		color: #f55522;
	}
	.cell-check {
		display: inline-flex;
		color: var(--muted);
	}
	.cell-check.on {
		color: var(--fg);
	}
	.cred-status {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		color: var(--badge-fg);
		white-space: nowrap;
	}
	/* Anytype tagItem.isSmall: filled 20px pill, radius 10, pale text. */
	.cell-tag {
		display: inline-block;
		border-radius: 10px;
		padding: 0 6px;
		font-size: 12px;
		line-height: 20px;
		height: 20px;
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		margin-right: 4px;
	}
	.cell-pop {
		position: fixed;
		z-index: 130;
		background: var(--panel, #1a1d23);
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 8px;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
	}
	.cell-pop.wide {
		width: 440px;
		max-width: calc(100vw - 24px);
	}
	.new-object {
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		border: none;
		background: none;
		color: var(--muted);
		font-size: 14px;
		padding: 10px 8px;
		cursor: pointer;
		text-align: left;
	}
	.new-object:hover {
		color: var(--fg);
	}
	.new-row td {
		border-bottom: none;
	}
	input.rename {
		background: none;
		border: none;
		outline: none;
		color: var(--fg);
		font: inherit;
		width: 100%;
	}
	.entry-row td {
		border-bottom: none;
		padding: 4px 0;
	}
	.entry-wrap {
		display: flex;
		align-items: center;
		gap: 6px;
		border: 2px solid var(--accent);
		border-radius: 8px;
		padding: 8px 10px;
		max-width: 420px;
	}
	.head-icon {
		margin-right: 5px;
		font-size: 12px;
	}
	.task-check {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		vertical-align: -4px;
		margin-right: 6px;
		padding: 0;
		border: none;
		background: none;
		color: var(--muted);
		cursor: pointer;
	}
	.task-check:hover {
		color: var(--fg);
	}
	.task-check.on {
		color: var(--accent);
	}
</style>
