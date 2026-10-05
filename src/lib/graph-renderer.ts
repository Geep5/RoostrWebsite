/**
 * The object graph's drawing, on three.js (WebGL): every node is one
 * instance of an anti-aliased disc and every edge one instance of a ribbon
 * of constant pixel width - two draw calls for the whole graph, whatever its
 * size. The page owns the layout, the view (scale + offset) and the HTML
 * labels; this only turns the arrays it fills into pixels.
 *
 * Screen mapping (both shaders): screen = (world - offset) * scale +
 * viewport / 2, y down, in CSS pixels; clip space is computed directly, so
 * no camera transform is involved.
 */
import {
	Color,
	DoubleSide,
	DynamicDrawUsage,
	Float32BufferAttribute,
	InstancedBufferAttribute,
	InstancedBufferGeometry,
	LinearSRGBColorSpace,
	Mesh,
	OrthographicCamera,
	Scene,
	ShaderMaterial,
	WebGLRenderer,
} from "three";

export interface GraphBuffers {
	/** Per node: x,y (world). */
	centers: Float32Array;
	/** Per node: radius (world units). */
	radii: Float32Array;
	/** Per node: r,g,b (0..1). */
	tints: Float32Array;
	/** Per node: 0 normal, 0.5 has an agent, 1 hovered/focused (brighter + ring). */
	flags: Float32Array;
	/** Per edge: x,y of its start / end (world). */
	starts: Float32Array;
	ends: Float32Array;
	/** Per edge: r,g,b,a. */
	colors: Float32Array;
}

export interface GraphView {
	scale: number;
	offsetX: number;
	offsetY: number;
	/** Canvas size in CSS pixels. */
	width: number;
	height: number;
}

export interface GraphRenderer {
	/** Draw one frame from the buffers' current contents. */
	render(view: GraphView): void;
	/** Run `frame` every animation frame; returns the stop. */
	loop(frame: () => void): () => void;
	destroy(): void;
}

const VIEW_UNIFORMS = `
uniform float uScale;
uniform vec2 uOffset;
uniform vec2 uViewport;
vec4 toClip(vec2 screen) {
	return vec4(screen.x / uViewport.x * 2.0 - 1.0, 1.0 - screen.y / uViewport.y * 2.0, 0.0, 1.0);
}
vec2 toScreen(vec2 world) {
	return (world - uOffset) * uScale + uViewport * 0.5;
}`;

const NODE_VERTEX = `${VIEW_UNIFORMS}
attribute vec2 iCenter;
attribute float iRadius;
attribute vec3 iTint;
attribute float iFlags;
varying vec2 vUv;
varying vec3 vTint;
varying float vFlags;
void main() {
	vUv = position.xy;
	vTint = iTint;
	vFlags = iFlags;
	// Zoomed far out a node stays a visible dot instead of vanishing under its caption.
	float rpx = max(iRadius * uScale, 3.0);
	gl_Position = toClip(toScreen(iCenter) + position.xy * rpx * 1.25);
}`;

// Flat disc (Anytype's graph look): anti-aliased fill, a ring when hovered. Premultiplied output.
const NODE_FRAGMENT = `
varying vec2 vUv;
varying vec3 vTint;
varying float vFlags;
void main() {
	float d = length(vUv * 1.25);
	float fill = 1.0 - smoothstep(0.9, 1.0, d);
	float ring = smoothstep(1.04, 1.1, d) * (1.0 - smoothstep(1.16, 1.24, d));
	vec3 color = vTint * (1.0 + vFlags * 0.25) + vec3(1.0) * (ring * vFlags * 0.9);
	float alpha = max(fill, ring * vFlags);
	gl_FragColor = vec4(color * alpha, alpha);
}`;

const EDGE_VERTEX = `${VIEW_UNIFORMS}
uniform float uWidth;
attribute vec2 iStart;
attribute vec2 iEnd;
attribute vec4 iColor;
varying vec4 vColor;
void main() {
	vColor = iColor;
	vec2 a = toScreen(iStart);
	vec2 b = toScreen(iEnd);
	vec2 along = b - a;
	vec2 dir = along / max(length(along), 1e-4);
	vec2 n = vec2(-dir.y, dir.x);
	gl_Position = toClip(a + along * position.y + n * (position.x * uWidth * 0.5));
}`;

