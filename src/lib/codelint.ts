/**
 * Linter for a Tool's code. The harness runs a Tool object's Code blocks,
 * joined in reading order, as the body of
 * `export default async function (input, roostr) { … }` and refuses code
 * that doesn't compile (glonOdin harness tool-runtime.ts compileProblem).
 * This parses the same wrapped module with the TypeScript compiler and
 * reports, per Code block:
 *  - errors: syntax and grammar errors, redeclared block-scoped names -
 *    code the harness won't run;
 *  - warnings: unused locals, unreachable code, unused labels.
 * Type checking is out of scope (no lib or SDK types are loaded), so
 * diagnostics that need them are dropped. The compiler is loaded on first
 * use only (a large chunk; only Tool pages pay for it).
 */
import type * as TS from "typescript";

export interface LintProblem {
	/** Offsets into the block's text; `to > from` whenever the block has text. */
	from: number;
	to: number;
	/** 1-based, within the block. */
	line: number;
	col: number;
	message: string;
	severity: "error" | "warning";
}

const HEADER = "export default async function (input: Record<string, unknown>, roostr: Roostr) {\n";
const FILE = "tool.ts";
/** Checker diagnostics that hold without lib types: compile errors, and the lint warnings. */
const CHECKER_SEVERITY: Record<number, LintProblem["severity"]> = {
	2451: "error", // Cannot redeclare block-scoped variable
	6133: "warning", // declared but its value is never read
	6196: "warning", // declared but never used
	6198: "warning", // all destructured elements are unused
	6199: "warning", // all variables are unused
	6205: "warning", // all type parameters are unused
	7027: "warning", // unreachable code
	7028: "warning", // unused label
};
/** The compiler, imported on first use. */
let compiler: Promise<typeof TS> | undefined;

/** Problems in each of a Tool's Code blocks (texts in reading order), index for index. */
export async function lintToolCode(blocks: string[]): Promise<LintProblem[][]> {
	const out: LintProblem[][] = blocks.map(() => []);
	if (!blocks.some((b) => b.trim())) return out;
	// A CommonJS package: the bundler hands its exports over as `default`.
	const ts = await (compiler ??= import("typescript").then((m) => ("default" in m && m.default ? m.default : m)));
	const code = blocks.join("\n");
	const source = `${HEADER}${code}\n}\n`;
	const file = ts.createSourceFile(FILE, source, ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);
	const host: TS.CompilerHost = {
		getSourceFile: (name) => (name === FILE ? file : undefined),
		getDefaultLibFileName: () => "lib.d.ts",
		writeFile: () => {},
		getCurrentDirectory: () => "/",
		getCanonicalFileName: (f) => f,
		useCaseSensitiveFileNames: () => true,
		getNewLine: () => "\n",
		fileExists: (f) => f === FILE,
		readFile: () => undefined,
	};
	const program = ts.createProgram(
		[FILE],
		{ noLib: true, noEmit: true, types: [], noUnusedLocals: true, allowUnreachableCode: false, allowUnusedLabels: false, target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext },
		host,
	);
	const syntactic = program.getSyntacticDiagnostics(file);
	// The checker's grammar errors (1xxx) are compile errors too; only ask it when the parse is clean.
	const semantic = syntactic.length ? [] : program.getSemanticDiagnostics(file);
	const found: { start: number; length: number; message: string; severity: LintProblem["severity"] }[] = [];
	for (const d of syntactic) found.push({ start: d.start ?? 0, length: d.length ?? 0, message: ts.flattenDiagnosticMessageText(d.messageText, "\n"), severity: "error" });
	for (const d of semantic) {
		const severity = d.code < 2000 ? "error" : CHECKER_SEVERITY[d.code];
		if (severity) found.push({ start: d.start ?? 0, length: d.length ?? 0, message: ts.flattenDiagnosticMessageText(d.messageText, "\n"), severity });
	}

	// Block i's text starts at starts[i] within `code`.
	const starts: number[] = [];
	let at = 0;
	for (const b of blocks) {
		starts.push(at);
		at += b.length + 1;
	}
	for (const f of found) {
		// Anything past the code (an unclosed brace meets the wrapper's end) lands at the end of the last block.
		const rel = Math.min(Math.max(f.start - HEADER.length, 0), code.length);
		let i = starts.length - 1;
		while (i > 0 && starts[i] > rel) i--;
		const text = blocks[i];
		let from = Math.min(rel - starts[i], text.length);
		let to = Math.min(from + f.length, text.length);
		if (to === from) {
			if (to < text.length) to++;
			else from = Math.max(0, from - 1);
		}
		const before = text.slice(0, from);
		const line = before.split("\n").length;
		const col = from - before.lastIndexOf("\n");
		out[i].push({ from, to, line, col, message: f.message, severity: f.severity });
	}
	for (const list of out) list.sort((a, b) => a.from - b.from);
	return out;
}
