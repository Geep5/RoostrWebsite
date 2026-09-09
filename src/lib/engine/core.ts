import { odinRuntime, type OdinRuntime } from "./odin-runtime";

const ABI_VERSION = 1;
const REQUEST_LIMIT = 16 * 1024 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

type CoreExports = WebAssembly.Exports & {
	memory: WebAssembly.Memory;
	_start(): void;
	core_abi_version(): number;
	core_reserve(length: number): number;
	core_execute(): number;
	core_response_pointer(): number;
	core_response_length(): number;
	core_reset(all: number): void;
};

export interface CoreInitOptions {
	/** Explicit bytes for native Bun parity runners; browsers normally fetch. */
	wasmBytes?: BufferSource;
	wasmUrl?: string | URL;
}

export class CoreError extends Error {
	constructor(public readonly code: "not_initialized" | "reentrant" | "request_limit" | "domain" | "runtime", message: string) {
		super(message);
		this.name = "CoreError";
	}
}

let exports: CoreExports | undefined;
let runtime: OdinRuntime | undefined;
let initialization: Promise<void> | undefined;
let active = false;
let generation = 0;

/** Must resolve before any codec, replay, query or mutation entrypoint is used. */
export function initCore(options: CoreInitOptions = {}): Promise<void> {
	if (initialization) return initialization;
	initialization = (async () => {
		const host = odinRuntime();
		let bytes = options.wasmBytes;
		if (!bytes) {
			const response = await fetch(options.wasmUrl ?? "/engine.wasm");
			if (!response.ok) throw new Error(`Unable to load shared core: HTTP ${response.status}`);
			bytes = await response.arrayBuffer();
		}
		const instance = (await WebAssembly.instantiate(bytes, host.imports)).instance;
		const api = instance.exports as CoreExports;
		if (!(api.memory instanceof WebAssembly.Memory)) throw new Error("Shared core does not export memory");
		for (const name of ["_start", "core_abi_version", "core_reserve", "core_execute", "core_response_pointer", "core_response_length", "core_reset"]) {
			if (typeof api[name] !== "function") throw new Error(`Shared core is missing ${name}`);
		}
		host.setMemory(api.memory);
		api._start();
		if (api.core_abi_version() !== ABI_VERSION) throw new Error("Shared core ABI version mismatch");
		exports = api;
		runtime = host;
		generation++;
	})().catch((error: unknown) => {
		initialization = undefined;
		throw error;
	});
	return initialization;
}

/** Query adapters invalidate their signatures when this value changes. */
export function coreGeneration(): number { return generation; }

/** Release the persistent query cache without unloading the initialized module. */
export function resetCore(): void {
	if (active) throw new CoreError("reentrant", "Cannot reset the shared core during a request");
	exports?.core_reset(1);
	generation++;
}

/** The ABI is synchronous and single-flight. Results are copied and decoded
 * before resetting the request arena. No WASM-backed view escapes this call.
 */
export function coreCall<T>(method: string, payload: unknown): T {
	const api = exports;
	if (!api) throw new CoreError("not_initialized", "Call and await initCore() before using the shared engine");
	if (active) throw new CoreError("reentrant", "Shared core calls are not reentrant");
	active = true;
	let trapped = false;
	try {
		const input = encoder.encode(JSON.stringify({ method, payload }));
		if (input.byteLength > REQUEST_LIMIT) throw new CoreError("request_limit", "Core request exceeds 16 MiB");
		runtime?.clearDiagnostic();
		const pointer = api.core_reserve(input.byteLength) >>> 0;
		if (!pointer) throw new CoreError("runtime", "Shared core refused the request reservation");
		new Uint8Array(api.memory.buffer, pointer, input.byteLength).set(input);
		if (api.core_execute() !== 0) throw new CoreError("runtime", "Shared core rejected the request state");
		const outputPointer = api.core_response_pointer() >>> 0;
		const outputLength = api.core_response_length() >>> 0;
		if (!outputLength || outputPointer + outputLength > api.memory.buffer.byteLength) {
			throw new CoreError("runtime", "Invalid shared core response bounds");
		}
		const result = JSON.parse(decoder.decode(new Uint8Array(api.memory.buffer, outputPointer, outputLength))) as { result?: T; error?: string };
		if (typeof result.error === "string") throw new CoreError("domain", result.error);
		if (!("result" in result)) throw new CoreError("runtime", "Shared core response is missing its result");
		return result.result as T;
	} catch (error) {
		if (error instanceof CoreError) {
			trapped = error.code === "runtime";
			throw error;
		}
		trapped = true;
		throw new CoreError("runtime", error instanceof Error ? error.message : String(error));
	} finally {
		try {
			api.core_reset(trapped ? 1 : 0);
			if (trapped) generation++;
		} finally {
			active = false;
		}
	}
}
