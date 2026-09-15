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

		// The Roostr palette: every object type has its own color and size,
		// like the real space - humans the big pink mass, tasks teal,
		// publishers gold, chats purple. Kind >= 10: the muted past.
		let color = vec3(0.92, 0.42, 0.55); // human
		let sizeMul = 1.0;
		if (iKind > 0.5 && iKind < 1.5) {
			color = vec3(0.35, 0.85, 0.6); // task
		} else if (iKind > 1.5 && iKind < 2.5) {
			color = vec3(0.95, 0.75, 0.8); // game
		} else if (iKind > 2.5 && iKind < 3.5) {
			color = vec3(0.95, 0.72, 0.35); // publisher
			sizeMul = 1.35;
		} else if (iKind > 3.5 && iKind < 4.5) {
			color = vec3(0.65, 0.5, 0.95); // chat
			sizeMul = 0.95;
		} else if (iKind > 4.5 && iKind < 5.5) {
			color = vec3(0.45, 0.7, 1.0); // query
			sizeMul = 0.9;
		} else if (iKind > 5.5 && iKind < 6.5) {
			color = vec3(0.8, 0.42, 0.75); // bookmark
			sizeMul = 0.9;
		} else if (iKind > 6.5 && iKind < 7.5) {
			color = vec3(0.95, 0.85, 0.4); // sponsor
			sizeMul = 1.15;
		} else if (iKind > 7.5 && iKind < 8.5) {
			color = vec3(0.95, 0.6, 0.35); // vendor
			sizeMul = 1.1;
		} else if (iKind > 8.5 && iKind < 9.5) {
			color = vec3(0.75, 0.8, 0.95); // page
			sizeMul = 1.2;
		} else if (iKind > 9.5) {
			// The past: tail markers, muted semi-gray.
			color = vec3(0.42, 0.44, 0.52);
			sizeMul = 0.55;
		}
		let size = (0.03 + fract(iSeed * 7.3) * 0.012) * sizeMul * (0.25 + 0.75 * grown);
		let glowBase = 0.8 + fract(iSeed * 4.7) * 0.5;
		if (iKind > 9.5) {
			glowBase = 0.4;
		}
		let glow = glowBase * grown;

		let p = vec3(node.x, node.y, node.z);
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
