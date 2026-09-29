/**
 * One ordered lane for an editor's writes.
 *
 * Every mutation the editor issues runs after the previous one settles, so
 * a debounced text save and a structural write (split, move, remove) can
 * never land out of order - the engine sees edits in the order the person
 * made them. A failed write does not stall the lane; its caller sees the
 * rejection.
 */
export interface WriteQueue {
	/** Run `work` after every earlier write has settled. */
	run<T>(work: () => Promise<T>): Promise<T>;
	/** Resolves once every write issued so far has settled. */
	idle(): Promise<void>;
}

export function createWriteQueue(): WriteQueue {
	let tail: Promise<unknown> = Promise.resolve();
	return {
		run<T>(work: () => Promise<T>): Promise<T> {
			const next = tail.then(work);
			tail = next.catch(() => {});
			return next;
		},
		idle: () => tail.then(() => {}),
	};
}

type AsyncFn = (...args: never[]) => Promise<unknown>;

/** The same API as `api`, every call routed through `queue` in call order. */
export function queued<T extends Record<string, AsyncFn>>(api: T, queue: WriteQueue): T {
	const out: Record<string, (...args: unknown[]) => Promise<unknown>> = {};
	for (const [name, fn] of Object.entries(api)) {
		const call = fn as unknown as (...args: unknown[]) => Promise<unknown>;
		out[name] = (...args: unknown[]) => queue.run(() => call(...args));
	}
	return out as unknown as T;
}
