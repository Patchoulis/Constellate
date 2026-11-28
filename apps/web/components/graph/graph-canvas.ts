/**
 * The single-canvas graph renderer.
 *
 *  - A dumb draw function over plain data: papers with world-space positions, and the groups to colour them by.
 *    Layout (lib/graph/layout.ts) happens outside; this module never computes a position.
 *  - ONE canvas, ONE requestAnimationFrame loop with a dirty flag. Input and data changes only mutate state and
 *    mark dirty; a frame is scheduled only then, so an idle graph costs nothing. Every layer is drawn under the
 *    same camera (glows -> papers -> marked papers and the hovered paper's links -> labels), so nothing can
 *    lag anything else.
 *  - Camera: d3-zoom supplies pan / zoom-to-cursor / pinch (screen = world * k + [x, y]).
 *  - Picking: the nearest paper whose dot (plus a few pixels) is under the pointer (lib/graph/pick.ts); a
 *    scan of every paper, about a tenth of a millisecond at 12,500 papers (tests/graph/perf.bench.ts).
 *
 * The glow shows what belongs together. Every paper adds a soft radial halo in its group's hue, composited
 * additively ("lighter"). One halo is faint; where a group is dense the halos add up, so each group glows in its
 * own colour, brightest at its core. Halos are pre-rendered once per hue as sprites, and the glows of a level are
 * added up once into a layer of their own (lib/graph/glow-layer.ts), so a frame draws one image for them, not one per
 * paper. The dots are kept as paths in the same way while the zoom leaves their size alone.
 *
 * Links show what one paper is closest to, and only on request: hovering a paper draws lines to its few most
 * similar papers (the engine cuts each paper's list at the natural gap in its similarities) and marks them.
 * A paper the user marked (right-click) has a ring and shows its links all the time. When one community is in
 * focus (or kinds of paper are dimmed), that goes only for the marked papers in view: a focused view is about
 * what it focuses on, and marks elsewhere would draw lines across it.
 * Nothing else is ever linked on screen, so the picture stays dots and glows.
 *
 * Levels of detail: the data carries several groupings of the same papers, coarse to fine (communities, their
 * sub-communities, ...). The zoom picks one: zooming in, the community's single glow gives way to the glows and
 * labels of its sub-communities; zooming out brings the community back.
 *
 * Focus: an optional set of papers (one community or sub-community). Papers outside it are dim grey dots with
 * no glow, and cannot be picked.
 */
import { select } from "d3-selection";
import { zoom, zoomIdentity, type ZoomBehavior, type ZoomTransform } from "d3-zoom";
import { Glide, prefersLessMotion, type Size, type View } from "@/lib/graph/glide";
import { GLOW_LAYER_MOST_PIXELS, glowLayerGeometry, inside, visibleWorld, type GlowLayerGeometry } from "@/lib/graph/glow-layer";
import type { MarkedPlace } from "@/lib/graph/labels";
import type { Box } from "@/lib/graph/layout";
import { nearestPaper } from "@/lib/graph/pick";
import { hsl } from "@/lib/utils";

export interface CanvasGroup {
	hue: number;
	label: string;
	/** number of papers: bigger groups get their label first where labels would collide */
	size: number;
	/** where it is: its label goes at the centre, and may be as wide as the group is on screen */
	x: number;
	y: number;
	r: number;
}

/** One grouping of all the papers. */
export interface CanvasLevel {
	groups: CanvasGroup[];
	/** group index of every paper */
	of: Int32Array;
	/** radius of one paper's halo, world units */
	glow: number;
}

export interface CanvasData {
	points: { x: number; y: number }[];
	/** how much larger than usual each paper's dot is drawn (1 = usual) */
	sizes: Float32Array;
	/** [paper, paper, similarity], each pair once */
	links: [number, number, number][];
	/** coarse to fine; at least one */
	levels: CanvasLevel[];
}

/** The glows of one level under one focus, drawn once (lib/graph/glow-layer.ts); no canvas when it would be too large. */
type GlowLayer =
	| {
			level: CanvasLevel;
			focus: Set<number> | null;
			geometry: GlowLayerGeometry;
			canvas: HTMLCanvasElement;
			context: CanvasRenderingContext2D;
	  }
	| { level: CanvasLevel; focus: Set<number> | null; geometry: null; canvas: null; context: null };

