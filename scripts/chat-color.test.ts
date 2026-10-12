import { describe, expect, test } from "bun:test";
import { DEFAULT_CHAT_COLOR, INK, WHITE, bubbleVars, inkFor, normalizeHex } from "../src/lib/chat-color";

describe("chat color", () => {
	test("hex validation normalizes #RGB/#RRGGBB and refuses the rest", () => {
		expect(normalizeHex("#edcf7f")).toBe("#EDCF7F");
		expect(normalizeHex(" 0af ")).toBe("#00AAFF");
		expect(normalizeHex("#123456")).toBe("#123456");
		for (const bad of ["", "#", "#12", "#1234", "#12345g", "red", "rgb(0,0,0)", "#1234567"]) expect(normalizeHex(bad)).toBeNull();
	});

	test("ink on light bubbles, white on dark ones", () => {
		expect(inkFor(DEFAULT_CHAT_COLOR)).toBe(INK);
		expect(inkFor("#FFFFFF")).toBe(INK);
		expect(inkFor("#FFEB3B")).toBe(INK);
		expect(inkFor("#000000")).toBe(WHITE);
		expect(inkFor("#1E3A8A")).toBe(WHITE);
		expect(inkFor("#7C3AED")).toBe(WHITE);
	});

	test("bubble vars carry the chosen ink and a matching tint", () => {
		expect(bubbleVars("#EDCF7F")).toBe("--bubble-bg: #EDCF7F; --bubble-ink: #1B1B1B; --bubble-tint: rgba(0, 0, 0, 0.12);");
		expect(bubbleVars("#000000")).toContain("--bubble-ink: #FFFFFF;");
	});
});
