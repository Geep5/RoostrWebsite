import { shader, vec2, vec3, vec4, cos, sin, max, smoothstep, sqrt, abs, length, storageRead } from "brometal";

/**
 * The welds. Each edge is a pair of indices into the uNodes storage
 * buffer, so when a new change pushes its connections down a generation
 * the edges follow their endpoints exactly. Same age drift and swirl as
 * the nodes.
 */
export const HeroEdges = shader({
	attributes: { aQuad: "vec2" },
	instanceAttributes: { iA: "float", iB: "float", iTint: "vec3", iBirth: "float", iDeath: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2", uWidth: "float", uNow: "float" },
	storage: { uNodes: "vec4" },
	varyings: { vTint: "vec3", vAlong: "float", vBirth: "float", vAcross: "float", vDeath: "float" },

	vertex({ aQuad, iA, iB, iTint, iBirth, iDeath }, { uViewProj, uTime, uMouse, uWidth, uNow, uNodes }, v) {
		v.vTint = iTint;
		v.vAlong = aQuad.y;
		v.vBirth = iBirth;
		v.vAcross = aQuad.x;
		v.vDeath = iDeath;

		const na = storageRead(uNodes, iA);
		const nb = storageRead(uNodes, iB);
		const gA = smoothstep(na.w, na.w + 2.8, uNow);
		const gB = smoothstep(nb.w, nb.w + 2.8, uNow);
		const start = vec3(
			na.x + cos(uTime * 1.1 + na.w * 27.3) * 0.012 * gA,
			na.y + sin(uTime * 1.5 + na.w * 13.7) * 0.022 * gA,
			na.z,
		);
		const end = vec3(
			nb.x + cos(uTime * 1.1 + nb.w * 27.3) * 0.012 * gB,
			nb.y + sin(uTime * 1.5 + nb.w * 13.7) * 0.022 * gB,
			nb.z,
		);

		const yaw = uTime * 0.22 + uMouse.x * 0.5;
		const tilt = 0.42 + uMouse.y * 0.18;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const ct = cos(tilt);
		const st = sin(tilt);

		const ax = start.x * cy + start.z * sy;
		const az0 = start.z * cy - start.x * sy;
		const ay = start.y * ct - az0 * st;
		const az1 = start.y * st + az0 * ct;
		const a2 = vec3(ax, ay, az1);

		const bx = end.x * cy + end.z * sy;
		const bz0 = end.z * cy - end.x * sy;
		const by = end.y * ct - bz0 * st;
		const bz1 = end.y * st + bz0 * ct;
		const b2 = vec3(bx, by, bz1);

		const dir = b2.sub(a2);
		const len = length(dir);
		const n = dir.scale(1.0 / max(len, 0.0001));
		const perp = vec3(-n.y, n.x, 0);

		const p = a2.add(n.scale(aQuad.y * len)).add(perp.scale(aQuad.x * uWidth * 0.5));
		return uViewProj.mul(vec4(p, 1));
	},

	fragment({ uNow }, { vTint, vAlong, vBirth, vAcross, vDeath }) {
		// No end taper: pipes run full-length into the bead centers, and the
		// beads (drawn after) cap the joints - solid connections, no ghostly
		// gap where a pipe meets its node.
		// The weld travels: the edge grows out from the new node toward its
		// parent, tip first, instead of appearing at once.
		const reach = smoothstep(vBirth + 0.3, vBirth + 1.9, uNow) * 1.15;
		const behindTip = 1.0 - smoothstep(reach - 0.14, reach, vAlong);
		const grown = smoothstep(vBirth, vBirth + 0.5, uNow);
		// A pipe, not a line: solid round tube - soft rim, fake cylindrical
		// shading so it reads as a toy pipe on the yellow page.
		const rim = 1.0 - smoothstep(0.68, 1.0, abs(vAcross));
		const round = 0.72 + 0.28 * sqrt(max(0.0, 1.0 - vAcross * vAcross));
		// A broken connection: the pipe snaps mid-span and the two halves
		// retract into their beads. Alive pipes keep iDeath at 1e30.
		const dying = smoothstep(vDeath, vDeath + 0.9, uNow);
		const gap = dying * 0.56;
		const intact = smoothstep(gap - 0.06, gap + 0.02, abs(vAlong - 0.5) + 0.001);
		return vec4(vTint.scale(round), rim * 0.92 * behindTip * grown * intact);
	},
});