export interface RendererCallbacks {
	/** The hovered paper (or null), where that paper is inside the canvas, and the same for the papers it is
	 *  linked to. Told again whenever the camera
	 *  moves, so whatever is shown beside the paper stays beside it. */
	onHover: (index: number | null, x: number, y: number, linked: { index: number; x: number; y: number }[]) => void;
	onPick: (index: number | null) => void;
	/** A paper was right-clicked: mark it, or unmark it. */
	onMark: (index: number) => void;
	/** Where the marked papers and the papers they are linked to are inside the canvas, newest mark first. Told
	 *  again whenever the camera moves, like onHover. */
	onMarked: (marked: MarkedPlace[]) => void;
	/** The zoom moved to another level of detail. */
	onLevel: (level: number) => void;
}

const NODE_R = 2.4; // world units
const PICK_MARGIN_PX = 5;
const SPRITE = 96;
const DIM = "rgba(120, 120, 120, 0.16)";
/** A feature's group with no hue of its own ("not stated", "not verified"): shown, but in no colour. */
const GREY = "rgba(190, 190, 190, 0.75)";
const MARKED_LINK = "rgba(255, 255, 255, 0.8)";
/** A marked paper's ring and links: the --mark colour of globals.css. Yellow is no community's dot colour at full
 *  strength next to white, and reads on the dark background. */
const MARK = "#fde047";
const MARK_LINK = "rgba(253, 224, 71, 0.45)";
const MARK_EDGE = "rgba(20, 20, 20, 0.85)"; // dark rim under the ring, so it reads over a bright glow too
const LABEL_LINE = 13; // px between the lines of a label
const LABEL_CHAR = 6.7; // px one character of the label font takes
const LABEL_MIN_CHARS = 24; // a label line may always be this long, however small the group is on screen
const LABEL_MIN_LINES = 3; // ... and a label may always have this many lines
/** Zoom, relative to the whole graph fitting the view, at which each finer level takes over. */
const LEVEL_ZOOM = [2.2, 4.6, 9, 16];

/** A label as whole words on at most `most` lines of about `chars` characters; only what still does not fit
 *  is cut, with an ellipsis. */
export function wrapLabel(label: string, chars: number, most: number): string[] {
	const lines = [""];
	for (const word of label.split(" ")) {
		const line = lines[lines.length - 1];
		if (!line || line.length + word.length < chars) lines[lines.length - 1] = line ? `${line} ${word}` : word;
		else if (lines.length < most) lines.push(word);
		else {
			lines[lines.length - 1] = `${line.replace(/[,:]$/, "")}…`;
			break;
		}
	}
	return lines;
}

/** A paper's dot, in world units at zoom k: its true size, but never under ~1px on screen. */
const dotRadius = (k: number) => Math.max(NODE_R, 0.85 / k);

function glowSprite(hue: number): HTMLCanvasElement {
	const canvas = document.createElement("canvas");
	canvas.width = canvas.height = SPRITE;
	const ctx = canvas.getContext("2d")!;
	const gradient = ctx.createRadialGradient(SPRITE / 2, SPRITE / 2, 0, SPRITE / 2, SPRITE / 2, SPRITE / 2);
	// Faint on purpose: a dozen or more halos overlap inside a group, and their sum is the glow.
	gradient.addColorStop(0, hsl(hue, 58, 0.075));
	gradient.addColorStop(0.45, hsl(hue, 55, 0.032));
	gradient.addColorStop(1, hsl(hue, 50, 0));
	ctx.fillStyle = gradient;
	ctx.fillRect(0, 0, SPRITE, SPRITE);
	return canvas;
}

