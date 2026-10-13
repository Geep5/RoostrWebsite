<script lang="ts">
	import { goto } from "$app/navigation";
	import { page } from "$app/state";
	import { untrack } from "svelte";
	import { buildSchemaGraph, type GraphObject, type GraphProperty, type GraphType, type SchemaGraph } from "$lib/graph";
	import { spaceRelations } from "$lib/relations";
	import { activeSpace } from "$lib/space.svelte";
	import { store, refreshAll } from "$lib/data.svelte";
	import type { RelationDefJSON, ValueJSON } from "$lib/types";

	type Sel = { k: "type"; t: number } | { k: "prop"; p: string } | { k: "obj"; t: number; j: number };

	/** Type→property curve bend: 0 straight, toward 1 pulled to the centre. */
	const CURVATURE = 0.55;
	const LEVELS = [
		["Types", "Types as clusters, sized by object count"],
		["Schema", "Properties appear; lines show which types use them"],
		["Objects", "Individual objects and their values"],
	] as const;

	// Each space gets its own graph.
	const defaultChannelId = $derived(store.channels[0]?.id ?? "");
	const channelId = $derived(activeSpace.id || defaultChannelId);
	// ?focus=<objectId>: pin that object and zoom to it ("show in graph").
	const focusId = $derived(page.url.searchParams.get("focus") ?? "");
	const relations = $derived(spaceRelations(store.relations, channelId));

	let graph = $state<SchemaGraph | null>(null);
	let status = $state("Loading graph…");
	let hover = $state<Sel | null>(null);
	let pin = $state<Sel | null>(null);
	let level = $state(0);
	let stage = $state<HTMLDivElement>();
	let world = $state<SVGGElement>();
	let zoomLabel = $state<HTMLSpanElement>();
	const layers: Record<"far" | "schema" | "schemaLabels" | "obj", SVGGElement | undefined> = { far: undefined, schema: undefined, schemaLabels: undefined, obj: undefined };

	// Built once per space (and once the definitions first load): rebuilding on
	// every sync refresh would reset the view and drop what is pinned.
	$effect(() => {
		const id = channelId;
		const isDefault = id === defaultChannelId && id !== "";
		if (!id || !store.loaded) {
			void refreshAll(); // channels not loaded yet; the effect re-runs when they land
			return;
		}
		const rels = untrack(() => relations);
		const types = untrack(() => store.types);
		let cancelled = false;
		status = "Loading graph…";
		void buildSchemaGraph(id, isDefault, rels, types)
			.then((g) => {
				if (cancelled) return;
				graph = g;
				hover = null;
				pin = null;
				status = g.types.length === 0 ? "Nothing in this space yet." : "";
			})
			.catch((e) => (status = `Graph failed: ${e}`));
		return () => {
			cancelled = true;
		};
	});

	const sel = $derived(pin ?? hover);
	const selType = $derived(graph && sel && sel.k !== "prop" ? graph.types[sel.t] : null);
	const selProp = $derived(graph && sel?.k === "prop" ? graph.props.find((p) => p.key === sel.p) ?? null : null);
	const selObj = $derived(selType && sel?.k === "obj" ? selType.objects[sel.j] : null);
	/** Every object in the graph by id, with its type: where a connection line ends. */
	const objIndex = $derived(new Map((graph?.types ?? []).flatMap((t) => t.objects.map((o) => [o.id, { o, t }] as const))));
	/** The selected object's connections to other objects: what it points at (link properties, text links, members) and what points at it. */
	const selLinks = $derived.by(() => {
		if (!selObj || !selType) return { out: [], inc: [] };
		const out = selObj.links.map((id) => objIndex.get(id)).filter((x) => !!x);
		const inc = [...objIndex.values()].filter(({ o }) => o !== selObj && o.links.includes(selObj.id));
		return { out, inc };
	});

	const typeOn = (t: GraphType) => !sel || (selType ? t === selType : !!selProp?.users.includes(t));
	const propOn = (p: GraphProperty) => !sel || (selObj ? selObj.held.has(p.key) : selType ? selType.usage.has(p.key) : p === selProp);

	/** A point (an object dot, or a type's centre) → property, bent toward the centre. */
	function edgePath(s: { x: number; y: number }, p: { x: number; y: number }): string {
		return `M${s.x},${s.y} Q${((s.x + p.x) / 2) * (1 - CURVATURE)},${((s.y + p.y) / 2) * (1 - CURVATURE)} ${p.x},${p.y}`;
	}
	const propByKey = $derived(new Map((graph?.props ?? []).map((p) => [p.key, p])));

	// ── Geometry, built once per graph: thousands of lines and dots drawn as a
	// few dozen shapes. One SVG element per line/dot made every pan frame
	// repaint ~15k nodes and every hover touch thousands of them. ──
	/** A dot as path commands (two arcs), so a whole type's dots are one shape. */
	const dotD = (x: number, y: number, r: number) => `M${x - r},${y}a${r},${r} 0 1,0 ${2 * r},0a${r},${r} 0 1,0 ${-2 * r},0`;
	/** Per type: its object→property lines, joined per property and in all. */
	const edgeGeo = $derived(
		(graph?.types ?? []).map((t) => {
			const perProp = new Map<string, string>();
			for (const o of t.objects) {
				for (const key of o.held) {
					const p = propByKey.get(key);
					if (p) perProp.set(key, (perProp.get(key) ?? "") + edgePath(o, p));
				}
			}
			return { all: [...perProp.values()].join(""), perProp };
		}),
	);
	const dotGeo = $derived((graph?.types ?? []).map((t) => t.objects.map((o) => dotD(o.x, o.y, 4.3)).join("")));
	/** The highlighted lines: the selected object's, a type's, or every type's to the selected property. */
	const litEdges = $derived.by((): Array<{ color: string; d: string }> => {
		if (!graph || !sel) return [];
		if (selObj && selType) return [{ color: selType.color, d: [...selObj.held].map((k) => propByKey.get(k)).filter((p) => !!p).map((p) => edgePath(selObj, p)).join("") }];
		if (selType) return [{ color: selType.color, d: edgeGeo[selType.index]?.all ?? "" }];
		if (selProp) return graph.types.map((t) => ({ color: t.color, d: edgeGeo[t.index]?.perProp.get(selProp.key) ?? "" })).filter((e) => e.d);
		return [];
	});

	/** Object labels: only those in view once zoomed far enough to read them, refreshed when the view settles. */
	const LABEL_CAP = 600;
	let labelObjs = $state<GraphObject[]>([]);
	let labelTimer: ReturnType<typeof setTimeout> | undefined;
	function scheduleLabels() {
		clearTimeout(labelTimer);
		labelTimer = setTimeout(updateLabels, 120);
	}
	function updateLabels() {
		if (!graph || v.s < 1.4) {
			if (labelObjs.length) labelObjs = [];
			return;
		}
		// A label runs right of its dot: widen the left edge so ones starting off-screen still show.
		const x0 = -v.x / v.s - 80;
		const y0 = -v.y / v.s - 10;
		const x1 = (box.w - v.x) / v.s + 10;
		const y1 = (box.h - v.y) / v.s + 10;
		const out: GraphObject[] = [];
		for (const t of graph.types) {
			for (const o of t.objects) {
				if (o.x >= x0 && o.x <= x1 && o.y >= y0 && o.y <= y1) out.push(o);
				if (out.length >= LABEL_CAP) break;
			}
		}
		labelObjs = out;
	}

	/** A label along the radius at angle `a`; flipped on the left half so it never reads upside down. `out`: text runs away from the centre. */
	function radial(a: number, dist: number, out: boolean) {
		const x = Math.cos(a) * dist;
		const y = Math.sin(a) * dist;
		const deg = (a * 180) / Math.PI;
		const flip = Math.cos(a) < -0.0001;
		return { x, y, anchor: out === flip ? "end" : "start", transform: `rotate(${flip ? deg + 180 : deg} ${x} ${y})` };
	}

	const propRadius = (p: GraphProperty) => 4 + Math.sqrt(p.users.length) * 2.4;

	// ── View: world → screen is translate(x, y) scale(s); kept off the reactive graph so panning never re-renders ──
	let v = { s: 0.3, x: 0, y: 0 };
	let box = { w: 0, h: 0 };
	let touched = false;
	let anim = 0;
	const wide = () => box.w > 900;
	/** Screen x of the world origin: left of centre when the side panel takes the right edge. */
	const centreX = () => (wide() ? (box.w - 300) / 2 : box.w / 2);

	function fitView() {
		const span = Math.min(wide() ? box.w - 360 : box.w * 0.95, box.h);
		return { s: Math.max(0.02, span / (2 * (graph?.extent ?? 1750))), x: centreX(), y: box.h / 2 };
	}

	function applyView() {
		const { s, x, y } = v;
		world?.setAttribute("transform", `translate(${x},${y}) scale(${s})`);
		const ramp = (a: number, b: number) => Math.min(1, Math.max(0, (s - a) / (b - a)));
		const schema = ramp(0.36, 0.52);
		const obj = ramp(1.4, 1.9);
		const set = (k: keyof typeof layers, o: number) => {
			const el = layers[k];
			if (!el) return;
			el.style.opacity = String(o);
			// Invisible layers leave the render tree: nothing to repaint while panning.
			el.style.display = o <= 0 ? "none" : "";
			// Object labels sit on the dots and must never take their pointer.
			el.style.pointerEvents = o < 0.3 || k === "obj" ? "none" : "auto";
		};
		set("far", 1 - schema);
		set("schema", schema);
		set("obj", obj);
		set("schemaLabels", schema * (1 - obj * 0.35));
		scheduleLabels();
		if (zoomLabel) zoomLabel.textContent = `${Math.round(s * 100)}%`;
		const next = s < 0.44 ? 0 : s < 1.65 ? 1 : 2;
		if (next !== level) level = next;
	}

	/** Eased zoom to scale `s2` centred on world point (cx, cy). */
	function zoomTo(s2: number, cx: number, cy: number) {
		cancelAnimationFrame(anim);
		touched = true;
		const from = { ...v };
		const t0 = performance.now();
		const fc = { x: (centreX() - from.x) / from.s, y: (box.h / 2 - from.y) / from.s };
		const step = (now: number) => {
			const k = Math.min(1, (now - t0) / 700);
			const e = k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
			const s = Math.exp(Math.log(from.s) + (Math.log(s2) - Math.log(from.s)) * e);
			v = { s, x: centreX() - (fc.x + (cx - fc.x) * e) * s, y: box.h / 2 - (fc.y + (cy - fc.y) * e) * s };
			applyView();
			if (k < 1) anim = requestAnimationFrame(step);
		};
		anim = requestAnimationFrame(step);
	}

	function goLevel(l: number) {
		if (!graph) return;
		const fit = fitView();
		if (l === 0) zoomTo(fit.s, 0, 0);
		else if (l === 1) zoomTo(Math.max(0.62, fit.s * 1.9), 0, 0);
		else if (selObj) zoomTo(2.3, selObj.x, selObj.y);
		else {
			const t = selType ?? graph.types.reduce((a, b) => (b.objects.length > a.objects.length ? b : a));
			zoomTo(2.3, t.x, t.y);
		}
	}

	// Fit on load and on resize until the user takes over; ?focus zooms to its object.
	// Only the stage, the graph and the focus re-run it: the view writes `level` and
	// `pin`, and tracking those would re-fit (and cancel) every zoom the user starts.
	$effect(() => {
		const el = stage;
		const g = graph;
		const focus = focusId;
		if (!el || !g) return;
		const refit = () => {
			const r = el.getBoundingClientRect();
			if (r.width === 0 || r.height === 0) return;
			box = { w: r.width, h: r.height };
			if (!touched) {
				v = fitView();
				applyView();
			}
		};
		untrack(() => {
			touched = false;
			refit();
			for (const t of focus ? g.types : []) {
				const j = t.objects.findIndex((o) => o.id === focus);
				if (j < 0) continue;
				pin = { k: "obj", t: t.index, j };
				zoomTo(2.3, t.objects[j].x, t.objects[j].y);
				break;
			}
		});
		const ro = new ResizeObserver(refit);
		ro.observe(el);
		return () => {
			ro.disconnect();
			cancelAnimationFrame(anim);
		};
	});

	// ── Pointer: drag pans, scroll zooms about the cursor, click pins what is under it ──
	let drag: { x: number; y: number; vx: number; vy: number; moved: boolean } | null = null;
	let dragging = $state(false);

	/** What is under the screen point: a type, a property, or (zoomed in) the object whose dot it is. */
	function hitAt(clientX: number, clientY: number): Sel | null {
		const target = document.elementFromPoint(clientX, clientY);
		const d = target instanceof Element ? (target as SVGElement).dataset : undefined;
		if (!d?.k) return null;
		if (d.k === "type") return { k: "type", t: Number(d.t) };
		if (d.k === "prop") return { k: "prop", p: d.p! };
		const t = Number(d.t);
		// Dots are too small to aim at until zoomed in: before that they stand for their type.
		if (v.s <= 1.2 || !graph || !stage) return { k: "type", t };
		// A type's dots are one shape: the nearest dot to the point is the one hit.
		const r = stage.getBoundingClientRect();
		const wx = (clientX - r.left - v.x) / v.s;
		const wy = (clientY - r.top - v.y) / v.s;
		let j = 0;
		let best = Infinity;
		graph.types[t].objects.forEach((o, i) => {
			const dd = (o.x - wx) ** 2 + (o.y - wy) ** 2;
			if (dd < best) {
				best = dd;
				j = i;
			}
		});
		return { k: "obj", t, j };
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		touched = true;
		cancelAnimationFrame(anim);
		const r = stage!.getBoundingClientRect();
		const mx = e.clientX - r.left;
		const my = e.clientY - r.top;
		const s2 = Math.min(7, Math.max(0.02, v.s * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0016))));
		const f = s2 / v.s;
		v = { s: s2, x: mx - (mx - v.x) * f, y: my - (my - v.y) * f };
		applyView();
	}

	function onDown(e: PointerEvent) {
		touched = true;
		cancelAnimationFrame(anim);
		drag = { x: e.clientX, y: e.clientY, vx: v.x, vy: v.y, moved: false };
		stage!.setPointerCapture(e.pointerId);
	}

	function onMove(e: PointerEvent) {
		if (drag) {
			const dx = e.clientX - drag.x;
			const dy = e.clientY - drag.y;
			if (Math.abs(dx) + Math.abs(dy) > 4) drag.moved = dragging = true;
			if (drag.moved) {
				v.x = drag.vx + dx;
				v.y = drag.vy + dy;
				applyView();
				return;
			}
		}
		// Captured pointers report the stage as target: hit-test the point instead.
		const next = hitAt(e.clientX, e.clientY);
		if (JSON.stringify(next) !== JSON.stringify(hover)) hover = next;
	}

	function onUp() {
		if (drag && !drag.moved) pin = hover;
		drag = null;
		dragging = false;
	}

	function onDblClick(e: MouseEvent) {
		const hit = hitAt(e.clientX, e.clientY);
		if (hit?.k === "obj" && graph) void goto(`/app/object/${graph.types[hit.t].objects[hit.j].id}`);
	}

	// ── Panel ──
	const pct = (u: number) => `${Math.round(u * 100)}%`;
	const relByKey = $derived(new Map(relations.map((r) => [r.key, r])));
	const nameById = $derived(new Map(store.summaries.map((s) => [s.id, s.name])));

	function formatValue(rel: RelationDefJSON | undefined, value: ValueJSON): string {
		const items = value.valuesValue ? value.valuesValue.items : [value];
		return items
			.map((it) => {
				if (it.linkValue) return nameById.get(it.linkValue.targetId) || "Untitled";
				if (it.boolValue !== undefined) return it.boolValue ? "Yes" : "No";
				if (it.intValue !== undefined) return rel?.format === "date" ? new Date(it.intValue).toLocaleDateString() : String(it.intValue);
				if (it.floatValue !== undefined) return String(it.floatValue);
				const s = it.stringValue ?? "";
				if (rel?.format === "object") return nameById.get(s) || "Untitled";
				return rel?.options.find((o) => o.id === s)?.text ?? s;
			})
			.filter(Boolean)
			.join(", ");
	}

	/** A property as the graph names it everywhere: its emoji, then its name. */
	function propLabel(rel: { iconEmoji?: string; name: string; key: string } | undefined, key: string): string {
		return rel ? `${rel.iconEmoji ? `${rel.iconEmoji} ` : ""}${rel.name || rel.key}` : key;
	}

	function objectFields(o: GraphObject) {
		return [...o.held].map((k) => ({ k: propLabel(relByKey.get(k), k), v: formatValue(relByKey.get(k), o.fields[k]) }));
	}
