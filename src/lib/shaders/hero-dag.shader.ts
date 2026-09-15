import { shader, vec2, vec3, vec4, sin, cos, fract, clamp, smoothstep, length } from "brometal";

/**
 * The workspace through time, as a tall glass: each height level is a
 * 2D slice of the object web at a moment - nodes and their connectedness
 * - stacked and slowly swirled, so you can see the structure persist and
 * evolve. Warm = human objects, cool = agent objects, bright = newest.
 */
export const HeroDag = shader({
	attributes: { aCorner: "vec2" },
	instanceAttributes: { iPos: "vec3", iKind: "float", iSeed: "float", iBirth: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2", uNow: "float" },
	varyings: { vUv: "vec2", vColor: "vec3", vAlpha: "float" },

	vertex({ aCorner, iPos, iKind, iSeed, iBirth }, { uViewProj, uTime, uMouse, uNow }, v) {
		v.vUv = aCorner;

		// The build: a change swells in when the present reaches its birth,
		// and the whole sculpture dissolves before the cycle restarts.
		const grown = smoothstep(iBirth, iBirth + 1.6, uNow) * (1.0 - smoothstep(23.2, 25.9, uNow));

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

		// Swirl: the whole stack turns like a tall glass, with a pointer tilt.
		let p = iPos;
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
		const core = 1 - smoothstep(0.5, 1.0, d);
		return vec4(vColor, core * vAlpha);
	},
});
