/**
 * Minimal, safe markdown renderer for chat messages (Discussion).
 * Escapes all input, then applies a chat-appropriate subset:
 * fenced code blocks, inline code, bold, italic, strikethrough,
 * links (+ bare URLs), lists, blockquotes, headings, tables, paragraphs.
 * No dependency, no raw HTML passthrough.
 */

/** A `| a | b |` line (a table row or header). */
const isRow = (line: string): boolean => /^\s*\|.*\|\s*$/.test(line);
/** The `|---|:---:|` line under a table's header. */
const isSeparator = (line: string): boolean => isRow(line) && cells(line).every((c) => /^:?-+:?$/.test(c));
/** A row's cells, outer pipes dropped; `\|` stays a literal pipe. */
function cells(line: string): string[] {
	return line
		.trim()
		.replace(/^\|/, "")
		.replace(/\|$/, "")
		.split(/(?<!\\)\|/)
		.map((c) => c.trim().replace(/\\\|/g, "|"));
}
/** A column of amounts and counts reads right-aligned, like a spreadsheet. */
const numeric = (c: string): boolean => /^[-+]?[$€£¥]?\s?[\d,.]+%?[kKmM]?$/.test(c.trim());
function table(head: string[], aligns: string[], rows: string[][]): string {
	const align = head.map((_, i) => aligns[i] || (rows.length && rows.every((r) => !r[i] || numeric(r[i])) ? "right" : ""));
	// Number columns keep their figures on one line; text columns wrap, so a
	// table fits a narrow chat pane instead of pushing its numbers off-screen.
	const td = (tag: "th" | "td", text: string, i: number) =>
		`<${tag}${align[i] === "right" ? ' class="num"' : ""}${align[i] ? ` style="text-align:${align[i]}"` : ""}>${inline(text ?? "")}</${tag}>`;
	return `<div class="md-table"><table><thead><tr>${head.map((h, i) => td("th", h, i)).join("")}</tr></thead><tbody>${rows
		.map((r) => `<tr>${head.map((_, i) => td("td", r[i] ?? "", i)).join("")}</tr>`)
		.join("")}</tbody></table></div>`;
}

const escapeHtml = (s: string): string =>
	s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function safeHref(url: string): string {
	const u = url.trim();
	if (/^(https?:|mailto:)/i.test(u)) return u;
	if (/^[\w.-]+\.[a-z]{2,}(\/\S*)?$/i.test(u)) return `https://${u}`; // bare domain
	return "";
}