export class GraphCanvasRenderer {
	private ctx: CanvasRenderingContext2D;
	private data: CanvasData | null = null;
	private sprites = new Map<number, HTMLCanvasElement>();
	/** every paper's linked papers, shown while it is hovered */
	private linked: number[][] = [];
	private transform: ZoomTransform = zoomIdentity;
	private zoomBehavior: ZoomBehavior<HTMLCanvasElement, unknown>;
	/** the camera's glide from one view to another: one at a time */
	private glide = new Glide();
	private fitZoom = 1;
	/** the box the whole graph occupies, once known: levels of detail are measured against fitting it */
	private whole: Box | null = null;
	/** the last box the camera was fitted to, and the camera that gave; if the camera is still that one when
	 *  the canvas changes size, nobody has moved it and the fit is simply redone */
	private fitted: { box: Box; transform: ZoomTransform } | null = null;
	private level = 0;
	private frame = 0;
	private focus: Set<number> | null = null;
	private selected: number | null = null;
	private hovered: number | null = null;
	/** the marked papers, newest first */
	private marked: number[] = [];
	/** papers drawn by a feature's groups instead of their communities (lib/graph/graph-colouring.ts); null: by community */
	private colouring: { of: Int16Array; hues: (number | null)[] } | null = null;
	/** the glows of the levels seen under the current focus, oldest first (glowLayerOf) */
	private glowLayers: GlowLayer[] = [];
	/** the dots of one level, focus and size (dotsOf) */
	private dotPaths: { level: CanvasLevel; focus: Set<number> | null; radius: number; dots: Path2D[]; dimmed: Path2D } | null = null;
	private observer: ResizeObserver;
	/** device pixels per CSS pixel the canvas's pixels were sized for: a frame draws at this, never at another */
	private dpr = 1;
	/** told when the device pixel ratio stops being `dpr` (the window moved to another screen, the page was
	 *  zoomed), which changes no CSS size and so tells the ResizeObserver nothing */
	private dprQuery: MediaQueryList | null = null;

	constructor(
		private canvas: HTMLCanvasElement,
		private callbacks: RendererCallbacks,
	) {
		this.ctx = canvas.getContext("2d")!;
		this.zoomBehavior = zoom<HTMLCanvasElement, unknown>()
			.scaleExtent([0.03, 40])
			.on("zoom", (event) => {
				this.transform = event.transform;
				this.updateLevel();
				this.tellHover(); // the hovered paper moved on screen
				this.tellMarked();
				this.markDirty();
			})
			// the person took hold of the graph (wheel, drag): a glide in progress gives way to them
			.on("start", (event) => event.sourceEvent && this.stopGliding());
		select(canvas).call(this.zoomBehavior).on("dblclick.zoom", null);
		canvas.addEventListener("mousemove", this.onMove);
		canvas.addEventListener("mouseleave", this.onLeave);
		canvas.addEventListener("click", this.onClick);
		canvas.addEventListener("contextmenu", this.onContextMenu);
		this.observer = new ResizeObserver(() => this.resize());
		this.observer.observe(canvas);
		this.resize();
	}

	destroy() {
		this.stopGliding();
		cancelAnimationFrame(this.frame);
		this.observer.disconnect();
		this.dprQuery?.removeEventListener("change", this.onDprChange);
		this.dprQuery = null;
		select(this.canvas).on(".zoom", null);
		this.canvas.removeEventListener("mousemove", this.onMove);
		this.canvas.removeEventListener("mouseleave", this.onLeave);
		this.canvas.removeEventListener("click", this.onClick);
		this.canvas.removeEventListener("contextmenu", this.onContextMenu);
		this.dropGlowLayers(this.glowLayers);
		this.glowLayers = [];
	}

	setData(data: CanvasData | null) {
		this.data = data;
		this.hovered = null;
		this.marked = [];
		this.colouring = null; // its papers were the last data's
		this.sprites.clear();
		this.dropGlowLayers(this.glowLayers);
		this.glowLayers = [];
		this.dotPaths = null;
		this.linked = (data?.points ?? []).map(() => []);
		// closest first, so that where only some of a paper's linked papers can be named, those are the closest
		for (const [a, b] of [...(data?.links ?? [])].sort((one, other) => other[2] - one[2])) {
			this.linked[a].push(b);
			this.linked[b].push(a);
		}
		this.updateLevel();
		this.tellHover(); // the paper hovered before is not one of these
		this.tellMarked();
		this.markDirty();
	}

	/**
	 * Draw the papers by a feature's groups (`of[paper]`, -1 for none; a group's hue, null for grey) instead of by
	 * their communities, or by their communities again (null). The glow layers and the dots' paths were drawn in
	 * the old colours: they are all dropped and drawn again for the new ones.
	 */
	setColouring(colouring: { of: Int16Array; hues: (number | null)[] } | null) {
		if (colouring === this.colouring) return;
		this.colouring = colouring;
		this.dropGlowLayers(this.glowLayers);
		this.glowLayers = [];
		this.dotPaths = null;
		this.markDirty();
	}

