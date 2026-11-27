/**
 * How large the panels around the graph are: what each may be, what is left of the window for the graph, how a
 * drag or a key changes a size, and what is read back from storage.
 *
 * Every size is in CSS pixels. A panel's size is the one dimension a user changes by dragging its edge: the width
 * of a side panel or of a panel laid over the graph, the height of one laid along its bottom. The user's sizes are
 * kept as chosen ("wanted"); what is shown ("fitted") is that, made smaller where the window is too small for it,
 * so making the window larger again brings the chosen size back.
 *
 * Sizes are kept in this browser's localStorage under one key, as `{"version":1,"sizes":{"sidebar":288,...}}`.
 * This module is pure; `useResizable` (lib/ui/use-resizable.ts) holds the sizes and the window's size in React.
 */

/** The panels a user can resize. */
export type PanelId = "sidebar" | "communities" | "reader" | "overview" | "log";

/** The edge of a panel its handle is on: the one facing the graph. */
export type Edge = "left" | "right" | "top";

export interface PanelSpec {
	/** what the handle is called to assistive technology */
	label: string;
	edge: Edge;
	/** the size it has until resized, and again after a reset */
	initial: number;
	/** the smallest size its content is usable at, and the largest it is ever made */
	min: number;
	max: number;
}

export const PANELS: Record<PanelId, PanelSpec> = {
	sidebar: { label: "Resize the runs sidebar", edge: "right", initial: 288, min: 240, max: 560 },
	communities: { label: "Resize the communities panel", edge: "left", initial: 352, min: 320, max: 720 },
	reader: { label: "Resize the paper reader", edge: "left", initial: 896, min: 400, max: 1800 },
	overview: { label: "Resize the overview", edge: "left", initial: 832, min: 400, max: 1800 },
	log: { label: "Resize the activity log", edge: "top", initial: 224, min: 80, max: 900 },
};

const IDS = Object.keys(PANELS) as PanelId[];

export type Sizes = Record<PanelId, number>;

/** The window's inner size. */
export interface Frame {
	width: number;
	height: number;
}

export interface Limits {
	min: number;
	max: number;
}

/** What the panels always leave of the graph. */
export const GRAPH_MIN_WIDTH = 320;
export const GRAPH_MIN_HEIGHT = 200;
/** The log's own line and its distance from the bottom of the window: what it takes beyond the height of its text. */
const LOG_CHROME = 46;

/** One step of an arrow key, and of an arrow key with Shift. */
export const KEY_STEP = 16;
export const KEY_STEP_LARGE = 64;

export function initialSizes(): Sizes {
	return Object.fromEntries(IDS.map((id) => [id, PANELS[id].initial])) as Sizes;
}

const clamp = (value: number, { min, max }: Limits) => Math.round(Math.min(max, Math.max(min, value)));

/** A range that is never empty: where the room left is less than the minimum, the minimum wins. */
const range = (min: number, max: number): Limits => ({ min, max: Math.max(min, Math.floor(max)) });

/**
 * The sizes as shown in a window of this size. Each is first kept within its own minimum and maximum. Then, if
 * the two side panels leave the graph less than GRAPH_MIN_WIDTH, both give way, each in proportion to how far it
 * is above its minimum; a panel over the graph or along its bottom is made no larger than leaves the graph
 * GRAPH_MIN_WIDTH by GRAPH_MIN_HEIGHT. No size goes below its minimum: in a window too small for even that, the
 * graph is what gets less. Without a frame (where there is no window, as when the page is rendered on a server),
 * only each panel's own limits apply.
 */
export function fitSizes(wanted: Sizes, frame: Frame | null): Sizes {
	const sizes = Object.fromEntries(IDS.map((id) => [id, clamp(wanted[id], PANELS[id])])) as Sizes;
	if (!frame) return sizes;
	const over = sizes.sidebar + sizes.communities + GRAPH_MIN_WIDTH - frame.width;
	const room = { sidebar: sizes.sidebar - PANELS.sidebar.min, communities: sizes.communities - PANELS.communities.min };
	const spare = room.sidebar + room.communities;
	if (over > 0 && spare > 0) {
		const cut = Math.min(over, spare);
		// rounded up, so the two cuts together are never less than what was needed (and neither more than its room)
		sizes.sidebar -= Math.ceil((cut * room.sidebar) / spare);
		sizes.communities -= Math.ceil((cut * room.communities) / spare);
	}
	for (const id of ["reader", "overview", "log"] as const) sizes[id] = clamp(sizes[id], limitsOf(id, sizes, frame));
	return sizes;
}

