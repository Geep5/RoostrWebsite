<script lang="ts">
	/**
	 * The landing scene: the workspace through time, as a tall glass. Each
	 * height level is a 2D slice of the object web at a moment - warm human
	 * objects, cool agent objects - stacked and slowly swirled, with
	 * near-vertical tracks following each node between slices. Degrades to
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

	interface Entity {
		kind: number;
		birth: number;
		x: number;
		z: number;
		dx: number;
		dz: number;
		links: number[];
	}

	interface StackData {
		pos: Float32Array;
		kind: Float32Array;
		seed: Float32Array;
		starts: Float32Array;
		ends: Float32Array;
		tints: Float32Array;
	}

	function buildStack(): StackData {
		const pos: number[] = [];
		const kind: number[] = [];
		const seed: number[] = [];
		const starts: number[] = [];
		const ends: number[] = [];
		const tints: number[] = [];

		// Persistent entities: born over the first two-thirds of the stack,
		// homes on the unit disc, drifting a little per slice.
		const entities: Entity[] = [];
		for (let i = 0; i < 52; i++) {
			const a = rnd() * Math.PI * 2;
			const r = 0.14 + rnd() * 0.5;
			entities.push({
				kind: rnd() < 0.55 ? 0 : 1,
				birth: Math.floor(rnd() * (SLICES * 0.7)),
				x: Math.cos(a) * r,
				z: Math.sin(a) * r,
				dx: (rnd() - 0.5) * 0.06,
				dz: (rnd() - 0.5) * 0.06,
				links: [],
			});
			// Link to 1-2 earlier entities: the web grows connected.
			for (let l = 0; l < (i > 0 && rnd() < 0.42 ? 2 : 1) && i > 0; l++) {
				const target = Math.floor(rnd() * i);
				if (!entities[i].links.includes(target)) entities[i].links.push(target);
			}
		}

		const yOf = (slice: number) => (slice - (SLICES - 1) / 2) * STEP;
		const posOf = (e: Entity, slice: number): [number, number, number] => {
			const age = slice - e.birth;
			return [e.x + e.dx * age, yOf(slice), e.z + e.dz * age];
		};
		const pushNode = (x: number, y: number, z: number, k: number) => {
			pos.push(x, y, z);
			kind.push(k);
			seed.push(rnd());
		};
		const pushLink = (a: [number, number, number], b: [number, number, number], tint: [number, number, number]) => {
			starts.push(...a);
			ends.push(...b);
			tints.push(...tint);
		};

		for (let k = 0; k < SLICES - 1; k++) {
			for (const [i, e] of entities.entries()) {
				if (e.birth !== k) continue;
				const [x, y, z] = posOf(e, k);
				pushNode(x, y, z, e.kind);
				// Its new links attach down through the stack, to the parents'
				// own planes - each change visibly joins history.
				for (const target of e.links) {
					const p = entities[target];
					pushLink([x, y, z], posOf(p, p.birth), e.kind === 0 ? [0.5, 0.38, 0.22] : [0.25, 0.42, 0.6]);
					pushNode(...posOf(p, p.birth), 3);
				}
				// Some nodes get re-touched later: an edit at a higher plane,
				// bright, with a thread back to where it was born.
				if (rnd() < 0.22 && k + 2 < SLICES) {
					const touch = k + 2 + Math.floor(rnd() * (SLICES - k - 2));
					const [tx, ty, tz] = posOf(e, touch);
					pushNode(tx, ty, tz, 2);
					pushLink([tx, ty, tz], [x, y, z], [0.5, 0.5, 0.62]);
					e.x = tx;
					e.z = tz;
					e.dx = 0;
					e.dz = 0;
					e.birth = touch;
				}
			}
		}

		// The now plane: the whole web as it stands - every node, every
		// connection - that the tail of changes below built up to.
		const NOW = SLICES - 1;
		for (const [i, e] of entities.entries()) {
			if (e.birth > NOW) continue;
			const [x, y, z] = posOf(e, NOW);
			pushNode(x, y, z, e.kind);
			for (const target of e.links) {
				if (target < i && entities[target].birth <= NOW) {
					pushLink([x, y, z], posOf(entities[target], NOW), e.kind === 0 ? [0.55, 0.42, 0.26] : [0.3, 0.48, 0.68]);
				}
			}
		}

		return {
			pos: new Float32Array(pos),
			kind: new Float32Array(kind),
			seed: new Float32Array(seed),
			starts: new Float32Array(starts),
			ends: new Float32Array(ends),
			tints: new Float32Array(tints),
		};
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

			const stack = buildStack();
			const quad = new Float32Array([-1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1, 1]);
			const strip = new Float32Array([0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1]);

			const nodes = createProgram(renderer, heroDag, { blend: "additive" });
			nodes.attributes.aCorner.set(quad);
			nodes.instanceAttributes.iPos.set(stack.pos);
			nodes.instanceAttributes.iKind.set(stack.kind);
			nodes.instanceAttributes.iSeed.set(stack.seed);

			const edges = createProgram(renderer, heroEdges, { blend: "alpha" });
			edges.attributes.aQuad.set(strip);
			edges.instanceAttributes.iStart.set(stack.starts);
			edges.instanceAttributes.iEnd.set(stack.ends);
			edges.instanceAttributes.iTint.set(stack.tints);
			edges.uniforms.uWidth.set(0.008);

			stop = renderer.loop((t) => {
				mx += (tx - mx) * 0.045;
				my += (ty - my) * 0.045;
				const proj = mat4.perspective(Math.PI / 4.4, renderer.aspect, 0.1, 100);
				const view = mat4.lookAt([0, 0.4, 4.4], [0, 0, 0], [0, 1, 0]);
				const viewProj = mat4.multiply(proj, view);
				edges.uniforms.uViewProj.set(viewProj);
				edges.uniforms.uTime.set(t);
				edges.uniforms.uMouse.set([mx, my]);
				edges.draw();
				nodes.uniforms.uViewProj.set(viewProj);
				nodes.uniforms.uTime.set(t);
				nodes.uniforms.uMouse.set([mx, my]);
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
