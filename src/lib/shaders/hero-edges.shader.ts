import { shader, vec2, vec3, vec4, cos, sin, max, smoothstep, length } from "brometal";

/**
 * The links of the change-DAG: thin quads stretched from each change to
 * its parent(s), same slow yaw and parallax as the nodes.
 */
export const HeroEdges = shader({
	attributes: { aQuad: "vec2" },
	instanceAttributes: { iStart: "vec3", iEnd: "vec3", iTint: "vec3" },
	uniforms: { uViewProj: "mat4", uMouse: "vec2", uWidth: "float" },
	varyings: { vTint: "vec3", vAlong: "float" },

	vertex({ aQuad, iStart, iEnd, iTint }, { uViewProj, uMouse, uWidth }, v) {
		v.vTint = iTint;
		v.vAlong = aQuad.y;

		const yaw = uMouse.x * 0.4;
		const tilt = uMouse.y * 0.22;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const ct = cos(tilt);
		const st = sin(tilt);

		const ax = iStart.x * cy + iStart.z * sy;
		const az0 = iStart.z * cy - iStart.x * sy;
		const ay = iStart.y * ct - az0 * st;
		const az1 = iStart.y * st + az0 * ct;
		const a = vec3(ax, ay, az1);

		const bx = iEnd.x * cy + iEnd.z * sy;
		const bz0 = iEnd.z * cy - iEnd.x * sy;
		const by = iEnd.y * ct - bz0 * st;
		const bz1 = iEnd.y * st + bz0 * ct;
		const b = vec3(bx, by, bz1);

		const dir = b.sub(a);
		const len = length(dir);
		const n = dir.scale(1.0 / max(len, 0.0001));
		// Perpendicular in the view plane (the camera looks down -Z).
		const perp = vec3(-n.y, n.x, 0);

		const p = a.add(n.scale(aQuad.y * len)).add(perp.scale(aQuad.x * uWidth * 0.5));
		return uViewProj.mul(vec4(p, 1));
	},

	fragment(_uniforms, { vTint, vAlong }) {
		const taper = smoothstep(0.0, 0.08, vAlong) * (1.0 - smoothstep(0.92, 1.0, vAlong));
		return vec4(vTint, taper * 0.7);
	},
});