/**
 * The sizes a panel can be dragged to, given the sizes the others are shown at (`fitted`) in a window of this
 * size: from its minimum to whatever leaves the graph GRAPH_MIN_WIDTH by GRAPH_MIN_HEIGHT, and never beyond its
 * maximum. The panels over the graph are measured against the graph's part of the window, between the side panels.
 * The communities panel is counted even while no run is shown and it is not on screen: the limits are then a
 * little stricter than they need be, never too loose.
 */
export function limitsOf(id: PanelId, fitted: Sizes, frame: Frame | null): Limits {
	const { min, max } = PANELS[id];
	if (!frame) return { min, max };
	const graphWidth = frame.width - fitted.sidebar - fitted.communities;
	switch (id) {
		case "sidebar":
			return range(min, Math.min(max, frame.width - fitted.communities - GRAPH_MIN_WIDTH));
		case "communities":
			return range(min, Math.min(max, frame.width - fitted.sidebar - GRAPH_MIN_WIDTH));
		case "reader":
		case "overview":
			return range(min, Math.min(max, graphWidth - GRAPH_MIN_WIDTH));
		case "log":
			return range(min, Math.min(max, frame.height - GRAPH_MIN_HEIGHT - LOG_CHROME));
	}
}

/** The size a drag has reached: the pointer moved from `from` to `to` (along the edge's axis) since it began. */
export function draggedSize(start: number, from: number, to: number, edge: Edge, limits: Limits): number {
	// a handle on the right edge grows its panel as it moves right; one on the left or top edge as it moves left or up
	const moved = edge === "right" ? to - from : from - to;
	return clamp(start + moved, limits);
}

/**
 * What a key pressed on a handle does to its panel's size: the arrow keys along its axis move the edge by
 * KEY_STEP (KEY_STEP_LARGE with Shift), Home and End go to the smallest and the largest size, Enter returns to the
 * initial size. Null for any other key, which the handle leaves alone.
 */
export function keyedSize(key: string, shift: boolean, size: number, edge: Edge, limits: Limits): number | "reset" | null {
	const step = shift ? KEY_STEP_LARGE : KEY_STEP;
	// the arrow that moves the edge away from its panel makes the panel larger
	const grow = { left: "ArrowLeft", right: "ArrowRight", top: "ArrowUp" }[edge];
	const shrink = { left: "ArrowRight", right: "ArrowLeft", top: "ArrowDown" }[edge];
	if (key === grow) return clamp(size + step, limits);
	if (key === shrink) return clamp(size - step, limits);
	if (key === "Home") return limits.min;
	if (key === "End") return limits.max;
	if (key === "Enter") return "reset";
	return null;
}

export const PANEL_SIZES_KEY = "constellate.panels";
const VERSION = 1;

export function serialisePanelSizes(sizes: Sizes): string {
	return JSON.stringify({ version: VERSION, sizes });
}

/**
 * Whatever storage holds, read as the panels' sizes. Anything that cannot be trusted (nothing, broken JSON,
 * another shape or version) gives the initial sizes; so does, for its panel, a size that is not a number within
 * that panel's limits. Never an error.
 */
export function parsePanelSizes(text: string | null | undefined): Sizes {
	const sizes = initialSizes();
	if (!text) return sizes;
	let stored: unknown;
	try {
		stored = JSON.parse(text);
	} catch (error) {
		return sizes;
	}
	if (typeof stored !== "object" || stored === null) return sizes;
	const { version, sizes: kept } = stored as { version?: unknown; sizes?: unknown };
	if (version !== VERSION || typeof kept !== "object" || kept === null) return sizes;
	for (const id of IDS) {
		const size = (kept as Record<string, unknown>)[id];
		if (typeof size === "number" && Number.isFinite(size) && size >= PANELS[id].min && size <= PANELS[id].max) sizes[id] = Math.round(size);
	}
	return sizes;
}
