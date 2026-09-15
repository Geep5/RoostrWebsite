import { shader, vec2, vec3, vec4, sin, mod, max, fract, clamp, smoothstep, length } from "brometal";

/**
 * Debug-bisect: the constellation version (no stream math) - known to render.
 */
export const HeroDag = shader({
	attributes: { aCorner: "vec2" },
	instanceAttributes: { iPos: "vec3", iKind: "float", iSeed: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2", uScroll: "float", uWrap: "float" },
	varyings: { vUv: "vec2", vColor: "vec3", vAlpha: "float" },

	vertex({ aCorner, iPos, iKind, iSeed }, { uViewProj, uTime, uMouse, uScroll, uWrap }, v) {
		v.vUv = aCorner;

		let color = vec3(1.0, 0.66, 0.3);
		let size = 0.034 + fract(iSeed * 7.3) * 0.014;
		let glow = 0.85 + fract(iSeed * 4.7) * 0.55;
		if (iKind > 0.5 && iKind < 1.5) {
			color = vec3(0.35, 0.68, 1.0);
		} else if (iKind > 1.5) {
			color = vec3(0.95, 0.97, 1.0);
			size = 0.034 + fract(iSeed * 5.1) * 0.008;
			glow = 1.1;
		}

		const wrap = max(uWrap, 0.001);
		const x = mod(iPos.x + uScroll, wrap) - wrap + 3.0;
		const born = x + wrap - 3.0;
		// A change swells from nothing over a long first stretch of the frame,
		// so growth reads as growth, not panning.
		const grown = smoothstep(0.0, 4.2, born);
		// Flat: the DAG lives on the z=0 plane; a small pan of parallax, no rotation.
		let p = vec3(x + uMouse.x * 0.08, iPos.y + sin(uTime * 0.6 + iSeed * 6.2832) * 0.02 - uMouse.y * 0.05, 0.0);

		const world = p.add(vec3(aCorner.x * size * grown, aCorner.y * size * grown, 0));
		v.vColor = color;
		v.vAlpha = glow * grown;
		return uViewProj.mul(vec4(world, 1));
	},

	fragment(_uniforms, { vUv, vColor, vAlpha }) {
		const d = length(vUv);
		const core = 1 - smoothstep(0.5, 1.0, d);
		return vec4(vColor, core * vAlpha);
	},
});