function inline(s: string): string {
	let out = escapeHtml(s);
	// inline code first (contents protected from other transforms)
	const codes: string[] = [];
	out = out.replace(/`([^`\n]+)`/g, (_, c) => {
		codes.push(`<code class="ic">${c}</code>`);
		return `\x00C${codes.length - 1}\x00`;
	});
	out = out
		.replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>")
		.replace(/(?<!\w)\*([^*\n]+)\*(?!\w)/g, "<i>$1</i>")
		.replace(/(?<!\w)_([^_\n]+)_(?!\w)/g, "<i>$1</i>")
		.replace(/~~([^~]+)~~/g, "<s>$1</s>")
		// [text](url); an in-app object link (`!` tag in chat) opens here, not in a new tab
		.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_, t, u) => {
			if (/^\/app\/object\/[\w-]+$/.test(u)) return `<a class="obj-link" href="${u}">${t}</a>`;
			const href = safeHref(u.replace(/&amp;/g, "&"));
			return href ? `<a href="${escapeHtml(href)}" target="_blank" rel="noopener">${t}</a>` : t;
		})
		// bare URLs
		.replace(/(^|[\s(])((?:https?:\/\/)[^\s<>()]+[^\s<>().,;:!?'"])/g, (_, pre, u) => `${pre}<a href="${u}" target="_blank" rel="noopener">${u}</a>`);
	return out.replace(/\x00C(\d+)\x00/g, (_, i) => codes[Number(i)] ?? "");
}

export function renderMarkdown(src: string): string {
	// 1) extract fenced code blocks
	const blocks: string[] = [];
	const text = src.replace(/```(\w*)\n?([\s\S]*?)(?:```|$)/g, (_, lang, code) => {
		blocks.push(
			`<pre class="cb">${lang ? `<span class="cb-lang">${escapeHtml(lang)}</span>` : ""}<code>${escapeHtml(code.replace(/\n$/, ""))}</code></pre>`,
		);
		return `\n\x00B${blocks.length - 1}\x00\n`;
	});

	// 2) block structure: headings, quotes, lists, paragraphs
	const lines = text.split("\n");
	const out: string[] = [];
	let para: string[] = [];
	let list: { tag: "ul" | "ol"; items: Array<{ text: string; check: "" | "todo" | "done" }> } | null = null;
	let quote: string[] = [];
	const flushPara = () => {
		if (para.length) {
			out.push(`<p>${para.map(inline).join("<br>")}</p>`);
			para = [];
		}
	};
	const flushList = () => {
		if (list) {
			const tasky = list.items.some((i) => i.check);
			const cls = tasky ? ' class="md-tasks"' : "";
			const li = (i: { text: string; check: string }) =>
				i.check
					? `<li class="md-task${i.check === "done" ? " md-done" : ""}"><span class="md-cb">${i.check === "done" ? "\u2713" : ""}</span><span>${inline(i.text)}</span></li>`
					: `<li>${inline(i.text)}</li>`;
			out.push(`<${list.tag}${cls}>${list.items.map(li).join("")}</${list.tag}>`);
			list = null;
		}
	};
	const flushQuote = () => {
		if (quote.length) {
			out.push(`<blockquote>${quote.map(inline).join("<br>")}</blockquote>`);
			quote = [];
		}
	};
	const flushAll = () => {
		flushPara();
		flushList();
		flushQuote();
	};

	for (let li = 0; li < lines.length; li++) {
		const line = lines[li];
		const blockMatch = line.match(/^\x00B(\d+)\x00$/);
		if (blockMatch) {
			flushAll();
			out.push(blocks[Number(blockMatch[1])] ?? "");
			continue;
		}
		// GFM table: a | row, then a |---| separator, then rows until one isn't a | row.
		if (isRow(line) && li + 1 < lines.length && isSeparator(lines[li + 1])) {
			flushAll();
			const head = cells(line);
			const aligns = cells(lines[li + 1]).map((c) => (/^:-+:$/.test(c) ? "center" : /^-+:$/.test(c) ? "right" : /^:-+$/.test(c) ? "left" : ""));
			const rows: string[][] = [];
			li += 2;
			while (li < lines.length && isRow(lines[li])) rows.push(cells(lines[li++]));
			li -= 1;
			out.push(table(head, aligns, rows));
			continue;
		}
		const h = line.match(/^(#{1,3})\s+(.*)$/);
		if (h) {
			flushAll();
			out.push(`<div class="md-h md-h${h[1].length}">${inline(h[2])}</div>`);
			continue;
		}
		if (/^\s*[-*_]{3,}\s*$/.test(line)) {
			flushAll();
			out.push("<hr>");
			continue;
		}
		const ul = line.match(/^\s*[-*]\s+(.*)$/);
		const ol = line.match(/^\s*\d+[.)]\s+(.*)$/);
		if (ul || ol) {
			flushPara();
			flushQuote();
			const tag = ul ? "ul" : "ol";
			if (!list || list.tag !== tag) flushList(), (list = { tag, items: [] });
			let item = (ul ?? ol)![1];
			let check: "" | "todo" | "done" = "";
			const task = ul && item.match(/^\[( |x|X)\]\s+(.*)$/);
			if (task) {
				check = task[1] === " " ? "todo" : "done";
				item = task[2];
			}
			list!.items.push({ text: item, check });
			continue;
		}
		const q = line.match(/^>\s?(.*)$/);
		if (q) {
			flushPara();
			flushList();
			quote.push(q[1]);
			continue;
		}
		if (line.trim() === "") {
			flushAll();
			continue;
		}
		flushList();
		flushQuote();
		para.push(line);
	}
	flushAll();
	return out.join("");
}
