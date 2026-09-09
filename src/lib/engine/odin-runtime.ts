/** Headless subset of Odin 2026-07a core/sys/wasm/js/odin.js.
 * js_wasm32 strings/slices lower to (pointer, length), integers to i32.
 * No DOM, eval, filesystem or network capability is supplied to the core.
 */
export interface OdinRuntime {
	imports: WebAssembly.Imports;
	setMemory(memory: WebAssembly.Memory): void;
	clearDiagnostic(): void;
}

export function odinRuntime(): OdinRuntime {
	let memory: WebAssembly.Memory | undefined;
	let diagnostic = "";
	const decoder = new TextDecoder();
	const bytes = (pointer: number, length: number) => {
		if (!memory) throw new Error("Odin memory is not initialized");
		return new Uint8Array(memory.buffer, pointer >>> 0, length >>> 0);
	};
	const trap = (): never => {
		throw new Error(diagnostic.trim() || "Odin core trapped");
	};
	const imports: WebAssembly.Imports = {
		odin_env: {
			write: (_fd: number, pointer: number, length: number) => {
				diagnostic = (diagnostic + decoder.decode(bytes(pointer, length))).slice(-8192);
			},
			trap,
			abort: trap,
			time_now: () => BigInt(Date.now()),
			tick_now: () => performance.now(),
			rand_bytes: (pointer: number, length: number) => {
				const output = bytes(pointer, length);
				for (let offset = 0; offset < output.length; offset += 65536) {
					crypto.getRandomValues(output.subarray(offset, offset + 65536));
				}
			},
			sqrt: Math.sqrt,
			sin: Math.sin,
			cos: Math.cos,
			pow: Math.pow,
			fmuladd: (x: number, y: number, z: number) => x * y + z,
			ln: Math.log,
			exp: Math.exp,
			ldexp: (x: number, exponent: number) => x * 2 ** exponent,
		},
	};
	return {
		imports,
		setMemory(value: WebAssembly.Memory) { memory = value; },
		clearDiagnostic() { diagnostic = ""; },
	};
}
