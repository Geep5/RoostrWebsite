import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
try {
	const manifest = JSON.parse(await readFile(resolve(root, "static/engine-core.json"), "utf8"));
	if (manifest.abiVersion !== 1 || manifest.target !== "js_wasm32" || manifest.artifact !== "engine.wasm") {
		throw new Error("Shared core manifest ABI/target mismatch");
	}
	const bytes = await readFile(resolve(root, "static/engine.wasm"));
	if (hash(bytes) !== manifest.sha256) throw new Error("Shared core WASM fingerprint mismatch");
	if (hash(JSON.stringify(manifest.sourceFiles)) !== manifest.sourceFingerprint) throw new Error("Invalid shared core source fingerprint");
	for (const path of ["src/lib/engine/core.ts", "src/lib/engine/odin-runtime.ts"]) {
		if (hash(await readFile(resolve(root, path))) !== manifest.runtimeFiles?.[path]) {
			throw new Error(`Shared core runtime changed: ${path}`);
		}
	}
	console.log(`Verified shared Odin core ${manifest.sourceFingerprint}`);
} catch (error) {
	console.error(error instanceof Error ? error.message : String(error));
	console.error("Regenerate and include static/engine.wasm + static/engine-core.json using bun run build:core before building/deploying.");
	process.exitCode = 1;
}
