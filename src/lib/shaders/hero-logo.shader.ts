import { shader, vec2, vec3, vec4, cos, sin, length, smoothstep, texture } from "brometal";

/**
 * The Roostr mark at the DAG's tip: a textured billboard that shares the
 * scene's parallax but never spins away from you.
 */
export const HeroLogo = shader({
	attributes: { aCorner: "vec2" },
	uniforms: { uViewProj: "mat4", uTex: "sampler2D", uMouse: "vec2" },
	varyings: { vUv: "vec2" },

	vertex({ aCorner }, { uViewProj, uMouse }, v) {
		v.vUv = aCorner.scale(0.5).add(vec2(0.5, 0.5));
		const yaw = uMouse.x * 0.4;
		const cy = cos(yaw);
		const sy = sin(yaw);
		const tilt = uMouse.y * 0.22;
		const ct = cos(tilt);
		const st = sin(tilt);
		const px = aCorner.x * 0.26 * cy;
		const pz0 = -(aCorner.x * 0.26) * sy;
		const py = aCorner.y * 0.26 * ct - pz0 * st;
		const pz1 = aCorner.y * 0.26 * st + pz0 * ct;
		return uViewProj.mul(vec4(vec3(px, py, pz1), 1));
	},

	fragment({ uTex }, { vUv }) {
		// Circular badge: the square mark floats as a coin, not a card.
		const d = length(vUv.sub(vec2(0.5, 0.5))) * 2.0;
		const mask = 1.0 - smoothstep(0.86, 0.96, d);
		const c = texture(uTex, vUv);
		return vec4(c.x, c.y, c.z, c.w * mask * 0.96);
	},
});
