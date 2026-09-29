/**
 * Chat drafts that outlive the composer: switching the pane to Properties
 * (or to another object) unmounts the chat, and a half-typed message must be
 * there when you come back. Kept per object + conversation thread, in memory
 * and in sessionStorage so a reload of the same tab keeps it too.
 */

const PREFIX = "roostr-draft:";
const memory = new Map<string, string>();

const keyOf = (objectId: string, threadId: string) => `${objectId}|${threadId}`;

export function loadDraft(objectId: string, threadId: string): string {
	const key = keyOf(objectId, threadId);
	const held = memory.get(key);
	if (held !== undefined) return held;
	try {
		return sessionStorage.getItem(PREFIX + key) ?? "";
	} catch {
		return "";
	}
}

export function saveDraft(objectId: string, threadId: string, text: string): void {
	const key = keyOf(objectId, threadId);
	if (text) memory.set(key, text);
	else memory.delete(key);
	try {
		if (text) sessionStorage.setItem(PREFIX + key, text);
		else sessionStorage.removeItem(PREFIX + key);
	} catch {
		/* storage full or blocked: the in-memory copy still covers pane switches */
	}
}
