/**
 * `use:portal` moves a floating element (a menu, flyout, popover, its
 * backdrop) to the end of <body> and lifts it above every pane - the
 * chat/properties drawer beside the document column sits at z-index 150 and
 * used to cover any menu reaching past the column. The element's own
 * z-index order is kept (flyout over its menu, backdrop under both) by
 * adding PANE_LAYER to it. Fixed positioning and scoped styles are
 * unchanged; the element is removed with its component.
 */
const PANE_LAYER = 200;

export function portal(node: HTMLElement): { destroy(): void } {
	document.body.appendChild(node);
	const own = Number.parseInt(getComputedStyle(node).zIndex, 10);
	node.style.zIndex = String(PANE_LAYER + (Number.isFinite(own) ? own : 0));
	return {
		destroy() {
			node.remove();
		},
	};
}