	/** The marked papers, newest first. */
	setMarked(indexes: number[]) {
		this.marked = indexes.filter((index) => this.data?.points[index]);
		this.tellMarked();
		this.markDirty();
	}

	/** Move the camera so a paper is in the middle of the part of the canvas left of `right` px from its right
	 *  edge (what a panel laid over the graph leaves free), zoomed in at least far enough to see its neighbours. */
	moveTo(index: number, right = 0) {
		const point = this.data?.points[index];
		const { clientWidth: width, clientHeight: height } = this.canvas;
		if (!point || !width || !height) return;
		const k = Math.max(this.transform.k, this.fitZoom * LEVEL_ZOOM[0]);
		// worked out for the canvas's size, and again if it changes on the way
		this.glideTo(({ width, height }) => {
			const free = width - right >= 160 ? width - right : width; // a sliver is no place to show it
			return { x: free / 2 - k * point.x, y: height / 2 - k * point.y, k };
		});
	}

	private stopGliding() {
		this.glide.stop();
	}

	/**
	 * Take the camera to `target` and call `arrived` there. It glides (lib/graph/glide.ts) unless told to go at once:
	 * the first view of a graph and a canvas that changed size are not journeys, and someone who asked their
	 * system for less motion gets none. A new destination ends a glide in progress and starts from where the
	 * camera is at that instant; so does the person taking hold of the graph.
	 */
	private glideTo(aim: (size: Size) => View, atOnce = false, arrived?: () => void) {
		const show = ({ x, y, k }: View) => select(this.canvas).call(this.zoomBehavior.transform, zoomIdentity.translate(x, y).scale(k));
		const { clientWidth: width, clientHeight: height } = this.canvas;
		if (atOnce || prefersLessMotion() || !width || !height) {
			this.glide.stop();
			show(aim({ width, height }));
			arrived?.();
			return;
		}
		this.glide.go(this.transform, aim, { width, height }, show, arrived);
	}

	setFocus(focus: Set<number> | null) {
		this.focus = focus;
		this.tellMarked(); // which marked papers are shown depends on the focus
		this.markDirty();
	}

	setSelected(index: number | null) {
		this.selected = index;
		this.markDirty();
	}

	/** Something changed: redraw on the next frame. */
	markDirty() {
		if (this.frame) return;
		this.frame = requestAnimationFrame(() => {
			this.frame = 0;
			this.draw();
		});
	}

	/** Move the camera so the given world rectangle fills the canvas. `whole` marks it as the full graph;
	 *  `atOnce` skips the glide (a graph's first view, a canvas that changed size). */
	fit(box: Box, whole = false, atOnce = false) {
		if (whole) this.whole = box;
		const width = this.canvas.clientWidth;
		const height = this.canvas.clientHeight;
		if (!width || !height) return;
		if (this.whole) this.fitZoom = this.zoomToFit(this.whole);
		this.fitted = null; // on the way, the camera is fitted to nothing
		// worked out for the canvas's size, and again if it changes on the way
		const aim = (size: Size): View => {
			const k = this.zoomToFit(box, size);
			return { x: size.width / 2 - (k * (box.minX + box.maxX)) / 2, y: size.height / 2 - (k * (box.minY + box.maxY)) / 2, k };
		};
		this.glideTo(aim, atOnce, () => (this.fitted = { box, transform: this.transform }));
	}

	/** The zoom at which a world rectangle just fits the canvas (or a picture of `size`), with a little margin. */
	private zoomToFit(box: Box, size: Size = { width: this.canvas.clientWidth, height: this.canvas.clientHeight }): number {
		return Math.min(size.width / (box.maxX - box.minX || 1), size.height / (box.maxY - box.minY || 1)) * 0.9;
	}

	/** The level of detail the current zoom shows. */
	private updateLevel() {
		const ratio = this.transform.k / this.fitZoom;
		const finest = (this.data?.levels.length ?? 1) - 1;
		const level = Math.min(finest, LEVEL_ZOOM.filter((threshold) => ratio >= threshold).length);
		if (level === this.level) return;
		this.level = level;
		this.callbacks.onLevel(level);
	}

	private sprite(hue: number): HTMLCanvasElement {
		const key = Math.round(hue);
		if (!this.sprites.has(key)) this.sprites.set(key, glowSprite(key));
		return this.sprites.get(key)!;
	}

