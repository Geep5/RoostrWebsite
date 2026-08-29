/**
 * Regenerate src/lib/engine/proto-text.ts from src/lib/engine-proto.proto.
 * Run after any schema change: bun run scripts/gen-proto-text.ts
 */
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const protoPath = resolve(root, "src/lib/engine-proto.proto");
const outPath = resolve(root, "src/lib/engine/proto-text.ts");

const text = readFileSync(protoPath, "utf8");
const out = `/**
 * GENERATED FILE — do not edit. Source: src/lib/engine-proto.proto.
 * Regenerate with: bun run scripts/gen-proto-text.ts
 *
 * The schema ships as a TS string constant so the engine loads it
 * identically under Vite (browser) and bun (tests) without ?raw imports.
 */
export const PROTO_TEXT: string = ${JSON.stringify(text)};
`;
writeFileSync(outPath, out);
console.log(`wrote ${outPath} (${text.length} chars of schema)`);