const EDGE_FRAGMENT = `
varying vec4 vColor;
void main() {
	gl_FragColor = vec4(vColor.rgb * vColor.a, vColor.a);
}`;

/** One instanced mesh: a unit shape (xy per vertex) drawn once per instance. */
function instanced(shape: number[], instances: Record<string, [Float32Array, number]>, vertexShader: string, fragmentShader: string, uniforms: Record<string, { value: unknown }>) {
	const geometry = new InstancedBufferGeometry();
	const xyz: number[] = [];
	for (let i = 0; i < shape.length; i += 2) xyz.push(shape[i], shape[i + 1], 0);
	geometry.setAttribute("position", new Float32BufferAttribute(xyz, 3));
	const attributes: InstancedBufferAttribute[] = [];
	let count = 0;
	for (const [name, [array, size]] of Object.entries(instances)) {
		const attr = new InstancedBufferAttribute(array, size);
		attr.setUsage(DynamicDrawUsage);
		geometry.setAttribute(name, attr);
		attributes.push(attr);
		count = array.length / size;
	}
	geometry.instanceCount = count;
	const material = new ShaderMaterial({
		vertexShader,
		fragmentShader,
		uniforms,
		transparent: true,
		premultipliedAlpha: true,
		// The shaders flip y (screen y points down), which reverses every
		// triangle's winding: one-sided, three culled all nodes and half the edges.
		side: DoubleSide,
		depthTest: false,
		depthWrite: false,
	});
	const mesh = new Mesh(geometry, material);
	// Positions come from the shader, not the mesh: three's bounds would cull it.
	mesh.frustumCulled = false;
	mesh.visible = count > 0;
	return { mesh, attributes, geometry, material };
}

export function createGraphRenderer(canvas: HTMLCanvasElement, buffers: GraphBuffers, clear: [number, number, number]): GraphRenderer {
	const renderer = new WebGLRenderer({ canvas, antialias: true, premultipliedAlpha: true });
	// Colors are written as given (the shaders' math), with no color-space conversion.
	renderer.outputColorSpace = LinearSRGBColorSpace;
	renderer.setClearColor(new Color().setRGB(clear[0], clear[1], clear[2], LinearSRGBColorSpace), 1);
	renderer.setPixelRatio(window.devicePixelRatio || 1);

	const view = { uScale: { value: 1 }, uOffset: { value: [0, 0] }, uViewport: { value: [1, 1] } };
	const edges = instanced(
		[-1, 0, 1, 0, -1, 1, -1, 1, 1, 0, 1, 1],
		{ iStart: [buffers.starts, 2], iEnd: [buffers.ends, 2], iColor: [buffers.colors, 4] },
		EDGE_VERTEX,
		EDGE_FRAGMENT,
		{ ...view, uWidth: { value: 1.5 } },
	);
	const nodes = instanced(
		[-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1],
		{ iCenter: [buffers.centers, 2], iRadius: [buffers.radii, 1], iTint: [buffers.tints, 3], iFlags: [buffers.flags, 1] },
		NODE_VERTEX,
		NODE_FRAGMENT,
		view,
	);
	// Edges under nodes.
	edges.mesh.renderOrder = 0;
	nodes.mesh.renderOrder = 1;
	const scene = new Scene();
	scene.add(edges.mesh, nodes.mesh);
	const camera = new OrthographicCamera();

	let width = 0;
	let height = 0;
	return {
		render(v) {
			if (v.width !== width || v.height !== height) {
				width = v.width;
				height = v.height;
				renderer.setSize(width, height, false);
			}
			view.uScale.value = v.scale;
			view.uOffset.value = [v.offsetX, v.offsetY];
			view.uViewport.value = [v.width, v.height];
			for (const attr of [...edges.attributes, ...nodes.attributes]) attr.needsUpdate = true;
			renderer.render(scene, camera);
		},
		loop(frame) {
			renderer.setAnimationLoop(frame);
			return () => renderer.setAnimationLoop(null);
		},
		destroy() {
			renderer.setAnimationLoop(null);
			for (const part of [edges, nodes]) {
				part.geometry.dispose();
				part.material.dispose();
			}
			renderer.dispose();
		},
	};
}