	/** The glow layer of this level under the current focus, drawn now if it is not yet; null when it would be too
	 *  large to keep. Layers of other levels are kept too (zooming back is free) as long as all of them together
	 *  stay within GLOW_LAYER_MOST_PIXELS; a new focus makes them all stale. A level is only ever one data's, so a
	 *  new graph never finds an old layer. */
	private glowLayerOf(level: CanvasLevel, data: CanvasData): { geometry: GlowLayerGeometry; canvas: HTMLCanvasElement } | null {
		const known = this.glowLayers.find((layer) => layer.level === level && layer.focus === this.focus);
		// the browser may drop a canvas's pixels (graphics reset, low memory): such a layer is drawn again
		if (known && !known.context?.isContextLost()) return known.geometry ? known : null;
		const geometry = glowLayerGeometry(data.points, level.glow, SPRITE);
		const kept = this.glowLayers.filter((layer) => layer.focus === this.focus && layer !== known);
		const pixels = (layer: GlowLayer) => (layer.geometry ? layer.geometry.width * layer.geometry.height : 0);
		if (geometry)
			while (kept.reduce((sum, layer) => sum + pixels(layer), geometry.width * geometry.height) > GLOW_LAYER_MOST_PIXELS) kept.shift();
		this.dropGlowLayers(this.glowLayers.filter((layer) => !kept.includes(layer)));
		if (!geometry) {
			// remembered, so that the size is not worked out again every frame
			this.glowLayers = [...kept, { level, focus: this.focus, geometry: null, canvas: null, context: null }];
			return null;
		}
		const canvas = document.createElement("canvas");
		canvas.width = geometry.width;
		canvas.height = geometry.height;
		const layer = canvas.getContext("2d")!;
		const { box, scale } = geometry;
		layer.setTransform(scale, 0, 0, scale, -box.minX * scale, -box.minY * scale);
		layer.globalCompositeOperation = "lighter";
		const radius = level.glow;
		const paint = this.paintOf(level);
		data.points.forEach((point, index) => {
			const hue = this.glowHue(paint, index);
			if (hue !== null) layer.drawImage(this.sprite(hue), point.x - radius, point.y - radius, radius * 2, radius * 2);
		});
		const made = { level, focus: this.focus, geometry, canvas, context: layer };
		this.glowLayers = [...kept, made];
		return made;
	}

	/** Give back the pixels of glow layers no longer used now, rather than whenever they are collected: a layer
	 *  is tens of megabytes. */
	private dropGlowLayers(layers: GlowLayer[]) {
		for (const { canvas } of layers) if (canvas) canvas.width = canvas.height = 0;
	}

	/** The papers' dots as paths, one per group and one for the papers out of focus. They stay the same while the
	 *  level, the focus and the dots' size do: a pan, a hover, and any zoom close enough that dots have their true
	 *  size, reuse them. */
	private dotsOf(level: CanvasLevel, data: CanvasData, radius: number) {
		const kept = this.dotPaths;
		if (kept && kept.level === level && kept.focus === this.focus && kept.radius === radius) return kept;
		const paint = this.paintOf(level);
		const dots = paint.hues.map(() => new Path2D());
		const dimmed = new Path2D();
		data.points.forEach((point, index) => {
			const group = paint.of[index];
			const path = this.lit(index) && group >= 0 ? dots[group] : dimmed;
			const size = radius * data.sizes[index];
			path.moveTo(point.x + size, point.y);
			path.arc(point.x, point.y, size, 0, Math.PI * 2);
		});
		this.dotPaths = { level, focus: this.focus, radius, dots, dimmed };
		return this.dotPaths;
	}

	/** Which group each paper is drawn in (-1: dimmed) and each group's hue (null: grey): its community's at this
	 *  level, or the feature's colouring when one is set. */
	private paintOf(level: CanvasLevel): { of: ArrayLike<number>; hues: (number | null)[] } {
		if (this.colouring) return this.colouring;
		let hues = this.levelHues.get(level);
		if (!hues) this.levelHues.set(level, (hues = level.groups.map((group) => group.hue)));
		return { of: level.of, hues };
	}

	private levelHues = new WeakMap<CanvasLevel, number[]>();

