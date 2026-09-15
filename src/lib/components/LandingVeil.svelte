<script lang="ts">
	/**
	 * The landing veil: a brometal/WebGPU scene — a lattice globe with two
	 * counter-rotating helical streams (human knowledge descending, agent
	 * work rising) braiding through the same substrate. Pointer parallax.
	 * Degrades to nothing where WebGPU is unavailable (the copy still reads).
	 */
	import { onMount } from "svelte";
	import { createRenderer, createProgram, mat4 } from "brometal";
	import heroVeil from "$lib/shaders/hero-veil.shader.gen";

	let canvas = $state<HTMLCanvasElement>();

	const LATTICE = 680;
	const STREAM = 380;
	const N = LATTICE + STREAM * 2;

	/** Cheap deterministic seeds so the scene is identical every load. */
	const rnd = (k: number) => {
		const x = Math.sin(k * 127.1 + 311.7) * 43758.5453;
		return x - Math.floor(x);
	};

	onMount(() => {
		if (!canvas) return;
		let cancelled = false;
		let stop: (() => void) | null = null;

		const corners = new Float32Array([-1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1, 1]);
		const pos = new Float32Array(N * 3);
		const kind = new Float32Array(N);
		const seed = new Float32Array(N);

		// Fibonacci sphere: an even lattice with no poles to clump at.
		const golden = Math.PI * (3 - Math.sqrt(5));
		let i = 0;
		for (let k = 0; k < LATTICE; k++, i++) {
			const y = 1 - (k / (LATTICE - 1)) * 2;
			const r = Math.sqrt(Math.max(0, 1 - y * y));
			const th = golden * k;
			pos[i * 3] = Math.cos(th) * r;
			pos[i * 3 + 1] = y;
			pos[i * 3 + 2] = Math.sin(th) * r;
			kind[i] = 0;
			seed[i] = rnd(k * 11 + 1);
		}
		for (let k = 0; k < STREAM; k++, i++) {
			kind[i] = 1;
			seed[i] = rnd(k * 3 + 17);
		}
		for (let k = 0; k < STREAM; k++, i++) {
			kind[i] = 2;
			seed[i] = rnd(k * 7 + 41);
		}

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
				renderer = await createRenderer(canvas!, { clearColor: [0.008, 0.01, 0.018, 1] });
			} catch {
				return; // no WebGPU: the hero copy carries the page alone
			}
			if (cancelled) {
				renderer.destroy();
				return;
			}
			const program = createProgram(renderer, heroVeil, { blend: "additive" });
			program.attributes.aCorner.set(corners);
			program.instanceAttributes.iPos.set(pos);
			program.instanceAttributes.iKind.set(kind);
			program.instanceAttributes.iSeed.set(seed);

			stop = renderer.loop((t) => {
				mx += (tx - mx) * 0.045;
				my += (ty - my) * 0.045;
				const proj = mat4.perspective(Math.PI / 4.6, renderer.aspect, 0.1, 100);
				const view = mat4.lookAt([0, 0.4, 5.4], [0, 0, 0], [0, 1, 0]);
				program.uniforms.uViewProj.set(mat4.multiply(proj, view));
				program.uniforms.uTime.set(t);
				program.uniforms.uMouse.set([mx, my]);
				program.draw();
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
