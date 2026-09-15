<script lang="ts">
	/**
	 * The landing scene: a living change-DAG. It opens on the seed web
	 * already built - the full now plane with the tail of changes beneath
	 * it - then grows one node at a time: each new change arrives at the
	 * front, welds itself to 1-3 existing nodes, and PUSHES those
	 * connections down a generation to take its spot. Newest at the front,
	 * always. Node positions live in a GPU storage buffer so edges follow
	 * their endpoints through every push. Nothing pregenerated past the
	 * seed web, nothing capped. Degrades to nothing where WebGPU is
	 * unavailable.
	 */
	import { onMount } from "svelte";
	import { createRenderer, createProgram, createStorageBuffer, mat4 } from "brometal";
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
	const NOW = SLICES - 1;
	/** Where the newest change always sits. */
	const FRONT_Y = 0.85;
	/** One generation down per push. */
	const GAP = 0.24;
	/** Room to grow: memory preallocated, the DAG itself never is. */
	const MAX_NODES = 65536;

	interface Entity {
		kind: number; // 0 human, 1 agent
		node: number; // index into the node storage buffer - its plane node
		degree: number;
	}

	/**
	 * The live scene: GPU arrays that only ever grow, the entity list new
	 * changes attach to, and the push-down roller. Per-mount so HMR starts
	 * clean.
	 */
	function createScene() {
		const nodeData = new Float32Array(MAX_NODES * 4); // x, yTopo, z, birth
		const yCur: number[] = [];
		const yTarget: number[] = [];
		let nNodes = 0;

		const iIdx: number[] = [];
		const iKind: number[] = [];
		const iSeed: number[] = [];
		const eA: number[] = [];
		const eB: number[] = [];
		const eTint: number[] = [];
		const eBirth: number[] = [];
		const entities: Entity[] = [];
		/** Tail markers: the frozen past, sinking away below the plane. */
		const tailNodes: number[] = [];
		const MUTED: [number, number, number] = [0.3, 0.33, 0.4];

		/** Tail height of a seed slice: history below the front plane. */
		const tailY = (slice: number) => FRONT_Y - (NOW - slice) * GAP;

		const addNode = (x: number, yTopo: number, z: number, birth: number, kind: number): number => {
			const idx = nNodes++;
			nodeData[idx * 4] = x;
			nodeData[idx * 4 + 1] = yTopo;
			nodeData[idx * 4 + 2] = z;
			nodeData[idx * 4 + 3] = birth;
			yCur[idx] = yTopo;
			yTarget[idx] = yTopo;
			iIdx.push(idx);
			iKind.push(kind);
			iSeed.push(rnd());
			return idx;
		};
		const addEdge = (a: number, b: number, tint: [number, number, number], birth: number) => {
			eA.push(a);
			eB.push(b);
			eTint.push(...tint);
			eBirth.push(birth);
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
			bornSlice: number;
			bornX: number;
			bornZ: number;
			retouch?: { slice: number; x: number; z: number; node?: number };
		}
		const seed: SeedEntity[] = [];
		for (let i = 0; i < 52; i++) {
			const a = rnd() * Math.PI * 2;
			const r = 0.14 + rnd() * 0.5;
			const x = Math.cos(a) * r;
			const z = Math.sin(a) * r;
			const birth = Math.floor(rnd() * (SLICES * 0.7));
			seed.push({
				kind: rnd() < 0.55 ? 0 : 1,
				birth,
				x,
				z,
				dx: (rnd() - 0.5) * 0.06,
				dz: (rnd() - 0.5) * 0.06,
				links: [],
				bornSlice: birth,
				bornX: x,
				bornZ: z,
			});
			for (let l = 0; l < (i > 0 && rnd() < 0.42 ? 2 : 1) && i > 0; l++) {
				const target = Math.floor(rnd() * i);
				if (!seed[i].links.includes(target)) seed[i].links.push(target);
			}
		}

		// Simulate the slices: some nodes get re-touched later, moving them.
		const posAt = (e: SeedEntity, slice: number): [number, number] => {
			const age = slice - e.birth;
			return [e.x + e.dx * age, e.z + e.dz * age];
		};
		for (const e of seed) {
			if (rnd() < 0.22 && e.birth + 2 < SLICES) {
				const touch = e.birth + 2 + Math.floor(rnd() * (SLICES - e.birth - 2));
				const [tx, tz] = posAt(e, touch);
				e.retouch = { slice: touch, x: tx, z: tz };
				e.x = tx;
				e.z = tz;
				e.dx = 0;
				e.dz = 0;
				e.birth = touch;
			}
		}

		// Nodes: a tail marker where each entity was born, re-touch markers,
		// context dots at link endpoints, and the live plane node at the
		// front that future changes weld to. Seed history is already grown.
		const SEED_BIRTH = -2;
		const tailNode: number[] = [];
		for (const e of seed) {
			const n = addNode(e.bornX, tailY(Math.min(e.bornSlice, NOW - 1)), e.bornZ, SEED_BIRTH, 3);
			tailNode.push(n);
			tailNodes.push(n);
		}
		for (const e of seed) {
			if (e.retouch) {
				const n = addNode(e.retouch.x, tailY(e.retouch.slice), e.retouch.z, SEED_BIRTH, 3);
				e.retouch.node = n;
				tailNodes.push(n);
			}
		}
		for (const e of seed) {
			const [px, pz] = posAt(e, NOW);
			const node = addNode(px, FRONT_Y, pz, SEED_BIRTH, e.kind);
			entities.push({ kind: e.kind, node, degree: e.links.length });
		}

		// Edges: tail births weld to their parents' tail markers (with dim
		// context dots there), re-touches thread down to the birth marker,
		// and the front plane carries every connection as the web stands.
		for (const [i, e] of seed.entries()) {
			for (const target of e.links) {
				const p = seed[target];
				const dot = addNode(p.bornX, tailY(Math.min(p.bornSlice, NOW - 1)), p.bornZ, SEED_BIRTH, 3);
				tailNodes.push(dot);
				addEdge(tailNode[i], tailNode[target], MUTED, SEED_BIRTH);
				if (target < i) {
					const bright: [number, number, number] = e.kind === 0 ? [0.55, 0.42, 0.26] : [0.3, 0.48, 0.68];
					addEdge(entities[i].node, entities[target].node, bright, SEED_BIRTH);
				}
			}
			if (e.retouch) {
				// The thread from the re-touch marker down to the birth marker.
				addEdge(e.retouch.node!, tailNode[i], MUTED, SEED_BIRTH);
			}
		}

		// --- the living web: one node at a time, rolled live ---------------
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
		 * a new object - a human's page or an agent's artifact - drifts into
		 * the now plane and welds itself to 1-3 existing nodes, so the plane
		 * keeps every node ever made and grows forever. The change itself
		 * drops into the tail: a muted marker with muted threads up to the
		 * parents it welded, sinking away into the past.
		 */
		const fireEvent = (at: number): boolean => {
			if (nNodes >= MAX_NODES - 2) return false;
			const k = rnd() < 0.625 ? 0 : 1;
			const a = rnd() * Math.PI * 2;
			// The plane widens a little as it fills.
			const r = 0.14 + rnd() * Math.min(0.62, 0.5 + entities.length * 0.0015);
			const x = Math.cos(a) * r;
			const z = Math.sin(a) * r;
			// Arrive flowy: spawn above the plane and drift down into place.
			const node = addNode(x, FRONT_Y + 0.55, z, at, k);
			yTarget[node] = FRONT_Y;
			const entity: Entity = { kind: k, node, degree: 0 };
			entities.push(entity);
			// The past: same spot, muted, already sinking.
			const marker = addNode(x, FRONT_Y, z, at, 3);
			tailNodes.push(marker);
			const linkCount = 1 + (rnd() < 0.45 ? 1 : 0) + (rnd() < 0.18 ? 1 : 0);
			for (let l = 0; l < linkCount; l++) {
				const t = pickTarget(entities.length - 1);
				if (t === -1) continue;
				const p = entities[t];
				addEdge(node, p.node, k === 0 ? [0.5, 0.38, 0.22] : [0.25, 0.42, 0.6], at);
				addEdge(marker, p.node, MUTED, at);
				entity.degree++;
				p.degree++;
			}
			return true;
		};

		/** History sinks: tail markers drift down, the plane never does. */
		const sinkTail = (dt: number) => {
			for (const i of tailNodes) yTarget[i] -= 0.05 * dt;
		};

		/** Ease every pushed node toward its new depth; true while moving. */
		const ease = (): boolean => {
			let moving = false;
			for (let i = 0; i < nNodes; i++) {
				const d = yTarget[i] - yCur[i];
				if (Math.abs(d) < 0.0004) continue;
				yCur[i] += d * 0.028;
				nodeData[i * 4 + 1] = yCur[i];
				moving = true;
			}
			return moving;
		};

		return {
			nodeData,
			sinkTail,
			get nNodes() {
				return nNodes;
			},
			iIdx,
			iKind,
			iSeed,
			eA,
			eB,
			eTint,
			eBirth,
			fireEvent,
			ease,
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

			const scene = createScene();
			const nodeBuf = createStorageBuffer(renderer, scene.nodeData);
			const quad = new Float32Array([-1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1, 1]);
			const strip = new Float32Array([0, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1]);

			const nodes = createProgram(renderer, heroDag, { blend: "additive" });
			nodes.attributes.aCorner.set(quad);
			nodes.instanceAttributes.iIdx.set(new Float32Array(scene.iIdx));
			nodes.instanceAttributes.iKind.set(new Float32Array(scene.iKind));
			nodes.instanceAttributes.iSeed.set(new Float32Array(scene.iSeed));
			nodes.uniforms.uNodes.set(nodeBuf);

			const edges = createProgram(renderer, heroEdges, { blend: "alpha" });
			edges.attributes.aQuad.set(strip);
			edges.instanceAttributes.iA.set(new Float32Array(scene.eA));
			edges.instanceAttributes.iB.set(new Float32Array(scene.eB));
			edges.instanceAttributes.iTint.set(new Float32Array(scene.eTint));
			edges.instanceAttributes.iBirth.set(new Float32Array(scene.eBirth));
			edges.uniforms.uNodes.set(nodeBuf);
			edges.uniforms.uWidth.set(0.008);

			let nextEventAt = 0.8 + rnd() * 1.2;
			let dirty = false;
			let lastT = 0;

			stop = renderer.loop((t) => {
				const dt = Math.min(t - lastT, 0.1);
				lastT = t;
				while (t >= nextEventAt) {
					if (scene.fireEvent(nextEventAt)) dirty = true;
					nextEventAt += 0.9 + rnd() * 1.8;
				}
				scene.sinkTail(dt);
				if (scene.ease()) {
					nodeBuf.write(scene.nodeData.subarray(0, scene.nNodes * 4));
				}
				if (dirty) {
					dirty = false;
					nodes.instanceAttributes.iIdx.set(new Float32Array(scene.iIdx));
					nodes.instanceAttributes.iKind.set(new Float32Array(scene.iKind));
					nodes.instanceAttributes.iSeed.set(new Float32Array(scene.iSeed));
					edges.instanceAttributes.iA.set(new Float32Array(scene.eA));
					edges.instanceAttributes.iB.set(new Float32Array(scene.eB));
					edges.instanceAttributes.iTint.set(new Float32Array(scene.eTint));
					edges.instanceAttributes.iBirth.set(new Float32Array(scene.eBirth));
				}
				mx += (tx - mx) * 0.045;
				my += (ty - my) * 0.045;
				const proj = mat4.perspective(Math.PI / 4.4, renderer.aspect, 0.1, 100);
				const view = mat4.lookAt([0, 0.4, 4.4], [0, 0, 0], [0, 1, 0]);
				const viewProj = mat4.multiply(proj, view);
				edges.uniforms.uViewProj.set(viewProj);
				edges.uniforms.uTime.set(t);
				edges.uniforms.uMouse.set([mx, my]);
				edges.uniforms.uNow.set(t);
				edges.draw();
				nodes.uniforms.uViewProj.set(viewProj);
				nodes.uniforms.uTime.set(t);
				nodes.uniforms.uMouse.set([mx, my]);
				nodes.uniforms.uNow.set(t);
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