	/** The hue of a paper's halo, or null for none: out of focus, in no group, or in a grey one. */
	private glowHue(paint: { of: ArrayLike<number>; hues: (number | null)[] }, index: number): number | null {
		const group = paint.of[index];
		return this.lit(index) && group >= 0 ? (paint.hues[group] ?? null) : null;
	}

	private lit(index: number): boolean {
		return this.focus === null || this.focus.has(index);
	}

	private onDprChange = () => this.resize();

	private resize() {
		const dpr = window.devicePixelRatio || 1;
		if (dpr !== this.dpr || !this.dprQuery) {
			this.dprQuery?.removeEventListener("change", this.onDprChange);
			this.dprQuery = window.matchMedia?.(`(resolution: ${dpr}dppx)`) ?? null;
			this.dprQuery?.addEventListener("change", this.onDprChange);
		}
		this.dpr = dpr;
		this.canvas.width = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
		this.canvas.height = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
		// a glide under way was aimed for the old size: it ends at once where it now should (lib/graph/glide.ts)
		if (this.whole) this.fitZoom = this.zoomToFit(this.whole);
		this.glide.resized({ width: this.canvas.clientWidth, height: this.canvas.clientHeight });
		if (this.fitted && this.fitted.transform === this.transform) this.fit(this.fitted.box, false, true);
		else if (this.whole) {
			this.fitZoom = this.zoomToFit(this.whole);
			this.updateLevel();
		}
		// what is inside the canvas changed with its size
		this.tellHover();
		this.tellMarked();
		this.markDirty();
	}

	/** The paper under the pointer, or null. Papers out of focus are not picked. */
	private pick(event: MouseEvent): number | null {
		if (!this.data) return null;
		const rect = this.canvas.getBoundingClientRect();
		const [wx, wy] = this.transform.invert([event.clientX - rect.left, event.clientY - rect.top]);
		// The pointer picks a paper anywhere on its dot plus a small margin: zoomed in, a dot is far bigger than
		// the margin, and a fixed reach would leave its outer part dead.
		const { k } = this.transform;
		// nearest dot whose own disc (they differ in size) plus the margin is under the pointer
		return nearestPaper(this.data.points, this.data.sizes, { x: wx, y: wy }, dotRadius(k), PICK_MARGIN_PX / k, (index) => this.lit(index));
	}

	/** Report the hovered paper and its current place on screen. */
	private tellHover() {
		const point = this.hovered === null ? undefined : this.data?.points[this.hovered];
		if (!point || this.hovered === null || !this.data) return this.callbacks.onHover(null, 0, 0, []);
		const { points } = this.data;
		const [x, y] = this.transform.apply([point.x, point.y]);
		// the papers its links (drawn while it is hovered) lead to, so they can be named too
		const { clientWidth, clientHeight } = this.canvas;
		const linked = this.linked[this.hovered]
			.filter((other) => this.lit(other))
			.map((other) => {
				const [ox, oy] = this.transform.apply([points[other].x, points[other].y]);
				return { index: other, x: ox, y: oy };
			})
			.filter((other) => other.x >= 0 && other.y >= 0 && other.x <= clientWidth && other.y <= clientHeight); // in view
		this.callbacks.onHover(this.hovered, x, y, linked);
	}

	/** The marked papers that are shown: all of them, or with a focus only those in it. */
	private shownMarked(): number[] {
		return this.focus === null ? this.marked : this.marked.filter((index) => this.lit(index));
	}

	/** Report where the shown marked papers and the papers they link to (those in focus) are on screen, in view
	 *  or not: the graph view picks which to name. */
	private tellMarked() {
		const points = this.data?.points;
		if (!points) return this.callbacks.onMarked([]);
		const place = (index: number) => {
			const [x, y] = this.transform.apply([points[index].x, points[index].y]);
			return { index, x, y };
		};
		this.callbacks.onMarked(
			this.shownMarked().map((index) => ({
				...place(index),
				linked: this.linked[index].filter((other) => this.lit(other)).map(place),
			})),
		);
	}

	private hover(index: number | null) {
		if (index === this.hovered) return;
		this.hovered = index;
		this.canvas.style.cursor = index === null ? "" : "pointer";
		this.tellHover();
		this.markDirty();
	}

	private onMove = (event: MouseEvent) => this.hover(this.pick(event));

	private onLeave = () => this.hover(null);

