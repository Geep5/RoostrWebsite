<script lang="ts">
	/**
	 * The landing scene: a living change-DAG. It opens with the web already
	 * built - the full now plane with the tail of changes beneath it - then
	 * keeps growing: live RNG events (a human object, an agent object, an
	 * edit) roll in one by one, attach to the existing web, and history
	 * slowly sinks so the tail stretches downward forever. Nothing is
	 * pregenerated past the seed web and nothing is capped. Degrades to
	 * nothing where WebGPU is unavailable.
	 */
	import { onMount } from "svelte";
	import { createRenderer, createProgram, mat4 } from "brometal";
	import heroDag from "$lib/shaders/hero-dag.shader.gen";
	import heroEdges from "$lib/shaders/hero-edges.shader.gen";

	let canvas = $state<HTMLCanvasElement>();

	/** Deterministic RNG: the same sculpture on every load. */
	const rnd = (() => {
		let s = 20260915;
		return () => {
			s = (s * 1664525 + 1013904223) >>> 0;
			return s / 4294967296;
		};
	})();

	const SLICES = 12;
	const STEP = 0.24;
	const NOW = SLICES - 1;
	/** Seconds between slices in the seed history. */
	const SLICE_SECONDS = 1.6;
	/** The present at first paint: all seed history already grown. */
	const P0 = NOW * SLICE_SECONDS + 2.5;
	/** Downward drift of history, world units per second. Keep in sync with
	 * the uNow sink in hero-dag/hero-edges shaders. */
	const SINK = 0.05;

	interface Entity {
		kind: number; // 0 human, 1 agent
		x: number;
		y: number; // baked, sink-offset included: a welded link endpoint
		z: number;
		degree: number;
	}

	/**
	 * The live scene: GPU arrays that only ever grow, the entity list new
	 * changes attach to, and the event roller. Per-mount so HMR starts clean.
	 */
	function createScene() {
		const pos: number[] = [];
		const kind: number[] = [];
		const seed: number[] = [];
		const births: number[] = [];
		const starts: number[] = [];
		const ends: number[] = [];
		const tints: number[] = [];
		const edgeBirths: number[] = [];
		const entities: Entity[] = [];

		const yOf = (slice: number) => (slice - (SLICES - 1) / 2) * STEP;
		const timeOf = (slice: number) => slice * SLICE_SECONDS;
		/** Plane height where new changes are born, baked for a birth at `at`. */
		const birthY = (at: number) => 0.85 + at * SINK;

		const pushNode = (x: number, y: number, z: number, k: number, birth: number) => {
			pos.push(x, y, z);
			kind.push(k);
			seed.push(rnd());
			births.push(birth);
		};
		const pushLink = (a: [number, number, number], b: [number, number, number], tint: [number, number, number], birth: number) => {
			starts.push(...a);
			ends.push(...b);
			tints.push(...tint);
			edgeBirths.push(birth);
		};

		// --- the seed web: already built at first paint --------------------
		interface SeedEntity {
			kind: number;
			birth: number;
			x: number;
			z: number;
			dx: number;
			dz: number;
			links: number[];
		}
		const seedEntities: SeedEntity[] = [];
		for (let i = 0; i < 52; i++) {
			const a = rnd() * Math.PI * 2;
			const r = 0.14 + rnd() * 0.5;
			seedEntities.push({
				kind: rnd() < 0.55 ? 0 : 1,
				birth: Math.floor(rnd() * (SLICES * 0.7)),
				x: Math.cos(a) * r,
				z: Math.sin(a) * r,
				dx: (rnd() - 0.5) * 0.06,
				dz: (rnd() - 0.5) * 0.06,
				links: [],
			});
			for (let l = 0; l < (i > 0 && rnd() < 0.42 ? 2 : 1) && i > 0; l++) {
				const target = Math.floor(rnd() * i);
				if (!seedEntities[i].links.includes(target)) seedEntities[i].links.push(target);
			}
		}

		// Baked with the P0 sink offset: at uNow = P0 the seed web sits at
		// exactly its designed composition, then sinks with everything else.
		const posOf = (e: SeedEntity, slice: number): [number, number, number] => {
			const age = slice - e.birth;
			return [e.x + e.dx * age, yOf(slice) + P0 * SINK, e.z + e.dz * age];
		};

		for (let k = 0; k < NOW; k++) {
			for (const e of seedEntities) {
				if (e.birth !== k) continue;
				const [x, y, z] = posOf(e, k);
				pushNode(x, y, z, e.kind, timeOf(k));
				// Its new links attach down through the stack, to the parents'
				// own planes - each change visibly joins history.
				for (const target of e.links) {
					const p = seedEntities[target];
					pushLink([x, y, z], posOf(p, p.birth), e.kind === 0 ? [0.5, 0.38, 0.22] : [0.25, 0.42, 0.6], timeOf(k));
					pushNode(...posOf(p, p.birth), 3, timeOf(p.birth));
				}
				// Some nodes get re-touched later: an edit at a higher plane,
				// bright, with a thread back to where it was born.
				if (rnd() < 0.22 && k + 2 < SLICES) {
					const touch = k + 2 + Math.floor(rnd() * (SLICES - k - 2));
					const [tx, ty, tz] = posOf(e, touch);
					pushNode(tx, ty, tz, 2, timeOf(touch));
					pushLink([tx, ty, tz], [x, y, z], [0.5, 0.5, 0.62], timeOf(touch));
					e.x = tx;
					e.z = tz;
					e.dx = 0;
					e.dz = 0;
					e.birth = touch;
				}
			}
		}

		// The now plane: the whole web as it stands - every node, every
		// connection - that the tail of changes below built up to. Each
		// entity's position here is its live endpoint for future changes.
		for (const [i, e] of seedEntities.entries()) {
			if (e.birth > NOW) continue;
			const [x, y, z] = posOf(e, NOW);
			pushNode(x, y, z, e.kind, timeOf(NOW));
			entities.push({ kind: e.kind, x, y, z, degree: e.links.length });
			for (const target of e.links) {
				if (target < i && seedEntities[target].birth <= NOW) {
					pushLink([x, y, z], posOf(seedEntities[target], NOW), e.kind === 0 ? [0.55, 0.42, 0.26] : [0.3, 0.48, 0.68], timeOf(NOW));
				}
			}
		}

		// --- the living web: events rolled live, never pregenerated --------
		/** Preferential attachment: usually someone recent, sometimes a hub. */
		const pickTarget = (exclude: number): number => {
			if (entities.length === 0) return -1;
			if (rnd() < 0.65) {
				const lo = Math.max(0, entities.length - 10);
				const t = lo + Math.floor(rnd() * (entities.length - lo));
				return t === exclude ? -1 : t;
			}
			let best = -1;
			let bestDeg = -1;
			for (let i = 0; i < 3; i++) {
				const t = Math.floor(rnd() * entities.length);
				if (t !== exclude && entities[t].degree > bestDeg) {
					best = t;
					bestDeg = entities[t].degree;
				}
			}
			return best;
		};

		/**
		 * One moment in the workspace, rolled live like a real Roostr DAG:
		 * usually a new object - a human's page or an agent's artifact -
		 * welding itself to 1-3 existing nodes; sometimes an edit, an old
		 * object burning bright again at the top with a thread back down.
		 */
		const fireEvent = (at: number) => {
			const y = birthY(at);
			if (rnd() < 0.88) {
				const k = rnd() < 0.625 ? 0 : 1;
				const a = rnd() * Math.PI * 2;
				const r = 0.14 + rnd() * 0.5;
				const e: Entity = { kind: k, x: Math.cos(a) * r, y, z: Math.sin(a) * r, degree: 0 };
				entities.push(e);
				pushNode(e.x, e.y, e.z, k, at);
				const linkCount = 1 + (rnd() < 0.45 ? 1 : 0) + (rnd() < 0.18 ? 1 : 0);
				for (let l = 0; l < linkCount; l++) {
					const t = pickTarget(entities.length - 1);
					if (t === -1) continue;
					const p = entities[t];
					pushLink([e.x, e.y, e.z], [p.x, p.y, p.z], k === 0 ? [0.5, 0.38, 0.22] : [0.25, 0.42, 0.6], at);
					e.degree++;
					p.degree++;
				}
			} else {
				const t = pickTarget(-1);
				if (t === -1) return;
				const p = entities[t];
				pushNode(p.x, y, p.z, 2, at);
				pushLink([p.x, y, p.z], [p.x, p.y, p.z], [0.5, 0.5, 0.62], at);
				p.degree++;
			}
		};

		return { pos, kind, seed, births, starts, ends, tints, edgeBirths, fireEvent };
	}

	onMount(() => {
		if (!canvas) return;
		let cancelled = false;
		let stop: (() => void) | null = null;

		let mx = 0,
			my = 0,
			tx = 0,
			ty = 0;
		const onMove = (e: PointerEvent) => {
			tx = (e.clientX / window.innerWidth) * 2 - 1;
			ty = (e.clientY / window.innerHeight) * 2 - 1;
		};
		window.addEventListener("pointermove", onMove);

		void (async () => {
			let renderer: Awaited<ReturnType<typeof createRenderer>>;
			try {
				renderer = await createRenderer(canvas!, { clearColor: [0.006, 0.008, 0.016, 1] });
			} catch {
				return;
			}
			if (cancelled) {
				renderer.destroy();
				return;
			}

			const scene = createScene();
			const quad = new Float32Array([-1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1, 1]);
			const strip = new Float32Array([0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1]);

			const nodes = createProgram(renderer, heroDag, { blend: "additive" });
			nodes.attributes.aCorner.set(quad);
			nodes.instanceAttributes.iPos.set(new Float32Array(scene.pos));
			nodes.instanceAttributes.iKind.set(new Float32Array(scene.kind));
			nodes.instanceAttributes.iSeed.set(new Float32Array(scene.seed));
			nodes.instanceAttributes.iBirth.set(new Float32Array(scene.births));

			const edges = createProgram(renderer, heroEdges, { blend: "alpha" });
			edges.attributes.aQuad.set(strip);
			edges.instanceAttributes.iStart.set(new Float32Array(scene.starts));
			edges.instanceAttributes.iEnd.set(new Float32Array(scene.ends));
			edges.instanceAttributes.iTint.set(new Float32Array(scene.tints));
			edges.instanceAttributes.iBirth.set(new Float32Array(scene.edgeBirths));
			edges.uniforms.uWidth.set(0.008);

			let nextEventAt = P0 + 0.8 + rnd() * 1.4;
			let dirty = false;

			stop = renderer.loop((t) => {
				const uNow = P0 + t;
				while (uNow >= nextEventAt) {
					scene.fireEvent(nextEventAt);
					nextEventAt += 0.9 + rnd() * 1.8;
					dirty = true;
				}
				if (dirty) {
					dirty = false;
					nodes.instanceAttributes.iPos.set(new Float32Array(scene.pos));
					nodes.instanceAttributes.iKind.set(new Float32Array(scene.kind));
					nodes.instanceAttributes.iSeed.set(new Float32Array(scene.seed));
					nodes.instanceAttributes.iBirth.set(new Float32Array(scene.births));
					edges.instanceAttributes.iStart.set(new Float32Array(scene.starts));
					edges.instanceAttributes.iEnd.set(new Float32Array(scene.ends));
					edges.instanceAttributes.iTint.set(new Float32Array(scene.tints));
					edges.instanceAttributes.iBirth.set(new Float32Array(scene.edgeBirths));
				}
				mx += (tx - mx) * 0.045;
				my += (ty - my) * 0.045;
				const proj = mat4.perspective(Math.PI / 4.4, renderer.aspect, 0.1, 100);
				const view = mat4.lookAt([0, 0.4, 4.4], [0, 0, 0], [0, 1, 0]);
				const viewProj = mat4.multiply(proj, view);
				edges.uniforms.uViewProj.set(viewProj);
				edges.uniforms.uTime.set(t);
				edges.uniforms.uMouse.set([mx, my]);
				edges.uniforms.uNow.set(uNow);
				edges.draw();
				nodes.uniforms.uViewProj.set(viewProj);
				nodes.uniforms.uTime.set(t);
				nodes.uniforms.uMouse.set([mx, my]);
				nodes.uniforms.uNow.set(uNow);
				nodes.draw();
			});
		})();

		return () => {
			cancelled = true;
			stop?.();
			window.removeEventListener("pointermove", onMove);
		};
	});
</script>

<canvas bind:this={canvas} aria-hidden="true"></canvas>

<style>
	canvas {
		position: absolute;
		inset: 0;
		display: block;
		width: 100%;
		height: 100%;
		min-width: 0;
		min-height: 0;
	}
</style>
