<script lang="ts">
	/**
	 * The landing scene: the glon change-DAG as a WebGPU starfield —
	 * generations of change objects (warm = human, cool = agent, bright =
	 * merges) linked parent to child, slowly turning, with the Roostr mark
	 * at its tip. Degrades to nothing where WebGPU is unavailable.
	 */
	import { onMount } from "svelte";
	import { createRenderer, createProgram, mat4 } from "brometal";
	import heroDag from "$lib/shaders/hero-dag.shader.gen";
	import heroEdges from "$lib/shaders/hero-edges.shader.gen";

	let canvas = $state<HTMLCanvasElement>();

	/** Deterministic RNG so the DAG is the same sculpture on every load. */
	const rnd = (() => {
		let s = 20260914;
		return () => {
			s = (s * 1664525 + 1013904223) >>> 0;
			return s / 4294967296;
		};
	})();

	interface DagData {
		pos: Float32Array;
		kind: Float32Array;
		seed: Float32Array;
		starts: Float32Array;
		ends: Float32Array;
		tints: Float32Array;
		edges: number;
		nodes: number;
	}

	function buildDag(): DagData {
		const LAYERS = 22;
		const SPACING = 0.62;
		const pos: number[] = [];
		const kind: number[] = [];
		const seed: number[] = [];
		const starts: number[] = [];
		const ends: number[] = [];
		const tints: number[] = [];
		const layerIds: number[][] = [];
		let n = 0;
		let e = 0;

		const push = (x: number, y: number, z: number, k: number): number => {
			pos.push(x, y, z);
			kind.push(k);
			seed.push(rnd());
			return n++;
		};
		const link = (child: number, parent: number) => {
			starts.push(pos[child * 3], pos[child * 3 + 1], pos[child * 3 + 2]);
			ends.push(pos[parent * 3], pos[parent * 3 + 1], pos[parent * 3 + 2]);
			// The link takes its child's authorship, dimmed.
			const warm = kind[child] < 0.5;
			const merge = kind[child] > 1.5;
			tints.push(merge ? 0.75 : warm ? 0.5 : 0.25, merge ? 0.75 : warm ? 0.4 : 0.42, merge ? 0.8 : warm ? 0.25 : 0.6);
			e++;
		};

		// Root change.
		layerIds.push([push(0, 0, 0, rnd() < 0.55 ? 0 : 1)]);
		for (let layer = 1; layer < LAYERS; layer++) {
			const prev = layerIds[layer - 1];
			const lx = layer * SPACING;
			const ids: number[] = [];
			// Each previous node usually continues; sometimes a second
			// parent makes a merge; sometimes an extra branch appears.
			for (const [i, parent] of prev.entries()) {
				const px = pos[parent * 3];
				const py = pos[parent * 3 + 1];
				const merge = prev.length > 1 && i > 0 && rnd() < 0.16;
				const child = push(lx, py * 0.9 + (rnd() - 0.5) * 0.5, px * 0.86 + (rnd() - 0.5) * 0.0, merge ? 2 : rnd() < 0.55 ? 0 : 1);
				link(child, parent);
				if (merge) link(child, prev[rnd() < 0.5 ? 0 : prev.length - 1]);
				ids.push(child);
				if (rnd() < 0.14 && ids.length < 7) {
					const branch = push(lx, py * 0.9 + (rnd() - 0.5) * 0.9, px * 0.86 + (rnd() - 0.5) * 0.0, rnd() < 0.5 ? 0 : 1);
					link(branch, parent);
					ids.push(branch);
				}
			}
			layerIds.push(ids);
		}

		return {
			pos: new Float32Array(pos),
			kind: new Float32Array(kind),
			seed: new Float32Array(seed),
			starts: new Float32Array(starts),
			ends: new Float32Array(ends),
			tints: new Float32Array(tints),
			edges: e,
			nodes: n,
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

			const dag = buildDag();
			const quad = new Float32Array([-1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1, 1]);
			const strip = new Float32Array([0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1]);

			const nodes = createProgram(renderer, heroDag, { blend: "additive" });
			nodes.attributes.aCorner.set(quad);
			nodes.instanceAttributes.iPos.set(dag.pos);
			nodes.instanceAttributes.iKind.set(dag.kind);
			nodes.instanceAttributes.iSeed.set(dag.seed);

			const edges = createProgram(renderer, heroEdges, { blend: "alpha" });
			edges.attributes.aQuad.set(strip);
			edges.instanceAttributes.iStart.set(dag.starts);
			edges.instanceAttributes.iEnd.set(dag.ends);
			edges.instanceAttributes.iTint.set(dag.tints);
			edges.uniforms.uWidth.set(0.017);

			stop = renderer.loop((t) => {
				mx += (tx - mx) * 0.045;
				my += (ty - my) * 0.045;
				// True 2D: an orthographic frame, halfH world units tall.
				const halfH = 1.4;
				const halfW = halfH * renderer.aspect;
				const cx = -0.4;
				const cy = 0.1;
				const viewProj = new Float32Array([
					1 / halfW, 0, 0, 0,
					0, 1 / halfH, 0, 0,
					0, 0, -0.02, 0,
					-cx / halfW, -cy / halfH, -1, 1,
				]);
				edges.uniforms.uViewProj.set(viewProj);
				edges.uniforms.uMouse.set([mx, my]);
				edges.uniforms.uScroll.set((t * 0.28) % 7);
				edges.uniforms.uWrap.set(7);
				edges.draw();
				nodes.uniforms.uViewProj.set(viewProj);
				nodes.uniforms.uTime.set(t);
				nodes.uniforms.uMouse.set([mx, my]);
				nodes.uniforms.uScroll.set((t * 0.28) % 7);
				nodes.uniforms.uWrap.set(7);
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
