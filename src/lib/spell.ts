/**
 * Basic English spellcheck over the editor's text blocks.
 *
 * Dictionary: the BSD `web2` word list (235k headwords) served as a static
 * asset and loaded lazily into a Set, plus light suffix stripping so
 * ordinary inflections (notes, edited, running, quickly) of known
 * headwords pass. A user ignore list ("Add to dictionary") persists in
 * localStorage and syncs into the same lookup.
 *
 * Rendering uses the CSS Custom Highlight API: misspelled words become
 * Ranges registered under the `spell` highlight - no DOM mutation, so the
 * marks engine and caret handling stay untouched.
 */

let dict: Set<string> | null = null;
/** Rank of ~10k common words (0 = most frequent); words outside it rank last. */
let frequency: Map<string, number> | null = null;

/**
 * web2 is a headword list - common inflections and contractions are
 * missing (it has "have" but not "has", "say" but not "says").
 */
const SUPPLEMENT = [
  "has",
  "says",
  "did",
  "done",
  "gone",
  "went",
  "been",
  "am",
  "are",
  "were",
  "cannot",
  "ok",
  "okay",
  "email",
  "emails",
  "todo",
  "todos",
  "app",
  "apps",
  "don't",
  "doesn't",
  "isn't",
  "aren't",
  "wasn't",
  "weren't",
  "can't",
  "won't",
  "wouldn't",
  "couldn't",
  "shouldn't",
  "didn't",
  "hasn't",
  "haven't",
  "hadn't",
  "it's",
  "that's",
  "there's",
  "here's",
  "what's",
  "who's",
  "let's",
  "i'm",
  "i've",
  "i'll",
  "i'd",
  "you're",
  "you've",
  "you'll",
  "you'd",
  "we're",
  "we've",
  "we'll",
  "we'd",
  "they're",
  "they've",
  "they'll",
  "they'd",
  "he's",
  "she's",
  "ain't",
  "y'all",
  "o'clock",
];
let loading: Promise<void> | null = null;

const IGNORE_KEY = "spell-ignore";

function ignoreSet(): Set<string> {
  try {
    return new Set(
      JSON.parse(localStorage.getItem(IGNORE_KEY) ?? "[]") as string[],
    );
  } catch {
    return new Set();
  }
}

export function ignoredWords(): string[] {
  return [...ignoreSet()].sort();
}

export function addToDictionary(word: string): void {
  const s = ignoreSet();
  s.add(word.toLowerCase());
  localStorage.setItem(IGNORE_KEY, JSON.stringify([...s]));
}

export function removeFromDictionary(word: string): void {
  const s = ignoreSet();
  s.delete(word.toLowerCase());
  localStorage.setItem(IGNORE_KEY, JSON.stringify([...s]));
}

export function loadDictionary(): Promise<void> {
  if (dict) return Promise.resolve();
  loading ??= Promise.all([
    fetch("/dict-en.txt").then((r) => r.text()),
    fetch("/dict-en-common.txt").then((r) => r.text()),
  ])
    .then(([headwords, common]) => {
      const s = new Set<string>();
      for (const line of headwords.split("\n")) {
        const w = line.trim();
        if (w) s.add(w.toLowerCase());
      }
      for (const w of SUPPLEMENT) s.add(w);
      // Common words are also accepted as known: web2 lacks many inflections the frequency list has.
      const f = new Map<string, number>();
      common.split("\n").forEach((line, i) => {
        const w = line.trim().toLowerCase();
        if (!w) return;
        f.set(w, i);
        s.add(w);
      });
      dict = s;
      frequency = f;
    })
    .catch(() => {
      dict = new Set(); // asset missing: never flag anything
      frequency = new Map();
    });
  return loading;
}

function inDict(w: string): boolean {
  return dict?.has(w) ?? true;
}

/** Known word? Checks the base plus common inflection strips. */
export function isKnown(word: string): boolean {
  if (!dict) return true; // not loaded yet - stay quiet
  const w = word.toLowerCase();
  if (w.length <= 1) return true;
  if (ignoreSet().has(w)) return true;
  if (inDict(w)) return true;
  // possessives and contractions
  if (w.endsWith("'s") && inDict(w.slice(0, -2))) return true;
  const strips: Array<[string, string]> = [
    ["ies", "y"],
    ["es", ""],
    ["s", ""],
    ["ed", ""],
    ["ed", "e"],
    ["d", ""],
    ["ing", ""],
    ["ing", "e"],
    ["ly", ""],
    ["er", ""],
    ["er", "e"],
    ["est", ""],
    ["est", "e"],
  ];
  for (const [suf, add] of strips) {
    if (
      w.length > suf.length + 1 &&
      w.endsWith(suf) &&
      inDict(w.slice(0, -suf.length) + add)
    )
      return true;
  }
  return false;
}

export interface Misspelling {
  from: number;
  to: number;
  word: string;
}

