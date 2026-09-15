import { shader, vec2, vec3, vec4, mod, max, smoothstep, length, abs } from "brometal";

/**
 * The links of the change-DAG: thin quads from each change to its
 * parent(s), wrapped along +x with the nodes. Edges whose endpoints land
 * on opposite sides of the wrap fade out instead of streaking across.
 */
export const HeroEdges = shader({
	attributes: { aQuad: "vec2" },
	instanceAttributes: { iStart: "vec3", iEnd: "vec3", iTint: "vec3" },
	uniforms: { uViewProj: "mat4", uMouse: "vec2", uWidth: "float", uScroll: "float", uWrap: "float" },
	varyings: { vTint: "vec3", vAlong: "float", vFade: "float" },

	vertex({ aQuad, iStart, iEnd, iTint }, { uViewProj, uMouse, uWidth, uScroll, uWrap }, v) {
		v.vTint = iTint;
		v.vAlong = aQuad.y;

		const AHEAD = 3.0;
		const sx = mod(iStart.x + uScroll, uWrap) - uWrap + AHEAD;
		const ex = mod(iEnd.x + uScroll, uWrap) - uWrap + AHEAD;
		// A wrapped edge would stretch the whole tube; kill it at the seam.
		v.vFade = 1.0 - smoothstep(1.2, 1.8, abs(sx - ex));

		const a2 = vec3(sx + uMouse.x * 0.08, iStart.y - uMouse.y * 0.05, 0.0);
		const b2 = vec3(ex + uMouse.x * 0.08, iEnd.y - uMouse.y * 0.05, 0.0);

		const dir = b2.sub(a2);
		const len = length(dir);
		const n = dir.scale(1.0 / max(len, 0.0001));
		const perp = vec3(-n.y, n.x, 0);

		const p = a2.add(n.scale(aQuad.y * len)).add(perp.scale(aQuad.x * uWidth * 0.5));
		return uViewProj.mul(vec4(p, 1));
	},

	fragment(_uniforms, { vTint, vAlong, vFade }) {
		const taper = smoothstep(0.0, 0.08, vAlong) * (1.0 - smoothstep(0.92, 1.0, vAlong));
		return vec4(vTint, taper * 0.85 * vFade);
	},
});
