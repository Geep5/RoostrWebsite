import { shader, vec2, vec3, vec4, sin, cos, mod, fract, clamp, smoothstep, length, mix, abs, step } from "brometal";

/**
 * The landing veil: a ghostly lattice globe (the substrate of record)
 * with two counter-rotating helical streams braiding through it - warm
 * embers of human knowledge descending, electric agent-work rising.
 * Where they cross the equator they interleave: the work layer.
 * iKind: 0 lattice, 1 human stream, 2 agent stream.
 */
export const HeroVeil = shader({
	attributes: { aCorner: "vec2" },
	instanceAttributes: { iPos: "vec3", iKind: "float", iSeed: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2" },
	varyings: { vUv: "vec2", vColor: "vec3", vAlpha: "float" },

	vertex({ aCorner, iPos, iKind, iSeed }, { uViewProj, uTime, uMouse }, v) {
		v.vUv = aCorner;

		let p = iPos;
		let size = 0.02;
		let color = vec3(0.55, 0.65, 0.85);
		let glow = 0.5;

		if (iKind < 0.5) {
			// Lattice: the quiet geometry everything else moves through.
			size = 0.013 + iSeed * 0.007;
			color = mix(vec3(0.38, 0.48, 0.68), vec3(0.62, 0.74, 1.0), fract(iSeed * 9.1));
			glow = 0.3 + fract(iSeed * 3.7) * 0.3;
		} else if (iKind < 1.5) {
			// Human knowledge: warm helix flowing down into the layer.
			const a = iSeed * 6.2832 + uTime * 0.42;
			const r = 1.55 + 0.3 * sin(iSeed * 39.7);
			const y = 1.5 - mod(iSeed * 3.7 + uTime * 0.055, 1.0) * 3.0;
			p = vec3(cos(a) * r, y, sin(a) * r);
			size = 0.013 + fract(iSeed * 7.3) * 0.010;
			color = mix(vec3(1.0, 0.58, 0.22), vec3(1.0, 0.82, 0.45), fract(iSeed * 5.9));
			glow = 0.65 + fract(iSeed * 4.7) * 0.55;
		} else {
			// Agents: electric helix rising out of the layer, counter-rotating.
			const a = -(iSeed * 6.2832 + uTime * 0.58);
			const r = 1.55 + 0.3 * cos(iSeed * 41.3);
			const y = -1.5 + mod(iSeed * 2.9 + uTime * 0.075, 1.0) * 3.0;
			p = vec3(cos(a) * r, y, sin(a) * r);
			size = 0.012 + fract(iSeed * 6.1) * 0.009;
			color = mix(vec3(0.3, 0.65, 1.0), vec3(0.55, 0.85, 1.0), fract(iSeed * 8.3));
			glow = 0.7 + fract(iSeed * 4.3) * 0.5;
		}

		// Global slow rotation + pointer parallax (yaw by x, tilt by y).
		const yaw = uTime * 0.07 + uMouse.x * 0.4;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const px = p.x * cy + p.z * sy;
		const pz0 = p.z * cy - p.x * sy;
		const tilt = uMouse.y * 0.22;
		const ct = cos(tilt);
		const st = sin(tilt);
		const py = p.y * ct - pz0 * st;
		const pz1 = p.y * st + pz0 * ct;
		p = vec3(px, py, pz1);

		const depthFade = clamp((p.z + 3.4) / 4.8, 0.12, 1.0);
		// Calm the equatorial crossing band so the copy reads through it.
		const isStream = step(0.5, iKind);
		const centerCalm = mix(1.0, 0.55 + 0.45 * clamp(abs(p.y), 0.0, 1.0), isStream);
		const world = p.add(vec3(aCorner.x * size, aCorner.y * size, 0));
		v.vColor = color;
		v.vAlpha = depthFade * glow * centerCalm;
		return uViewProj.mul(vec4(world, 1));
	},

	fragment(_uniforms, { vUv, vColor, vAlpha }) {
		const d = length(vUv);
		const core = 1 - smoothstep(0.5, 1.0, d);
		return vec4(vColor, core * vAlpha);
	},
});