</script>

<svelte:head><title>Graph — Roostr</title></svelte:head>

<div class="stage">
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="canvas" bind:this={stage} onwheel={onWheel} onpointerdown={onDown} onpointermove={onMove} onpointerup={onUp} ondblclick={onDblClick} class:dragging>
		{#if graph}
			{@const g = graph}
			<svg width="100%" height="100%">
				<g bind:this={world}>
					<g bind:this={layers.schema}>
						<g class="screen">
							<!-- One line per object per property it holds, drawn as one shape per type; the highlighted ones go over the top. -->
							{#each g.types as t (t.key)}
								<path d={edgeGeo[t.index]?.all ?? ""} fill="none" stroke={t.color} stroke-linecap="round" stroke-width="0.7" stroke-opacity={sel ? 0.025 : 0.12} />
							{/each}
							{#each litEdges as e, i (i)}
								<path d={e.d} fill="none" stroke={e.color} stroke-linecap="round" stroke-width="1.4" stroke-opacity="0.6" />
							{/each}
						</g>
						{#if selType && sel?.k === "type"}
							{#each g.props as p (p.key)}
								{#if !selType.usage.has(p.key)}
									<path d={edgePath(selType, p)} fill="none" stroke={selType.color} stroke-opacity="0.28" stroke-width="1.2" stroke-dasharray="3 9" />
								{/if}
							{/each}
						{/if}
						{#each g.props as p (p.key)}
							{#each p.targets as tt (tt.key)}
								{@const ex = tt.x - Math.cos(tt.a) * tt.rad}
								{@const ey = tt.y - Math.sin(tt.a) * tt.rad}
								{@const lit = !!sel && (selProp === p || (!!selType && !selObj && selType.usage.has(p.key)) || (!!selObj && selObj.held.has(p.key)))}
								<path d="M{p.x},{p.y} Q{(p.x + ex) * 0.15},{(p.y + ey) * 0.15} {ex},{ey}" fill="none" stroke="#e7e9ee" stroke-opacity={lit ? 0.55 : sel ? 0.03 : 0.1} stroke-width={lit ? 2 : 1.2} stroke-dasharray="2 7" stroke-linecap="round" />
							{/each}
						{/each}
						<text y="-10" text-anchor="middle" class="sans" font-size="30" font-weight="500" fill="#c9cfdc" pointer-events="none">{g.props.length} properties in use</text>
						<text y="26" text-anchor="middle" class="sans" font-size="18" fill="#6f7686" pointer-events="none">any type can hold any of them</text>
					</g>

					<g bind:this={layers.far}>
						<circle r={g.rp} fill="rgba(255,255,255,0.02)" stroke="rgba(255,255,255,0.18)" stroke-width="3" stroke-dasharray="4 14" />
						<text y="-18" text-anchor="middle" class="sans" font-size="96" font-weight="500" fill="#e7e9ee">{g.props.length}</text>
						<text y="58" text-anchor="middle" class="mono" font-size="30" letter-spacing="4" fill="#7d8496">SHARED PROPERTIES</text>
						{#each g.types as t (t.key)}
							{@const on = typeOn(t)}
							{@const l = radial(t.a, g.rt + t.rad + 40, true)}
							<line x1={Math.cos(t.a) * (g.rp + 16)} y1={Math.sin(t.a) * (g.rp + 16)} x2={t.x - Math.cos(t.a) * (t.rad + 8)} y2={t.y - Math.sin(t.a) * (t.rad + 8)} stroke={t.color} stroke-opacity={on ? 0.45 : 0.08} stroke-width={2 + t.usage.size * 2.2} stroke-linecap="round" />
							<text x={l.x} y={l.y} text-anchor={l.anchor} transform={l.transform} dominant-baseline="central" class="sans" font-size="60" letter-spacing="3" data-k="type" data-t={t.index}
								><tspan fill={on ? "#e7e9ee" : "#4a4f5c"} font-weight="500" data-k="type" data-t={t.index}>{t.icon ? `${t.icon} ` : ""}{t.name.toUpperCase()}</tspan><tspan fill={on ? t.color : "#3a3f4a"} dx="18" data-k="type" data-t={t.index}>{t.objects.length}</tspan></text
							>
						{/each}
					</g>

					{#each g.types as t (t.key)}
						{@const isSel = selType === t}
						<g opacity={typeOn(t) ? 1 : 0.18}>
							<circle cx={t.x} cy={t.y} r={t.rad} fill={t.color} fill-opacity={isSel ? 0.14 : 0.06} stroke={t.color} stroke-opacity={isSel ? 0.7 : 0.22} stroke-width={isSel ? 2.5 : 1.5} data-k="type" data-t={t.index} />
							<path d={dotGeo[t.index] ?? ""} fill={t.color} data-k="obj" data-t={t.index} />
							{#if selObj && isSel}
								<circle cx={selObj.x} cy={selObj.y} r="6.5" fill={t.color} stroke="#fff" stroke-width="1.5" pointer-events="none" />
							{/if}
						</g>
					{/each}

					<!-- The selected object's connections to other objects: solid out (it points at them), dashed in (they point at it). -->
					{#if selObj && selType}
						<g pointer-events="none">
							{#each selLinks.out as l (l.o.id)}
								<path d="M{selObj.x},{selObj.y} Q{(selObj.x + l.o.x) * 0.35},{(selObj.y + l.o.y) * 0.35} {l.o.x},{l.o.y}" fill="none" stroke={selType.color} stroke-width="1.6" stroke-opacity="0.85" />
								<circle cx={l.o.x} cy={l.o.y} r="6" fill={l.t.color} stroke="#fff" stroke-width="1.2" />
							{/each}
							{#each selLinks.inc as l (l.o.id)}
								<path d="M{l.o.x},{l.o.y} Q{(selObj.x + l.o.x) * 0.35},{(selObj.y + l.o.y) * 0.35} {selObj.x},{selObj.y}" fill="none" stroke={l.t.color} stroke-width="1.6" stroke-opacity="0.85" stroke-dasharray="5 4" />
								<circle cx={l.o.x} cy={l.o.y} r="6" fill={l.t.color} stroke="#fff" stroke-width="1.2" />
							{/each}
						</g>
					{/if}

					<g bind:this={layers.schemaLabels}>
						{#each g.props as p (p.key)}
							{@const on = propOn(p)}
							{@const idle = !!selType && !selType.usage.has(p.key)}
							{@const r = propRadius(p)}
							{@const l = radial(p.a, g.rp - r - 14, false)}
							<circle cx={p.x} cy={p.y} r={r + 8} fill="transparent" data-k="prop" data-p={p.key} />
							<circle cx={p.x} cy={p.y} {r} fill={on && !idle ? "#e9ebf0" : "#0c0e13"} stroke={idle ? "rgba(255,255,255,0.35)" : on ? "#e9ebf0" : "#3a3f4a"} stroke-width="2" stroke-dasharray={idle ? "3 3" : undefined} pointer-events="none" />
							{#if p.targets.length}
								<circle cx={p.x} cy={p.y} r={r + 5} fill="none" stroke={on ? "#e9ebf0" : "#3a3f4a"} stroke-opacity="0.5" stroke-width="1" pointer-events="none" />
							{/if}
							<text x={l.x} y={l.y} text-anchor={l.anchor} transform={l.transform} dominant-baseline="central" class="mono" font-size="17" fill={idle ? "#6f7686" : on ? "#dfe2ea" : "#3f4450"} data-k="prop" data-p={p.key}>{propLabel(relByKey.get(p.key), p.name)}</text>
						{/each}
						{#each g.types as t (t.key)}
							{@const on = typeOn(t)}
							{@const l = radial(t.a, g.rt + t.rad + 18, true)}
							<text x={l.x} y={l.y} text-anchor={l.anchor} transform={l.transform} dominant-baseline="central" class="sans" font-size="22" data-k="type" data-t={t.index}
								><tspan fill={on ? "#e7e9ee" : "#4f5461"} font-weight="500" data-k="type" data-t={t.index}>{t.icon ? `${t.icon} ` : ""}{t.name}</tspan><tspan fill={on ? t.color : "#3d424d"} dx="8" class="mono" font-size="17" data-k="type" data-t={t.index}>{t.objects.length}</tspan></text
							>
						{/each}
					</g>

					<g bind:this={layers.obj} pointer-events="none">
						{#each labelObjs as o (o.id)}
							<text x={o.x + 6.5} y={o.y} font-size="4.2" class="sans" fill={selObj === o ? "#fff" : "#c6cbd6"} dominant-baseline="central">{o.name}</text>
						{/each}
					</g>
				</g>
			</svg>
		{/if}
	</div>

	{#if status}<p class="status">{status}</p>{/if}

	{#if graph && !status}
		<div class="intro">
			<div class="mono eyebrow">Object graph</div>
			<div class="lede">Everything is an object. Types group objects. Properties are shared: any type can hold any of them, and lines appear only where they’re used.</div>
		</div>

		<aside class="panel">
			{#if selObj && selType}
				<div class="col">
					<div class="obj-head">
						<div class="row"><span class="dot" style:background={selType.color}></span><span class="mono kicker">{selType.icon ? `${selType.icon} ` : ""}{selType.name.toUpperCase()}</span></div>
						<div class="title">{selObj.name}</div>
					</div>
					<div>
						{#each objectFields(selObj) as f (f.k)}
							<div class="field"><span class="mono fk">{f.k}</span><span class="fv">{f.v}</span></div>
						{/each}
					</div>
					<div class="note">{graph.props.length + graph.idle.length - selObj.held.size} other properties available, empty</div>
					{#if selLinks.out.length || selLinks.inc.length}
						<div>
							{#if selLinks.out.length}<div class="field"><span class="mono fk">Links to</span><span class="fv">{selLinks.out.map((l) => l.o.name).join(", ")}</span></div>{/if}
							{#if selLinks.inc.length}<div class="field"><span class="mono fk">Linked from</span><span class="fv">{selLinks.inc.map((l) => l.o.name).join(", ")}</span></div>{/if}
						</div>
					{/if}
					<a class="open" href="/app/object/{selObj.id}">Open object</a>
				</div>
			{:else if selType}
				<div class="col">
					<div class="row">
						<span class="dot big" style:background={selType.color}></span>
						<span class="title grow">{selType.icon ? `${selType.icon} ` : ""}{selType.name}</span>
						<span class="mono muted small">{selType.objects.length} objects</span>
					</div>
					<div class="mono kicker">IN USE · {selType.usage.size} OF {graph.props.length + graph.idle.length}</div>
					<div class="bars">
						{#each [...selType.usage].sort((a, b) => b[1] - a[1]) as [key, u] (key)}
							<div class="bar-row">
								<span class="bar-name">{propLabel(relByKey.get(key), key)}</span>
								<span class="track"><span class="fill" style:width={pct(u)} style:background={selType.color}></span></span>
								<span class="mono muted small right">{pct(u)}</span>
							</div>
						{/each}
					</div>
					<div class="mono kicker">AVAILABLE, UNUSED</div>
					<div class="chips">
						{#each [...graph.props.filter((p) => !selType.usage.has(p.key)).map((p) => propLabel(relByKey.get(p.key), p.name)), ...graph.idle.map((r) => propLabel(r, r.key))] as name (name)}
							<span class="mono chip">{name}</span>
						{/each}
					</div>
				</div>
			{:else if selProp}
				{@const users = [...selProp.users].sort((a, b) => b.usage.get(selProp.key)! * b.objects.length - a.usage.get(selProp.key)! * a.objects.length)}
				<div class="col">
					<div class="row baseline">
						<span class="mono title grow">{selProp.icon ? `${selProp.icon} ` : ""}{selProp.name}</span>
						<span class="mono muted small">{selProp.targets.length ? `→ ${selProp.targets.map((t) => `${t.icon ? `${t.icon} ` : ""}${t.name}`).join(", ")}` : selProp.format}</span>
					</div>
					<div class="mono kicker">USED BY {users.length} OF {graph.types.length} TYPES</div>
					<div class="bars">
						{#each users as t (t.key)}
							{@const u = t.usage.get(selProp.key)!}
							<div class="bar-row with-dot">
								<span class="dot" style:background={t.color}></span>
								<span class="bar-name">{t.icon ? `${t.icon} ` : ""}{t.name}</span>
								<span class="track"><span class="fill" style:width={pct(u)} style:background={t.color}></span></span>
								<span class="mono muted small right">{pct(u)}</span>
							</div>
						{/each}
					</div>
					<div class="note">{users.length === graph.types.length ? "Used by every type." : `The other ${graph.types.length - users.length} types can hold ${selProp.name} too; none of their objects do yet.`}</div>
				</div>
			{:else}
				<div class="col">
					<div class="totals">
						<div><div class="num">{graph.types.length}</div><div class="mono kicker">TYPES</div></div>
						<div><div class="num">{graph.objectCount}</div><div class="mono kicker">OBJECTS</div></div>
						<div><div class="num">{graph.props.length}</div><div class="mono kicker">PROPERTIES</div></div>
					</div>
					<div class="note">Hover a type, property or object. Click to pin. Scroll to zoom. Double-click an object to open it.</div>
				</div>
			{/if}
		</aside>

		<div class="levels">
			<div class="hint">{LEVELS[level][1]}</div>
			<div class="pill">
				{#each LEVELS as [label], i (label)}
					<button class:on={i === level} onclick={() => goLevel(i)}><span class="mono n">{i + 1}</span>{label}</button>
				{/each}
				<span class="mono zoom" bind:this={zoomLabel}>100%</span>
			</div>
		</div>
	{/if}
</div>

<style>
	.stage {
		position: relative;
		width: 100%;
		height: calc(100vh - 120px);
		min-height: 400px;
		overflow: hidden;
		border-radius: 12px;
		background: radial-gradient(ellipse at 50% 45%, #141823 0%, #0c0e13 70%);
		color: #e7e9ee;
		user-select: none;
	}
	.mono {
		font-family: ui-monospace, "SF Mono", Menlo, monospace;
	}
	.sans {
		font-family: inherit;
	}
	.canvas {
		position: absolute;
		inset: 0;
		cursor: grab;
		touch-action: none;
	}
	.canvas.dragging {
		cursor: grabbing;
	}
	.canvas svg {
		display: block;
	}
	.screen {
		mix-blend-mode: screen;
	}
	.status {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		color: #8a90a0;
		pointer-events: none;
	}
	.intro {
		position: absolute;
		top: 24px;
		left: 28px;
		max-width: 320px;
		pointer-events: none;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.eyebrow {
		font-size: 11px;
		letter-spacing: 0.14em;
		color: #7d8496;
		text-transform: uppercase;
	}
	.lede {
		font-size: 15px;
		line-height: 1.5;
		color: #b9bfcc;
		text-wrap: pretty;
	}
	.panel {
		position: absolute;
		top: 24px;
		right: 24px;
		width: 300px;
		max-height: calc(100% - 140px);
		overflow: auto;
		box-sizing: border-box;
		background: rgba(20, 23, 31, 0.86);
		backdrop-filter: blur(10px);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 14px;
		padding: 18px 18px 16px;
	}
	.col {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.row.baseline {
		align-items: baseline;
	}
	.grow {
		flex: 1;
		min-width: 0;
	}
	.title {
		font-size: 18px;
		font-weight: 600;
		overflow-wrap: anywhere;
	}
	.kicker {
		font-size: 10px;
		letter-spacing: 0.12em;
		color: #7d8496;
	}
	.muted {
		color: #8a90a0;
	}
	.small {
		font-size: 11px;
	}
	.right {
		text-align: right;
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
	}
	.dot.big {
		width: 12px;
		height: 12px;
	}
	.bars {
		display: flex;
		flex-direction: column;
		gap: 7px;
	}
	.bar-row {
		display: grid;
		grid-template-columns: 96px 1fr 36px;
		align-items: center;
		gap: 10px;
		font-size: 13px;
	}
	.bar-row.with-dot {
		grid-template-columns: 8px 104px 1fr 36px;
		gap: 9px;
	}
	.bar-name {
		color: #d6dae3;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.track {
		height: 4px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.06);
		overflow: hidden;
	}
	.fill {
		display: block;
		height: 100%;
		border-radius: 2px;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}
	.chip {
		font-size: 11px;
		color: #6f7686;
		border: 1px dashed rgba(255, 255, 255, 0.14);
		border-radius: 5px;
		padding: 2px 6px;
	}
	.note {
		font-size: 12px;
		line-height: 1.5;
		color: #7d8496;
	}
	.obj-head {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.obj-head .kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		color: #8a90a0;
	}
	.field {
		display: grid;
		grid-template-columns: 90px 1fr;
		gap: 10px;
		padding: 6px 0;
		border-top: 1px solid rgba(255, 255, 255, 0.05);
		font-size: 13px;
	}
	.fk {
		font-size: 11px;
		color: #7d8496;
		padding-top: 1px;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.fv {
		color: #dde1e9;
		overflow-wrap: anywhere;
	}
	.open {
		align-self: flex-start;
		font-size: 13px;
		color: #c9cfe0;
	}
	.open:hover {
		color: #fff;
	}
	.totals {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 8px;
	}
	.num {
		font-size: 22px;
		font-weight: 500;
	}
	.levels {
		position: absolute;
		left: 50%;
		bottom: 24px;
		transform: translateX(-50%);
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
	}
	.hint {
		font-size: 13px;
		color: #8a90a0;
		text-align: center;
		pointer-events: none;
	}
	.pill {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 4px;
		background: rgba(20, 23, 31, 0.9);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 999px;
	}
	.pill button {
		display: flex;
		align-items: center;
		gap: 8px;
		border: 0;
		cursor: pointer;
		border-radius: 999px;
		padding: 8px 16px;
		font: inherit;
		font-size: 13px;
		font-weight: 500;
		background: transparent;
		color: #9aa0ae;
	}
	.pill button.on {
		background: #e7e9ee;
		color: #0c0e13;
	}
	.n {
		font-size: 10px;
		opacity: 0.6;
	}
	.zoom {
		font-size: 11px;
		color: #6f7686;
		padding: 0 12px 0 6px;
		min-width: 40px;
		text-align: right;
	}
	/* Narrow screens: the panel and intro would cover the graph; the panel tucks under the top edge. */
	@media (max-width: 900px) {
		.intro {
			display: none;
		}
		.panel {
			left: 12px;
			right: 12px;
			top: 12px;
			width: auto;
			max-height: 38%;
		}
	}
</style>
