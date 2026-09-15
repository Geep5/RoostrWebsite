import { shader, vec2, vec3, vec4, sin, cos, fract, clamp, smoothstep, length, mix } from "brometal";

/**
 * The glon change-DAG as a starfield: generations of change objects
 * linked parent to child, drifting as one structure. Warm nodes are
 * human-authored changes, cool nodes agent-authored, bright nodes are
 * merges where two histories join.
 * iKind: 0 human, 1 agent, 2 merge.
 */
export const HeroDag = shader({
	attributes: { aCorner: "vec2" },
	instanceAttributes: { iPos: "vec3", iKind: "float", iSeed: "float" },
	uniforms: { uViewProj: "mat4", uTime: "float", uMouse: "vec2" },
	varyings: { vUv: "vec2", vColor: "vec3", vAlpha: "float" },

	vertex({ aCorner, iPos, iKind, iSeed }, { uViewProj, uTime, uMouse }, v) {
		v.vUv = aCorner;

		let color = vec3(1.0, 0.66, 0.3); // human: ember
		let size = 0.028 + fract(iSeed * 7.3) * 0.012;
		let glow = 0.65 + fract(iSeed * 4.7) * 0.5;
		if (iKind > 0.5 && iKind < 1.5) {
			color = vec3(0.35, 0.68, 1.0); // agent: electric
		} else if (iKind > 1.5) {
			color = vec3(0.95, 0.97, 1.0); // merge: white-hot
			size = 0.03 + fract(iSeed * 5.1) * 0.008;
			glow = 1.1;
		}

		// A gentle generational pulse running back through the DAG.
		const pulse = 1.0 + 0.35 * sin(uTime * 0.9 + iPos.z * 1.7);
		let p = vec3(iPos.x, iPos.y, iPos.z);

		// Slow yaw + pointer parallax, same for every program in the scene.
		const yaw = uTime * 0.05 + uMouse.x * 0.4;
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

		const depthFade = clamp((p.z + 15.0) / 17.0, 0.16, 1.0);
		const world = p.add(vec3(aCorner.x * size, aCorner.y * size, 0));
		v.vColor = color;
		v.vAlpha = depthFade * glow * pulse;
		return uViewProj.mul(vec4(world, 1));
	},

	fragment(_uniforms, { vUv, vColor, vAlpha }) {
		const d = length(vUv);
		const core = 1 - smoothstep(0.5, 1.0, d);
		return vec4(vColor, core * vAlpha);
	},
});
