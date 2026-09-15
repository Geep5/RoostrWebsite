import { shader, vec2, vec3, vec4, cos, sin, mod, max, min, smoothstep, length, abs } from "brometal";

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

		const yaw = uMouse.x * 0.4;
		const tilt = uMouse.y * 0.22;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const ct = cos(tilt);
		const st = sin(tilt);

		const a = vec3(sx, iStart.y, iStart.z);
		const b = vec3(ex, iEnd.y, iEnd.z);

		const ax = a.x * cy + a.z * sy;
		const az0 = a.z * cy - a.x * sy;
		const ay = a.y * ct - az0 * st;
		const az1 = a.y * st + az0 * ct;
		const a2 = vec3(ax, ay, az1);

		const bx = b.x * cy + b.z * sy;
		const bz0 = b.z * cy - b.x * sy;
		const by = b.y * ct - bz0 * st;
		const bz1 = b.y * st + bz0 * ct;
		const b2 = vec3(bx, by, bz1);

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