	// d3-zoom swallows the click that ends a drag, so this only fires for a real click.
	private onClick = (event: MouseEvent) => this.callbacks.onPick(this.pick(event));

	// Right-click marks a paper. The browser's own menu has nothing to offer on the graph, so it never opens here.
	private onContextMenu = (event: MouseEvent) => {
		event.preventDefault();
		const index = this.pick(event);
		if (index !== null) this.callbacks.onMark(index);
	};

	private draw() {
		const { ctx, data, dpr } = this;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, this.canvas.clientWidth, this.canvas.clientHeight);
		if (!data) return;
		const { x: tx, y: ty, k } = this.transform;
		const level = data.levels[Math.min(this.level, data.levels.length - 1)];
		const groupOf = (index: number) => level.groups[level.of[index]];

		// THE camera: one transform, then everything is world coordinates.
		ctx.setTransform(dpr * k, 0, 0, dpr * k, dpr * tx, dpr * ty);
		const glow = (index: number, radius: number) => {
			const point = data.points[index];
			ctx.drawImage(this.sprite(groupOf(index).hue), point.x - radius, point.y - radius, radius * 2, radius * 2);
		};

		// 1. Glows, additive: halos of one hue pile up into the group's glow. Drawn once into the glow layer while
		// the level and the focus stay the same; when that layer would be too large, halo by halo, only those that
		// reach the screen (the canvas would clip the others, after paying for them).
		ctx.globalCompositeOperation = "lighter";
		const layer = this.glowLayerOf(level, data);
		if (layer) {
			const { box } = layer.geometry;
			ctx.drawImage(layer.canvas, box.minX, box.minY, box.maxX - box.minX, box.maxY - box.minY);
		} else {
			const reach = visibleWorld(this.transform, this.canvas.clientWidth, this.canvas.clientHeight, level.glow);
			const paint = this.paintOf(level);
			data.points.forEach((point, index) => {
				if (!inside(reach, point.x, point.y)) return;
				const hue = this.glowHue(paint, index);
				if (hue !== null) ctx.drawImage(this.sprite(hue), point.x - level.glow, point.y - level.glow, level.glow * 2, level.glow * 2);
			});
		}
		ctx.globalCompositeOperation = "source-over";

		// 1b. Links, under the papers: a line runs from a paper's centre to another's and crosses whatever lies between,
		// so every dot is drawn over it. The marked papers' links (those in focus), then the hovered paper's.
		const marked = this.shownMarked();
		if (marked.length > 0) {
			const links = new Path2D();
			for (const index of marked) {
				const point = data.points[index];
				for (const other of this.linked[index]) {
					if (!this.lit(other)) continue;
					const end = data.points[other];
					links.moveTo(point.x, point.y);
					links.lineTo(end.x, end.y);
				}
			}
			ctx.strokeStyle = MARK_LINK;
			ctx.lineWidth = 1 / k;
			ctx.stroke(links);
		}
		if (this.hovered !== null && data.points[this.hovered]) {
			const point = data.points[this.hovered];
			ctx.strokeStyle = MARKED_LINK;
			ctx.lineWidth = 1.3 / k;
			ctx.beginPath();
			for (const other of this.linked[this.hovered]) {
				if (!this.lit(other)) continue;
				const end = data.points[other];
				ctx.moveTo(point.x, point.y);
				ctx.lineTo(end.x, end.y);
			}
			ctx.stroke();
		}

		// 2. Papers: one path per group (a community, or a feature's group) plus one for everything out of focus or
		// in no group. Never under ~1px on screen.
		const radius = dotRadius(k);
		const { dots, dimmed } = this.dotsOf(level, data, radius);
		const { hues } = this.paintOf(level);
		ctx.fillStyle = DIM;
		ctx.fill(dimmed);
		dots.forEach((path, group) => {
			const hue = hues[group];
			ctx.fillStyle = hue === null ? GREY : hsl(hue, 74);
			ctx.fill(path);
		});