const TOKEN = /[A-Za-z][A-Za-z']*/g;

/** Misspelled tokens in a text. Skips ALLCAPS, digits-adjacent, URLs. */
export function checkText(text: string): Misspelling[] {
  if (!dict) return [];
  const out: Misspelling[] = [];
  for (const m of text.matchAll(TOKEN)) {
    const word = m[0].replace(/'+$/, "");
    if (word.length <= 1) continue;
    if (word === word.toUpperCase() && word.length > 1) continue; // acronyms
    const at = m.index;
    // inside a URL/path/email? look back for schemes or separators
    const before = text.slice(Math.max(0, at - 8), at);
    if (/[/@.\\-]$/.test(before) || /https?:$/.test(before)) continue;
    if (!isKnown(word)) out.push({ from: at, to: at + word.length, word });
  }
  return out;
}

// ── Suggestions ──────────────────────────────────────────────────

const LETTERS = "abcdefghijklmnopqrstuvwxyz";

/** Every string one edit away (deletion, transposition, replacement, insertion). */
function edits1(w: string): Set<string> {
  const out = new Set<string>();
  for (let i = 0; i <= w.length; i++) {
    const head = w.slice(0, i);
    const tail = w.slice(i);
    if (tail) out.add(head + tail.slice(1));
    if (tail.length > 1) out.add(head + tail[1] + tail[0] + tail.slice(2));
    for (const c of LETTERS) {
      if (tail) out.add(head + c + tail.slice(1));
      out.add(head + c + tail);
    }
  }
  return out;
}

/**
 * Split into two words ("alot" → "a lot", "infront" → "in front"). Both
 * halves must be very common: the frequency list's tail is web
 * abbreviations ("ed" 1584, "wi" 2323, "ch" 2766) that produce "occur ed".
 */
const SPLIT_MAX_RANK = 1500;
function splits(w: string): string[] {
  const out: string[] = [];
  for (let i = 1; i <= w.length - 1; i++) {
    const a = w.slice(0, i);
    const b = w.slice(i);
    if (
      (frequency?.get(a) ?? Infinity) < SPLIT_MAX_RANK &&
      (frequency?.get(b) ?? Infinity) < SPLIT_MAX_RANK &&
      (a.length > 1 || a === "a") &&
      b.length > 1
    )
      out.push(`${a} ${b}`);
  }
  return out;
}

/** Restore the misspelling's casing pattern onto a lowercase suggestion. */
function matchCase(source: string, suggestion: string): string {
  if (source === source.toUpperCase() && source.length > 1)
    return suggestion.toUpperCase();
  if (source[0] === source[0].toUpperCase())
    return suggestion[0].toUpperCase() + suggestion.slice(1);
  return suggestion;
}

/**
 * Up to `limit` corrections, best first. Candidates are known words one edit
 * away (plus a two-word split); two edits away only when nothing closer
 * exists, and capped since that space is ~10⁶ strings for long words.
 * Ranking is by frequency: web2 is full of obscure headwords ("tch", "tec"),
 * so without it "teh" would never surface "the". Ties fall back to
 * preserving the first letter, then length.
 */
export function suggestions(word: string, limit = 5): string[] {
  if (!dict) return [];
  const w = word.toLowerCase();
  if (w.length <= 1) return [];
  // A split ranks behind common single words but ahead of obscure headwords ("a lot" over "alod").
  const SPLIT_PENALTY = 5000;
  const rankOf = (c: string): number => {
    const sp = c.indexOf(" ");
    if (sp < 0) return frequency?.get(c) ?? Number.MAX_SAFE_INTEGER;
    return (
      SPLIT_PENALTY +
      Math.max(
        frequency?.get(c.slice(0, sp)) ?? 0,
        frequency?.get(c.slice(sp + 1)) ?? 0,
      )
    );
  };
  const rank = (candidates: Iterable<string>): string[] =>
    [...new Set(candidates)]
      .filter((c) => c !== w && (c.includes(" ") || isKnown(c)))
      .sort(
        (a, b) =>
          rankOf(a) - rankOf(b) ||
          Number(b[0] === w[0]) - Number(a[0] === w[0]) ||
          Math.abs(a.length - w.length) - Math.abs(b.length - w.length) ||
          a.localeCompare(b),
      );
  const one = edits1(w);
  let found = rank([...one, ...splits(w)]);
  // Obscure-only hits at distance 1 should not hide a common word at distance 2 ("tommorow" → "tomorrow").
  const bestRank = found.length ? rankOf(found[0]) : Number.MAX_SAFE_INTEGER;
  if (
    (found.length === 0 || bestRank === Number.MAX_SAFE_INTEGER) &&
    w.length <= 12
  ) {
    const two = new Set<string>();
    for (const e of one) {
      for (const f of edits1(e)) if (frequency?.has(f)) two.add(f);
      if (two.size > 200) break;
    }
    found = rank([...two, ...found]);
  }
  return found.slice(0, limit).map((s) => matchCase(word, s));
}
