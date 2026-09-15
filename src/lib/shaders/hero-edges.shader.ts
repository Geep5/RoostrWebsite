import { shader, vec2, vec3, vec4, cos, sin, max, smoothstep, length } from "brometal";

/**
 * The connectedness: thin quads welding nodes inside each time slice,
 * and near-vertical tracks following the same node between slices - the
 * threads that show structure persisting through time. Same swirl as the
 * nodes so everything stays welded.
 */
export const HeroEdges = shader({
	attributes: { aQuad: "vec2" },
	instanceAttributes: { iStart: "vec3", iEnd: "vec3", iTint: "vec3", iBirth: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2", uWidth: "float", uNow: "float" },
	varyings: { vTint: "vec3", vAlong: "float", vBirth: "float" },

	vertex({ aQuad, iStart, iEnd, iTint, iBirth }, { uViewProj, uTime, uMouse, uWidth, uNow }, v) {
		v.vTint = iTint;
		v.vAlong = aQuad.y;
		v.vBirth = iBirth;

		const yaw = uTime * 0.22 + uMouse.x * 0.5;
		const tilt = 0.42 + uMouse.y * 0.18;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const ct = cos(tilt);
		const st = sin(tilt);

		const ax = iStart.x * cy + iStart.z * sy;
		const az0 = iStart.z * cy - iStart.x * sy;
		const ay = iStart.y * ct - az0 * st;
		const az1 = iStart.y * st + az0 * ct;
		const a2 = vec3(ax, ay, az1);

		const bx = iEnd.x * cy + iEnd.z * sy;
		const bz0 = iEnd.z * cy - iEnd.x * sy;
		const by = iEnd.y * ct - bz0 * st;
		const bz1 = iEnd.y * st + bz0 * ct;
		const b2 = vec3(bx, by, bz1);

		const dir = b2.sub(a2);
		const len = length(dir);
		const n = dir.scale(1.0 / max(len, 0.0001));
		const perp = vec3(-n.y, n.x, 0);

		const p = a2.add(n.scale(aQuad.y * len)).add(perp.scale(aQuad.x * uWidth * 0.5));
		return uViewProj.mul(vec4(p, 1));
	},

	fragment({ uNow }, { vTint, vAlong, vBirth }) {
		const taper = smoothstep(0.0, 0.08, vAlong) * (1.0 - smoothstep(0.92, 1.0, vAlong));
		const grown = smoothstep(vBirth, vBirth + 1.6, uNow) * (1.0 - smoothstep(23.2, 25.9, uNow));
		return vec4(vTint, taper * 0.8 * grown);
	},
});