		// 3. Marked papers (those in focus): the papers their links (1b) end at, then a yellow ring on a dark rim. The
		// ring sits outside where the selected paper's ring goes, so a paper that is both shows both.
		if (marked.length > 0) {
			const ends = new Path2D();
			const rings = new Path2D();
			for (const index of marked) {
				const point = data.points[index];
				for (const other of this.linked[index]) {
					if (!this.lit(other)) continue;
					const end = data.points[other];
					const size = radius * data.sizes[other] * 1.1;
					ends.moveTo(end.x + size, end.y);
					ends.arc(end.x, end.y, size, 0, Math.PI * 2);
				}
				const ring = radius * data.sizes[index] * 1.5 + 7 / k;
				rings.moveTo(point.x + ring, point.y);
				rings.arc(point.x, point.y, ring, 0, Math.PI * 2);
			}
			ctx.fillStyle = MARK_LINK;
			ctx.fill(ends);
			ctx.strokeStyle = MARK_EDGE;
			ctx.lineWidth = 4 / k;
			ctx.stroke(rings);
			ctx.strokeStyle = MARK;
			ctx.lineWidth = 2 / k;
			ctx.stroke(rings);
		}

		// 4. Hovered and selected papers: a halo, a white core, a ring. The hovered one also shows the papers its links
		// (1b) end at.
		for (const index of new Set([this.selected, this.hovered])) {
			if (index === null || !data.points[index]) continue;
			const point = data.points[index];
			const ends = index === this.hovered ? this.linked[index].filter((other) => this.lit(other)).map((other) => data.points[other]) : [];
			ctx.fillStyle = "#ffffff";
			ctx.beginPath();
			for (const end of ends) {
				ctx.moveTo(end.x + radius * 1.25, end.y);
				ctx.arc(end.x, end.y, radius * 1.25, 0, Math.PI * 2);
			}
			ctx.fill();
			ctx.globalCompositeOperation = "lighter";
			for (let pass = 0; pass < 9; pass++) glow(index, 26);
			ctx.globalCompositeOperation = "source-over";
			ctx.fillStyle = "#ffffff";
			ctx.beginPath();
			ctx.arc(point.x, point.y, radius * 1.5, 0, Math.PI * 2);
			ctx.fill();
			ctx.strokeStyle = hsl(groupOf(index).hue, 80);
			ctx.lineWidth = 1.4 / k;
			ctx.beginPath();
			ctx.arc(point.x, point.y, radius * 1.5 + 4 / k, 0, Math.PI * 2);
			ctx.stroke();
		}

		// 5. Labels of the groups that have a paper in focus, in screen space so they stay one size.
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		const shown = new Set<number>();
		if (this.focus) for (const index of this.focus) shown.add(level.of[index]);
		ctx.font = '600 11px "JetBrains Mono", ui-monospace, monospace';
		ctx.textAlign = "center";
		ctx.textBaseline = "middle";
		ctx.lineJoin = "round";
		ctx.lineWidth = 3;
		// Biggest groups first; a label that would cover one already placed waits until there is room (zoom in).
		const placed: { left: number; right: number; top: number; bottom: number }[] = [];
		const order = level.groups.map((_, index) => index).sort((a, b) => level.groups[b].size - level.groups[a].size);
		for (const index of order) {
			if (this.focus && !shown.has(index)) continue;
			const group = level.groups[index];
			// A label gets the room its group has on screen, and never less than the minimum: zoomed in, the
			// whole name is there to read.
			const wide = 2 * group.r * k;
			const lines = wrapLabel(
				group.label,
				Math.max(LABEL_MIN_CHARS, Math.floor(wide / LABEL_CHAR)),
				Math.max(LABEL_MIN_LINES, Math.floor(wide / (2 * LABEL_LINE))),
			);
			const x = group.x * k + tx;
			const y = group.y * k + ty;
			const half = Math.max(...lines.map((line) => ctx.measureText(line).width)) / 2 + 4;
			const tall = (lines.length * LABEL_LINE) / 2 + 2;
			const box = { left: x - half, right: x + half, top: y - tall, bottom: y + tall };
			const covers = (other: typeof box) =>
				box.left < other.right && box.right > other.left && box.top < other.bottom && box.bottom > other.top;
			if (placed.some(covers)) continue;
			placed.push(box);
			lines.forEach((line, row) => {
				const lineY = y + (row - (lines.length - 1) / 2) * LABEL_LINE;
				ctx.strokeStyle = "rgba(20, 20, 20, 0.85)";
				ctx.strokeText(line, x, lineY);
				ctx.fillStyle = hsl(group.hue, 86);
				ctx.fillText(line, x, lineY);
			});
		}
	}
}
