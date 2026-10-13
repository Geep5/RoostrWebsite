/**
 * In-app tabs - Anytype's tab bar mapped onto the web app.
 *
 * Browser model: the ACTIVE tab follows navigation (opening an object from
 * the sidebar replaces the active tab's content, like a browser tab), and
 * "Open in new tab" adds a background tab.
 *
 * Each space keeps its own strip: switching spaces puts that space's tabs
 * back as you left them. Strips live in this device's localStorage only -
 * open tabs are not synced, so another device keeps its own.
 */
import { goto } from "$app/navigation";
import { browser } from "$app/environment";

const KEY = "roostr.tabs.spaces";
/** The single strip kept before tabs were per space: adopted by the first space opened. */
const LEGACY_KEY = "roostr.tabs";
export const HOME_PATH = "/app";

export interface Tab {
	uid: number;
	path: string;
}

interface Saved {
	paths: string[];
	active: number;
}

let nextUid = 1;

class TabStrip {
	list = $state<Tab[]>([]);
	active = $state(0);
	/** The space whose strip is showing; "" until the app knows its space (nothing is saved before then). */
	space = "";
	private saved: Record<string, Saved> = {};
	private legacy: Saved | null = null;

	constructor() {
		if (!browser) return;
		try {
			const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Record<string, Saved> | null;
			if (raw && typeof raw === "object") this.saved = raw;
			const old = JSON.parse(localStorage.getItem(LEGACY_KEY) ?? "null") as Saved | null;
			if (old && Array.isArray(old.paths) && old.paths.length) this.legacy = old;
		} catch {
			// corrupted store - start fresh
		}
	}

	private save(): void {
		if (!browser || !this.space) return;
		this.saved[this.space] = { paths: this.list.map((t) => t.path), active: this.active };
		localStorage.setItem(KEY, JSON.stringify(this.saved));
		if (this.legacy) {
			this.legacy = null;
			localStorage.removeItem(LEGACY_KEY);
		}
	}

	/**
	 * Shows `spaceId`'s strip, as it was left on this device. Returns the
	 * active tab's path, or "" when the space has no tabs here yet.
	 */
	use(spaceId: string): string {
		if (spaceId === this.space) return this.list[this.active]?.path ?? "";
		const s = this.saved[spaceId] ?? (this.space === "" ? this.legacy : null) ?? null;
		this.space = spaceId;
		if (s && Array.isArray(s.paths) && s.paths.length) {
			this.list = s.paths.map((path) => ({ uid: nextUid++, path }));
			this.active = Math.min(Math.max(0, s.active | 0), s.paths.length - 1);
		} else {
			this.list = [];
			this.active = 0;
		}
		return this.list[this.active]?.path ?? "";
	}

	/** Drops a space's strip (the space was deleted or left). */
	forget(spaceId: string): void {
		if (!browser || !(spaceId in this.saved)) return;
		delete this.saved[spaceId];
		localStorage.setItem(KEY, JSON.stringify(this.saved));
	}

	/** Route changed: the active tab absorbs it (an empty strip gets its first tab). */
	sync(path: string): void {
		if (this.list.length === 0) {
			this.list = [{ uid: nextUid++, path }];
			this.active = 0;
			this.save();
			return;
		}
		const cur = this.list[this.active];
		if (cur.path !== path) {
			cur.path = path;
			this.save();
		}
	}

	/** Add a tab; background by default (browser convention for context-menu opens). */
	open(path: string, background = true): void {
		this.list.push({ uid: nextUid++, path });
		if (!background) {
			this.active = this.list.length - 1;
			void goto(path);
		}
		this.save();
	}

	activate(i: number): void {
		if (i < 0 || i >= this.list.length || i === this.active) return;
		this.active = i;
		this.save();
		void goto(this.list[i].path);
	}

	/**
	 * Reorder by drag. `active` is an index, so it is re-derived from the
	 * moved-to position of the tab that was active - the strip rearranges
	 * without switching what you are looking at.
	 */
	move(from: number, to: number): void {
		if (from < 0 || from >= this.list.length) return;
		const activeUid = this.list[this.active]?.uid;
		const [moved] = this.list.splice(from, 1);
		this.list.splice(Math.max(0, Math.min(to, this.list.length)), 0, moved);
		const at = this.list.findIndex((t) => t.uid === activeUid);
		if (at >= 0) this.active = at;
		this.save();
	}

	close(i: number): void {
		if (this.list.length === 0) return;
		const wasActive = i === this.active;
		this.list.splice(i, 1);
		if (this.list.length === 0) {
			this.list = [{ uid: nextUid++, path: HOME_PATH }];
			this.active = 0;
			this.save();
			void goto(HOME_PATH);
			return;
		}
		if (i < this.active) this.active--;
		else if (wasActive) {
			this.active = Math.min(this.active, this.list.length - 1);
			void goto(this.list[this.active].path);
		}
		this.save();
	}
}

export const tabs = new TabStrip();
