import { shader, vec2, vec3, vec4, sin, cos, fract, clamp, smoothstep, length, storageRead } from "brometal";

/**
 * The workspace as a living change-DAG. Every node is an entry in the
 * uNodes storage buffer (x, topological y, z, birth time) so edges and
 * depth pushes stay welded: the newest change always sits at the front,
 * and its arrival pushes the nodes it connects down a generation. Warm =
 * human objects, cool = agent objects, bright = re-touches.
 */
export const HeroDag = shader({
	attributes: { aCorner: "vec2" },
	instanceAttributes: { iIdx: "float", iKind: "float", iSeed: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2", uNow: "float" },
	storage: { uNodes: "vec4" },
	varyings: { vUv: "vec2", vColor: "vec3", vAlpha: "float" },

	vertex({ aCorner, iIdx, iKind, iSeed }, { uViewProj, uTime, uMouse, uNow, uNodes }, v) {
		v.vUv = aCorner;
		const node = storageRead(uNodes, iIdx);

		// A change blooms in slowly when the present reaches its birth; the
		// web only ever grows, there is no cycle and no cap.
		const grown = smoothstep(node.w, node.w + 2.8, uNow);

		let color = vec3(1.0, 0.66, 0.3);
		let size = (0.03 + fract(iSeed * 7.3) * 0.012) * (0.25 + 0.75 * grown);
		let glow = (0.8 + fract(iSeed * 4.7) * 0.5) * grown;
		if (iKind > 0.5 && iKind < 1.5) {
			color = vec3(0.35, 0.68, 1.0);
		} else if (iKind > 1.5 && iKind < 2.5) {
			color = vec3(0.95, 0.97, 1.0);
			size = (0.038 + fract(iSeed * 5.1) * 0.008) * (0.25 + 0.75 * grown);
			glow = 1.15 * grown;
		} else if (iKind > 2.5) {
			// Context: the parent a change attaches to - present, not the story.
			color = vec3(0.42, 0.48, 0.62);
			size = 0.016 * (0.25 + 0.75 * grown);
			glow = 0.35 * grown;
		}

		// Age drift: everything settles downward slowly (rate in sync with
		// hero-edges and LandingVeil.svelte); depth pushes move node.y.
		let p = vec3(node.x, node.y - (uNow - node.w) * 0.02, node.z);
		const yaw = uTime * 0.22 + uMouse.x * 0.5;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const px = p.x * cy + p.z * sy;
		const pz0 = p.z * cy - p.x * sy;
		const tilt = 0.42 + uMouse.y * 0.18;
		const ct = cos(tilt);
		const st = sin(tilt);
		const py = p.y * ct - pz0 * st;
		const pz1 = p.y * st + pz0 * ct;
		p = vec3(px, py, pz1);

		const depthFade = clamp((p.z + 3.4) / 4.6, 0.25, 1.0);
		const world = p.add(vec3(aCorner.x * size, aCorner.y * size, 0));
		v.vColor = color;
		v.vAlpha = depthFade * glow;
		return uViewProj.mul(vec4(world, 1));
	},

	fragment(_uniforms, { vUv, vColor, vAlpha }) {
		const d = length(vUv);
		const core = 1.0 - smoothstep(0.0, 0.32, d);
		const halo = (1.0 - smoothstep(0.1, 1.0, d)) * 0.5;
		return vec4(vColor, (core + halo) * vAlpha);
	},
});
