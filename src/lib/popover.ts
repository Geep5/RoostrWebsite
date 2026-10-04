/**
 * Keep a popover fully on screen, whatever the zoom or window size. It is
 * pinned (position: fixed, so no scrolling ancestor clips it) against its
 * anchor - the element it opens from, its parent by default:
 *
 * - `below`: under the anchor, its right edge or left edge on the anchor's
 *   (`align`), shifted back inside the window when it would spill over.
 * - `beside`: to the anchor's left, else its right, else - no room either
 *   way, a narrow or zoomed-in window - under it.
 *
 * It is never taller than the window and is lifted up when it would run
 * off the bottom; its own scroll areas take the rest. Re-placed on resize
 * and scroll.
 */
const MARGIN = 8;
const GAP = 12;

export interface KeepInViewOptions {
	placement: "below" | "beside";
	align?: "left" | "right";
}

export function keepInView(node: HTMLElement, options: KeepInViewOptions) {
	let opts = options;
	const place = () => {
		const anchor = node.parentElement;
		if (!anchor) return;
		const a = anchor.getBoundingClientRect();
		const vw = window.innerWidth;
		const vh = window.innerHeight;
		node.style.position = "fixed";
		node.style.right = "auto";
		node.style.bottom = "auto";
		node.style.maxWidth = `${vw - 2 * MARGIN}px`;
		const w = node.offsetWidth;
		let left: number;
		let top: number;
		const roomLeft = a.left - GAP - MARGIN;
		const roomRight = vw - a.right - GAP - MARGIN;
		if (opts.placement === "beside" && roomLeft >= w) {
			left = a.left - GAP - w;
			top = a.top - 6;
		} else if (opts.placement === "beside" && roomRight >= w) {
			left = a.right + GAP;
			top = a.top - 6;
		} else {
			left = opts.align === "right" ? a.right - w : a.left;
			top = a.bottom + 6;
		}
		left = Math.min(Math.max(MARGIN, left), Math.max(MARGIN, vw - w - MARGIN));
		// Never taller than the window; lifted up when it would run off the bottom.
		node.style.maxHeight = `${vh - 2 * MARGIN}px`;
		const h = node.offsetHeight;
		top = Math.min(Math.max(MARGIN, top), Math.max(MARGIN, vh - h - MARGIN));
		node.style.left = `${left}px`;
		node.style.top = `${top}px`;
	};
	place();
	// Content (async lists) can change the width after mount.
	const resize = new ResizeObserver(place);
	resize.observe(node);
	window.addEventListener("resize", place);
	window.addEventListener("scroll", place, true);
	return {
		update(next: KeepInViewOptions) {
			opts = next;
			place();
		},
		destroy() {
			resize.disconnect();
			window.removeEventListener("resize", place);
			window.removeEventListener("scroll", place, true);
		},
	};
}
