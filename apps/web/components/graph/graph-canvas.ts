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
 *
 * The intro (lib/graph/intro.ts): for a few seconds after a run is made, the same layers are drawn as a sky that
 * becomes the graph. Pale stars gather to the papers' places, a line is traced through a few papers of each
 * community, the glows and colours come up (the glow layer drawn fainter, the dots' paths in pale and then in their
 * colours) and the labels last. Its last frame is the usual graph. While it plays, nothing is hovered, picked or
 * marked; a press, a wheel or a key ends it at once.
 */
import { select } from "d3-selection";
import { zoom, zoomIdentity, type ZoomBehavior, type ZoomTransform } from "d3-zoom";
import { Glide, prefersLessMotion, type Size, type View } from "@/lib/graph/glide";
import { GLOW_LAYER_MOST_PIXELS, glowLayerGeometry, inside, visibleWorld, type GlowLayerGeometry } from "@/lib/graph/glow-layer";
import {
	INTRO_MS,
	LINE_ALPHA,
	introFrame,
	introPlan,
	introProgress,
	packConstellations,
	paintGathering,
	tracedTo,
	TWINKLE_GROUPS,
	type Constellations,
	type IntroFrame,
	type IntroPlan,
} from "@/lib/graph/intro";
import { MOST_LABELS, type MarkedPlace } from "@/lib/graph/labels";
import { screenDotRadius } from "@/lib/graph/dot-size";
import { REGION_FROM, REGION_GLOW, haloLayerFits, lookTowards, markLook } from "@/lib/graph/mark-look";
import { FIELD_STARS, FIELD_TILE, STAR_COLOUR, fieldOffset, fieldStars, skyStarCount, type StarField } from "@/lib/graph/sky";
import type { Box } from "@/lib/graph/layout";
import { LASSO_MIN, lassoed, reach, type Place } from "@/lib/graph/lasso";
import { nearestPaper } from "@/lib/graph/pick";
import { selectKeyHeld, type SelectKey } from "@/lib/ui/prefs";
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
	/** A paper was right-clicked, or clicked with the select key held: mark it, or unmark it. */
	onMark: (index: number) => void;
	/** A lasso was drawn (a drag with the select key held): mark the papers inside it. */
	onMarkMany: (indexes: number[]) => void;
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
const LASSO_FILL = "rgba(253, 224, 71, 0.08)";
/** How far a marked paper's soft halo reaches in the region look, in screen pixels (lib/graph/mark-look.ts). */
const REGION_HALO_PX = 9;
/** Pixels across one halo in the layer the halos are kept in: a soft blob needs few. */
const HALO_TEXELS = 16;
/** A click this soon after a lasso or a select-click ended is the same press: it opens nothing. */
const SAME_PRESS_MS = 300;
const LABEL_LINE = 13; // px between the lines of a label
const LABEL_CHAR = 6.7; // px one character of the label font takes
const LABEL_MIN_CHARS = 24; // a label line may always be this long, however small the group is on screen
const LABEL_MIN_LINES = 3; // ... and a label may always have this many lines
/** Zoom, relative to the whole graph fitting the view, at which each finer level takes over. */
const LEVEL_ZOOM = [2.2, 4.6, 9, 16];
/** The intro's stars: pale white-blue, as the loading sky's (components/graph/sky-loading.tsx). */
const STAR = STAR_COLOUR;
const STAR_LINE = "#ffffff";
/** The same colour as one pixel's red, green and blue, as a little-endian word (every machine the app runs on). */
const STAR_WORD = (255 << 16) | (226 << 8) | 214;
/** A star is drawn as a square of the dot's area, a little smaller: a square is far cheaper than a disc. */
const STAR_SIDE = 1.6;
/** The star field behind the graph (lib/graph/sky.ts), its brightest stars first: every level draws the first of them. */
const FIELD = fieldStars(FIELD_STARS.many);

/** The shown marked papers as drawn (GraphCanvasRenderer.marksOf). */
interface MarkDrawing {
	links: Float64Array;
	ends: Path2D;
	rings: Float64Array;
}

/** What the intro works out once, before its first frame. */
interface Intro {
	started: number;
	duration: number;
	plan: IntroPlan;
	/** how strong each twinkle group is this frame (lib/graph/intro.ts paintGathering), made once */
	groupAlpha: Float32Array;
	constellations: Constellations;
	/** the pixels the gathering stars are written into (starPixelsOf) */
	pixels: { canvas: HTMLCanvasElement; context: CanvasRenderingContext2D; image: ImageData; words: Uint32Array } | null;
}

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

/** A paper's dot, in world units at zoom k: its true size, but never under ~1px on screen (lib/graph/dot-size.ts). */
const dotRadius = (k: number) => screenDotRadius(NODE_R, k);

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
	/** the marked papers drawn without their links (their ring and name only) */
	private noLinks: ReadonlySet<number> = new Set();
	/** the key that selects: with it held, a click marks a paper and a drag draws a lasso instead of moving the graph */
	private selectKey: SelectKey = "primary";
	/** the lasso being drawn: the pointer drawing it and where it went, in the canvas's CSS pixels */
	private lasso: { pointer: number; line: Place[] } | null = null;
	/** when the last press with the select key ended: the click the browser sends after it is not a pick */
	private selectedAt = -Infinity;
	/** papers drawn by a feature's groups instead of their communities (lib/graph/graph-colouring.ts); null: by community */
	private colouring: { of: Int16Array; hues: (number | null)[] } | null = null;
	/** the glows of the levels seen under the current focus, oldest first (glowLayerOf) */
	private glowLayers: GlowLayer[] = [];
	/** the dots of one level, focus and size (dotsOf) */
	private dotPaths: { level: CanvasLevel; focus: Set<number> | null; radius: number; dots: Path2D[]; dimmed: Path2D } | null = null;
	/** the marked papers' links, ends and rings, and what they were made for (marksOf) */
	private markPaths: {
		marked: number[];
		noLinks: ReadonlySet<number>;
		focus: Set<number> | null;
		radius: number;
		drawn: MarkDrawing | null;
	} | null = null;
	private observer: ResizeObserver;
	/** device pixels per CSS pixel the canvas's pixels were sized for: a frame draws at this, never at another */
	private dpr = 1;
	/** told when the device pixel ratio stops being `dpr` (the window moved to another screen, the page was
	 *  zoomed), which changes no CSS size and so tells the ResizeObserver nothing */
	private dprQuery: MediaQueryList | null = null;
	/** how many stars are drawn behind the graph, and one tile of them, drawn once (starTileOf) */
	private stars: StarField = "few";
	private starTile: { count: number; dpr: number; canvas: HTMLCanvasElement; pattern: CanvasPattern } | null = null;
	/** the camera when the whole graph was first fitted: the star field moves with the pan from there */
	private origin: { x: number; y: number } | null = null;
	/** from how many shown marked papers they are a highlighted region (lib/graph/mark-look.ts), and the look: 0 rings,
	 *  1 region, and between while one gives way to the other */
	private regionFrom: number = REGION_FROM.initial;
	private look = { from: 0, target: 0, started: 0 };
	/** the dots in the region look: the shown marked papers in their colours, every other paper dimmed (regionDotsOf) */
	private regionDots: {
		level: CanvasLevel;
		focus: Set<number> | null;
		marked: number[];
		colouring: GraphCanvasRenderer["colouring"];
		radius: number;
		dots: Path2D[];
		dimmed: Path2D;
	} | null = null;
	/** the soft pale-yellow halo of a marked paper in the region look, and all the shown ones drawn together (haloLayerOf) */
	private markHalo: HTMLCanvasElement | null = null;
	private haloLayer: {
		marked: number[];
		focus: Set<number> | null;
		noLinks: ReadonlySet<number>;
		k: number;
		layer: { canvas: HTMLCanvasElement; box: Box } | null;
	} | null = null;
	/** the camera moved since the hovered and marked papers' places were last told (draw tells them) */
	private cameraMoved = false;
	/** the intro, while it plays */
	private intro: Intro | null = null;
	/** the press that ended the intro: the click or right-click it ends in opens and marks nothing */
	private skipPress = false;
	/** where the intro draws the labels it fades in (labelLayerOf) */
	private labelLayer: CanvasRenderingContext2D | null = null;

	constructor(
		private canvas: HTMLCanvasElement,
		private callbacks: RendererCallbacks,
	) {
		this.ctx = canvas.getContext("2d")!;
		this.zoomBehavior = zoom<HTMLCanvasElement, unknown>()
			.scaleExtent([0.03, 40])
			// d3-zoom's own rule (no secondary button; Ctrl only with the wheel, which is a pinch), and not a press
			// with the select key held: that one marks or draws a lasso
			.filter(
				(event: MouseEvent | WheelEvent | TouchEvent) =>
					(!event.ctrlKey || event.type === "wheel") &&
					!("button" in event && event.button) &&
					(event.type === "wheel" || !selectKeyHeld(this.selectKey, event)),
			)
			.on("zoom", (event) => {
				this.transform = event.transform;
				this.updateLevel();
				// the hovered and marked papers moved on screen: told with the next frame, once however many moves
				// come before it
				this.cameraMoved = true;
				this.markDirty();
			})
			// the person took hold of the graph (wheel, drag): a glide in progress gives way to them
			.on("start", (event) => event.sourceEvent && this.stopGliding());
		// Before anything else hears them: a press, a wheel or a key ends the intro, and then does what it always does.
		canvas.addEventListener("pointerdown", this.onSkipPress, { capture: true });
		canvas.addEventListener("wheel", this.onSkip, { capture: true, passive: true });
		window.addEventListener("keydown", this.onSkip, { capture: true });
		select(canvas).call(this.zoomBehavior).on("dblclick.zoom", null);
		canvas.addEventListener("mousemove", this.onMove);
		canvas.addEventListener("mouseleave", this.onLeave);
		canvas.addEventListener("click", this.onClick);
		canvas.addEventListener("contextmenu", this.onContextMenu);
		canvas.addEventListener("pointerdown", this.onPointerDown);
		canvas.addEventListener("pointermove", this.onPointerMove);
		canvas.addEventListener("pointerup", this.onPointerUp);
		canvas.addEventListener("pointercancel", this.onPointerCancel);
		this.observer = new ResizeObserver(() => this.resize());
		this.observer.observe(canvas);
		this.resize();
		// The community labels are drawn in the page's own font, which may still be loading for the first frames: they
		// are drawn again once it is there, rather than left in the fallback font until something else moves.
		document.fonts?.addEventListener("loadingdone", this.onFontsLoaded);
		void document.fonts?.ready.then(this.onFontsLoaded);
	}

	private onFontsLoaded = () => this.markDirty();

	destroy() {
		this.stopGliding();
		cancelAnimationFrame(this.frame);
		this.observer.disconnect();
		document.fonts?.removeEventListener("loadingdone", this.onFontsLoaded);
		this.dprQuery?.removeEventListener("change", this.onDprChange);
		this.dprQuery = null;
		select(this.canvas).on(".zoom", null);
		this.endIntro();
		this.canvas.removeEventListener("pointerdown", this.onSkipPress, { capture: true });
		this.canvas.removeEventListener("wheel", this.onSkip, { capture: true });
		window.removeEventListener("keydown", this.onSkip, { capture: true });
		this.canvas.removeEventListener("mousemove", this.onMove);
		this.canvas.removeEventListener("mouseleave", this.onLeave);
		this.canvas.removeEventListener("click", this.onClick);
		this.canvas.removeEventListener("contextmenu", this.onContextMenu);
		this.canvas.removeEventListener("pointerdown", this.onPointerDown);
		this.canvas.removeEventListener("pointermove", this.onPointerMove);
		this.canvas.removeEventListener("pointerup", this.onPointerUp);
		this.canvas.removeEventListener("pointercancel", this.onPointerCancel);
		this.dropGlowLayers(this.glowLayers);
		this.glowLayers = [];
	}

	setData(data: CanvasData | null) {
		this.data = data;
		this.endIntro(); // it was these papers' intro
		this.hovered = null;
		this.marked = [];
		this.colouring = null; // its papers were the last data's
		this.sprites.clear();
		this.dropGlowLayers(this.glowLayers);
		this.glowLayers = [];
		this.dotPaths = null;
		this.markPaths = null;
		this.regionDots = null;
		if (this.haloLayer?.layer) this.haloLayer.layer.canvas.width = this.haloLayer.layer.canvas.height = 0;
		this.haloLayer = null;
		this.look = { from: 0, target: 0, started: 0 }; // nothing of these papers is marked yet
		this.origin = null;
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

	/** The marked papers drawn as marked, newest first (those hidden on the graph are left out by the caller, and drawn
	 *  as any paper), and those of them drawn without their links: with their ring and name only. */
	setMarked(indexes: number[], noLinks: ReadonlySet<number> = new Set()) {
		this.marked = indexes.filter((index) => this.data?.points[index]);
		this.noLinks = noLinks;
		this.updateLook();
		this.tellMarked();
		this.markDirty();
	}

	/** From how many shown marked papers they are drawn as a highlighted region instead of rings. */
	setRegionFrom(from: number) {
		if (from === this.regionFrom) return;
		this.regionFrom = from;
		this.updateLook();
		this.tellMarked();
		this.markDirty();
	}

	/** In the region, the marked papers shown in full: the hovered one and the selected one, when they are shown marks.
	 *  Looked up without going through every mark, as it is asked for on every frame. */
	private markedInFull(): number[] {
		const full: number[] = [];
		for (const index of [this.hovered, this.selected])
			if (index !== null && !full.includes(index) && this.lit(index) && this.marked.includes(index)) full.push(index);
		return full;
	}

	/** Whether the shown marked papers are the region now (or becoming it). */
	private get inRegion(): boolean {
		return this.look.target === 1;
	}

	/** The look for the marked papers shown now; a change of look fades over LOOK_FADE_MS, or is at once with less
	 *  motion. */
	private updateLook() {
		const target = markLook(this.shownMarked().length, this.regionFrom) === "region" ? 1 : 0;
		if (target === this.look.target) return;
		const now = performance.now();
		this.look = { from: prefersLessMotion() ? target : this.lookNow(now), target, started: now };
	}

	/** How far the look is toward the region now: 0 rings, 1 region. */
	private lookNow(now: number): number {
		return lookTowards(this.look.from, this.look.target, now - this.look.started);
	}

	/** The key that, held, makes a click mark a paper and a drag draw a lasso. */
	setSelectKey(key: SelectKey) {
		this.selectKey = key;
	}

	/**
	 * Play the intro over the papers set last (lib/graph/intro.ts), `lines` being each community's constellation (its
	 * papers in the order the line goes through them), with the camera fitted to the whole graph by the caller. Not for someone
	 * who asked their system for less motion: the graph is simply there.
	 */
	playIntro(lines: readonly (readonly number[])[], duration = INTRO_MS) {
		const data = this.data;
		if (!data || prefersLessMotion()) return;
		// the camera is the one the graph was just fitted with (components/graph/graph-view.tsx): fitted again here, the
		// graph after the intro could end a fraction of a pixel away from one opened without it
		const plan = introPlan(data.points.length);
		this.hover(null);
		this.stopGliding();
		// Nothing is made ahead here: the glows and dots are made by the frames that first need them, as for a graph
		// without the intro, so that the graph after it is the same to the pixel.
		this.intro = {
			started: performance.now(),
			duration,
			plan,
			groupAlpha: new Float32Array(TWINKLE_GROUPS),
			constellations: packConstellations(lines.map((line) => line.filter((index) => data.points[index]))),
			pixels: null,
		};
		cancelAnimationFrame(this.frame);
		this.frame = 0;
		this.markDirty();
	}

	/** Whether the intro is playing. */
	get introPlaying(): boolean {
		return this.intro !== null;
	}

	/** End the intro where it would have ended: the usual graph, at once. */
	skipIntro() {
		if (!this.intro) return;
		this.endIntro();
		this.markDirty();
	}

	/** Nothing of the intro is kept once it is over: its layer's pixels are given back now. */
	private endIntro() {
		const pixels = this.intro?.pixels;
		if (pixels) pixels.canvas.width = pixels.canvas.height = 0;
		this.intro = null;
		if (this.labelLayer) this.labelLayer.canvas.width = this.labelLayer.canvas.height = 0;
		this.labelLayer = null;
	}

	private onSkip = () => this.skipIntro();

	private onSkipPress = () => {
		// a press that ends the intro still moves the graph or draws a lasso; its click opens nothing
		this.skipPress = this.intro !== null;
		this.skipIntro();
	};

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
		this.updateLook(); // which marked papers are shown, and how many, depends on the focus
		this.tellMarked();
		this.markDirty();
	}

	setSelected(index: number | null) {
		this.selected = index;
		if (this.inRegion) this.tellMarked(); // in the region, a selected marked paper is named
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
		this.glideTo(aim, atOnce, () => {
			this.fitted = { box, transform: this.transform };
			// the star field is where the loading sky had it while the camera is where the graph first came up
			if (whole && !this.origin) this.origin = { x: this.transform.x, y: this.transform.y };
		});
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

	/** The dots in the region look, as paths: the shown marked papers in their groups' colours, and one path for every
	 *  other paper, dimmed. Kept while the marks, the focus, the level, the colouring and the dots' size stay the same. */
	private regionDotsOf(level: CanvasLevel, data: CanvasData, radius: number) {
		const kept = this.regionDots;
		if (
			kept &&
			kept.level === level &&
			kept.focus === this.focus &&
			kept.marked === this.marked &&
			kept.colouring === this.colouring &&
			kept.radius === radius
		)
			return kept;
		// only the shown marked papers: every other paper is the usual dots filled grey (draw), whose paths are kept, and
		// are each a community's few hundred dots; one path of all the dimmed papers would take the slow way to draw
		const paint = this.paintOf(level);
		const dots = paint.hues.map(() => new Path2D());
		for (const index of this.shownMarked()) {
			const group = paint.of[index];
			if (group < 0) continue;
			const point = data.points[index];
			const size = radius * data.sizes[index];
			dots[group].moveTo(point.x + size, point.y);
			dots[group].arc(point.x, point.y, size, 0, Math.PI * 2);
		}
		this.regionDots = { level, focus: this.focus, marked: this.marked, colouring: this.colouring, radius, dots, dimmed: new Path2D() };
		return this.regionDots;
	}

	/** The shown marked papers' halos, drawn once into a layer at one zoom and kept while the marks, the focus and
	 *  about the zoom stay the same (lib/graph/mark-look.ts haloLayerFits); null when such a layer would be too large
	 *  (zoomed far in), and the halos in view are then drawn one by one. */
	private haloLayerOf(data: CanvasData, k: number): { canvas: HTMLCanvasElement; box: Box } | null {
		const kept = this.haloLayer;
		if (kept && kept.marked === this.marked && kept.focus === this.focus && kept.noLinks === this.noLinks && haloLayerFits(kept.k, k))
			return kept.layer;
		if (kept?.layer) kept.layer.canvas.width = kept.layer.canvas.height = 0;
		const reach = REGION_HALO_PX / k;
		const marked = this.shownMarked();
		const geometry = glowLayerGeometry(
			marked.map((index) => data.points[index]),
			reach,
			HALO_TEXELS,
			GLOW_LAYER_MOST_PIXELS / 4,
		);
		let layer: { canvas: HTMLCanvasElement; box: Box } | null = null;
		if (geometry) {
			const canvas = document.createElement("canvas");
			canvas.width = geometry.width;
			canvas.height = geometry.height;
			const halos = canvas.getContext("2d")!;
			const { box, scale } = geometry;
			halos.setTransform(scale, 0, 0, scale, -box.minX * scale, -box.minY * scale);
			halos.globalCompositeOperation = "lighter";
			const halo = this.markHaloOf();
			for (const index of marked) {
				const point = data.points[index];
				halos.drawImage(halo, point.x - reach, point.y - reach, reach * 2, reach * 2);
			}
			layer = { canvas, box };
		}
		this.haloLayer = { marked: this.marked, focus: this.focus, noLinks: this.noLinks, k, layer };
		return layer;
	}

	/** A marked paper's soft halo in the region look: pale yellow, faint, drawn additively, so that many marks close
	 *  together read as one lit region. Made once. */
	private markHaloOf(): HTMLCanvasElement {
		if (this.markHalo) return this.markHalo;
		const canvas = document.createElement("canvas");
		canvas.width = canvas.height = 64;
		const halo = canvas.getContext("2d")!;
		const gradient = halo.createRadialGradient(32, 32, 0, 32, 32, 32);
		gradient.addColorStop(0, "rgba(253, 236, 150, 0.34)");
		gradient.addColorStop(0.5, "rgba(253, 236, 150, 0.12)");
		gradient.addColorStop(1, "rgba(253, 236, 150, 0)");
		halo.fillStyle = gradient;
		halo.fillRect(0, 0, 64, 64);
		this.markHalo = canvas;
		return canvas;
	}

	/**
	 * The shown marked papers: their links (x1, y1, x2, y2 each), the dots their links end at (a path) and their rings
	 * (x, y and the ring's radius in world units before the few pixels it keeps around its dot); null with none shown.
	 * Kept while the marks, the focus and the dots' size stay the same, so a pan or a zoom does not make them again.
	 *
	 * The links and rings are stroked one by one (strokeMarks): a stroke wider than a device pixel of one path with
	 * thousands of lines takes the browser's slow way, about 55 ms a frame for 500 marked papers' links on a 1.5x
	 * screen, where the same lines one by one take under a millisecond.
	 */
	private marksOf(data: CanvasData, radius: number) {
		const kept = this.markPaths;
		if (kept && kept.marked === this.marked && kept.noLinks === this.noLinks && kept.focus === this.focus && kept.radius === radius)
			return kept.drawn;
		const drawn = this.markDrawing(this.shownMarked(), data, radius);
		this.markPaths = { marked: this.marked, noLinks: this.noLinks, focus: this.focus, radius, drawn };
		return drawn;
	}

	/** These marked papers' links, the dots their links end at and their rings; null for none. */
	private markDrawing(marked: number[], data: CanvasData, radius: number): MarkDrawing | null {
		if (marked.length === 0) return null;
		const links: number[] = [];
		const ends = new Path2D();
		const rings = new Float64Array(marked.length * 3);
		marked.forEach((index, order) => {
			const point = data.points[index];
			// a mark drawn without its links keeps its ring (and its name); hovering it still shows its links
			for (const other of this.noLinks.has(index) ? [] : this.linked[index]) {
				if (!this.lit(other)) continue;
				const end = data.points[other];
				links.push(point.x, point.y, end.x, end.y);
				const size = radius * data.sizes[other] * 1.1;
				ends.moveTo(end.x + size, end.y);
				ends.arc(end.x, end.y, size, 0, Math.PI * 2);
			}
			rings.set([point.x, point.y, radius * data.sizes[index] * 1.5], order * 3);
		});
		return { links: Float64Array.from(links), ends, rings };
	}

	/** Stroke each of `lines` (x1, y1, x2, y2 each) on its own, in the current stroke style. */
	private strokeLines(lines: Float64Array) {
		const { ctx } = this;
		for (let at = 0; at < lines.length; at += 4) {
			ctx.beginPath();
			ctx.moveTo(lines[at], lines[at + 1]);
			ctx.lineTo(lines[at + 2], lines[at + 3]);
			ctx.stroke();
		}
	}

	/** Stroke each of `rings` (x, y, radius each, plus `more` world units) on its own, in the current stroke style. */
	private strokeRings(rings: Float64Array, more: number) {
		const { ctx } = this;
		for (let at = 0; at < rings.length; at += 3) {
			ctx.beginPath();
			ctx.arc(rings[at], rings[at + 1], rings[at + 2] + more, 0, Math.PI * 2);
			ctx.stroke();
		}
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
		// Setting a canvas's size empties it, even to the size it has: only when it changed.
		const width = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
		const height = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
		if (this.canvas.width !== width) this.canvas.width = width;
		if (this.canvas.height !== height) this.canvas.height = height;
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
		// Drawn now, not in the next frame: the canvas was just emptied, and this runs between layout and paint
		// (a ResizeObserver's callback). Left to the next frame, every step of a panel being dragged wider or
		// narrower would paint the empty canvas first, and the graph would be gone for as long as the drag lasts.
		cancelAnimationFrame(this.frame);
		this.frame = 0;
		this.draw();
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
	 *  or not: the graph view picks which to name. Only the newest marks name their neighbours (MOST_LABELS), so only
	 *  theirs are told: with hundreds marked, the others' would be worked out on every move of the camera for nothing. */
	private tellMarked() {
		const points = this.data?.points;
		if (!points) return this.callbacks.onMarked([]);
		const { x: tx, y: ty, k } = this.transform;
		const place = (index: number) => ({ index, x: points[index].x * k + tx, y: points[index].y * k + ty });
		// in the region, only a marked paper hovered or selected is named, as it is shown in full
		const named = this.inRegion
			? this.shownMarked().filter((index) => index === this.hovered || index === this.selected)
			: this.shownMarked();
		this.callbacks.onMarked(
			named.map((index, order) => ({
				...place(index),
				// a mark drawn without its links names none of its neighbours either
				linked:
					order < MOST_LABELS.marked && !this.noLinks.has(index) ? this.linked[index].filter((other) => this.lit(other)).map(place) : [],
			})),
		);
	}

	private hover(index: number | null) {
		if (index === this.hovered) return;
		this.hovered = index;
		this.canvas.style.cursor = index === null ? "" : "pointer";
		this.tellHover();
		if (this.inRegion) this.tellMarked(); // a marked paper hovered in the region is named
		this.markDirty();
	}

	private onMove = (event: MouseEvent) => this.hover(this.intro ? null : this.pick(event));

	private onLeave = () => this.hover(null);

	// d3-zoom swallows the click that ends a drag, so this only fires for a real click.
	private onClick = (event: MouseEvent) => {
		if (this.skipPress || this.intro) return void (this.skipPress = false); // the press that ended the intro
		if (performance.now() - this.selectedAt < SAME_PRESS_MS) return; // the end of a lasso or of a select-click
		this.callbacks.onPick(this.pick(event));
	};

	/** Where a pointer is inside the canvas, in CSS pixels. */
	private placeOf(event: MouseEvent): Place {
		const rect = this.canvas.getBoundingClientRect();
		return { x: event.clientX - rect.left, y: event.clientY - rect.top };
	}

	// A press with the select key held starts a lasso (d3-zoom was told to leave it alone).
	private onPointerDown = (event: PointerEvent) => {
		if (event.button !== 0 || this.lasso || !selectKeyHeld(this.selectKey, event)) return;
		try {
			this.canvas.setPointerCapture(event.pointerId); // the lasso goes on when the pointer leaves the canvas
		} catch (error) {
			// a pointer the browser no longer knows: the lasso is still drawn while it stays over the canvas
		}
		this.stopGliding();
		this.lasso = { pointer: event.pointerId, line: [this.placeOf(event)] };
		this.canvas.style.cursor = "crosshair";
	};

	private onPointerMove = (event: PointerEvent) => {
		if (event.pointerId !== this.lasso?.pointer) return;
		this.lasso.line.push(this.placeOf(event));
		this.markDirty();
	};

	// Released: a press that hardly moved marks the paper under it; a lasso marks the papers in focus inside it.
	private onPointerUp = (event: PointerEvent) => {
		const lasso = this.endLasso(event);
		if (!lasso || !this.data) return;
		this.selectedAt = performance.now();
		if (reach(lasso.line) < LASSO_MIN) {
			if (this.skipPress) return; // the press that ended the intro marks nothing
			const index = this.pick(event);
			if (index !== null) this.callbacks.onMark(index);
			return;
		}
		const places = this.data.points.map((point) => {
			const [x, y] = this.transform.apply([point.x, point.y]);
			return { x, y };
		});
		const found = lassoed(lasso.line, places, (index) => this.lit(index));
		if (found.length > 0) this.callbacks.onMarkMany(found);
	};

	private onPointerCancel = (event: PointerEvent) => void this.endLasso(event);

	/** Stop drawing the lasso this pointer was drawing, and hand it back; null when it was drawing none. */
	private endLasso(event: PointerEvent) {
		const lasso = this.lasso;
		if (event.pointerId !== lasso?.pointer) return null;
		this.lasso = null;
		if (this.canvas.hasPointerCapture(event.pointerId)) this.canvas.releasePointerCapture(event.pointerId);
		this.canvas.style.cursor = "";
		this.markDirty();
		return lasso;
	}

	// Right-click marks a paper. The browser's own menu has nothing to offer on the graph, so it never opens here.
	private onContextMenu = (event: MouseEvent) => {
		event.preventDefault();
		if (this.skipPress || this.intro) return void (this.skipPress = false); // the press that ended the intro
		const index = this.pick(event);
		if (index !== null) this.callbacks.onMark(index);
	};

	/** The intro's frame now, or null when it is not playing (or has just ended: this frame is the usual graph). */
	private introNow(): IntroFrame | null {
		const intro = this.intro;
		if (!intro) return null;
		const p = introProgress(performance.now() - intro.started, intro.duration);
		if (p >= 1) {
			this.endIntro();
			return null;
		}
		return introFrame(p);
	}

	/** The layer the intro fades the labels in with, as large as the canvas and empty: made once per intro. */
	private labelLayerOf(): CanvasRenderingContext2D | null {
		const { width, height } = this.canvas;
		let layer = this.labelLayer;
		if (!layer) {
			layer = document.createElement("canvas").getContext("2d");
			if (!layer) return null;
			this.labelLayer = layer;
		}
		if (layer.canvas.width !== width) layer.canvas.width = width;
		if (layer.canvas.height !== height) layer.canvas.height = height;
		layer.setTransform(1, 0, 0, 1, 0, 0);
		layer.clearRect(0, 0, width, height);
		return layer;
	}

	/** One tile of the star field with its first `count` stars, drawn once for this device pixel ratio and kept. */
	private starTileOf(count: number): CanvasPattern | null {
		const { dpr } = this;
		if (this.starTile?.count === count && this.starTile.dpr === dpr) return this.starTile.pattern;
		const side = Math.round(FIELD_TILE * dpr);
		const canvas = this.starTile?.canvas ?? document.createElement("canvas");
		canvas.width = canvas.height = side;
		const tile = canvas.getContext("2d");
		if (!tile) return null;
		tile.fillStyle = STAR;
		for (let index = 0; index < count; index++) {
			const size = FIELD.size[index] * dpr;
			tile.globalAlpha = FIELD.bright[index];
			tile.fillRect(FIELD.place[index * 2] * side - size / 2, FIELD.place[index * 2 + 1] * side - size / 2, size, size);
		}
		const pattern = this.ctx.createPattern(canvas, "repeat");
		if (!pattern) return null;
		this.starTile = { count, dpr, canvas, pattern };
		return pattern;
	}

	/** The star field behind everything (lib/graph/sky.ts), `count` stars a tile, as strong as `alpha`: one repeated
	 *  image, in screen space, moved a small share of the camera's pan since the graph was first shown. */
	private drawStarField(count: number, alpha: number) {
		if (count <= 0 || alpha <= 0) return;
		const pattern = this.starTileOf(count);
		if (!pattern) return;
		const { ctx, dpr } = this;
		const { x, y } = this.transform;
		const offset = fieldOffset(x - (this.origin?.x ?? x), y - (this.origin?.y ?? y));
		pattern.setTransform(new DOMMatrix([1, 0, 0, 1, offset.x * dpr, offset.y * dpr]));
		ctx.setTransform(1, 0, 0, 1, 0, 0);
		ctx.globalAlpha = alpha;
		ctx.fillStyle = pattern;
		ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
		ctx.globalAlpha = 1;
	}

	/** How many stars behind the graph: none, a few or many (Settings, "Background stars"). */
	setStars(stars: StarField) {
		if (stars === this.stars) return;
		this.stars = stars;
		this.markDirty();
	}

	/** The pixels the gathering stars are written into: as large as the canvas, made once and kept while it keeps
	 *  its size. */
	private starPixelsOf(intro: Intro) {
		const { width, height } = this.canvas;
		const known = intro.pixels;
		if (known && known.image.width === width && known.image.height === height) return known;
		if (known) known.canvas.width = known.canvas.height = 0;
		const canvas = document.createElement("canvas");
		canvas.width = width;
		canvas.height = height;
		const context = canvas.getContext("2d");
		if (!context) return null;
		const image = context.createImageData(width, height);
		intro.pixels = { canvas, context, image, words: new Uint32Array(image.data.buffer) };
		return intro.pixels;
	}

	/**
	 * The intro's papers on their way from where they started (somewhere in view) to their places: pale squares
	 * written straight into pixels, then drawn as one image. Thousands of stars move every frame, and one canvas call
	 * for each would cost more than a frame has.
	 */
	private drawGathering(intro: Intro, frame: IntroFrame, data: CanvasData, radius: number, now: number) {
		const pixels = this.starPixelsOf(intro);
		if (!pixels) return;
		const { ctx, dpr } = this;
		const { x: tx, y: ty, k } = this.transform;
		const { gathered } = frame;
		// a slight twinkle while they appear, steady once they are on their way
		const groupAlpha = intro.groupAlpha;
		for (let group = 0; group < TWINKLE_GROUPS; group++)
			groupAlpha[group] = frame.stars * (1 - (1 - gathered) * 0.25 * (0.5 + 0.5 * Math.sin(now / 260 + group * 1.7)));
		// a star's side in device pixels, before its paper's own size
		const side = radius * STAR_SIDE * k * dpr;
		paintGathering(
			{ words: pixels.words, width: pixels.image.width, height: pixels.image.height },
			intro.plan,
			data.points,
			data.sizes,
			{ x: tx, y: ty, k, dpr },
			side,
			gathered,
			groupAlpha,
			STAR_WORD,
		);
		pixels.context.putImageData(pixels.image, 0, 0);
		ctx.setTransform(1, 0, 0, 1, 0, 0);
		ctx.globalAlpha = 1; // each star's own strength is in its pixels
		ctx.drawImage(pixels.canvas, 0, 0);
		ctx.setTransform(dpr * k, 0, 0, dpr * k, dpr * tx, dpr * ty);
	}

	/** The constellations traced so far: one line per community, and a white dot on each paper it has reached. */
	private drawConstellations(intro: Intro, frame: IntroFrame, data: CanvasData, radius: number) {
		const { ctx } = this;
		const { k } = this.transform;
		const { papers, offsets } = intro.constellations;
		const { points } = data;
		const line = new Path2D();
		const dots = new Path2D();
		const dot = radius * 1.7;
		for (let community = 0; community + 1 < offsets.length; community++) {
			const first = offsets[community];
			const count = offsets[community + 1] - first;
			if (count === 0) continue;
			const { whole, part } = tracedTo(frame.trace, count - 1);
			const begin = points[papers[first]];
			line.moveTo(begin.x, begin.y);
			for (let step = 0; step <= whole && step < count; step++) {
				const point = points[papers[first + step]];
				if (step > 0) line.lineTo(point.x, point.y);
				dots.moveTo(point.x + dot, point.y);
				dots.arc(point.x, point.y, dot, 0, Math.PI * 2);
			}
			if (part > 0 && whole + 1 < count) {
				const from = points[papers[first + whole]];
				const to = points[papers[first + whole + 1]];
				line.lineTo(from.x + (to.x - from.x) * part, from.y + (to.y - from.y) * part);
			}
		}
		ctx.globalAlpha = frame.line;
		ctx.strokeStyle = STAR_LINE;
		ctx.lineWidth = 1 / k;
		ctx.stroke(line);
		ctx.globalAlpha = Math.min(1, frame.line / LINE_ALPHA) * 0.9;
		ctx.fillStyle = STAR_LINE;
		ctx.fill(dots);
		ctx.globalAlpha = 1;
	}

	private draw() {
		if (this.cameraMoved) {
			this.cameraMoved = false;
			this.tellHover();
			this.tellMarked();
		}
		const { ctx, data, dpr } = this;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, this.canvas.clientWidth, this.canvas.clientHeight);
		if (!data) return;
		const intro = this.intro;
		const frame = this.introNow();
		const now = performance.now();
		const { x: tx, y: ty, k } = this.transform;
		const level = data.levels[Math.min(this.level, data.levels.length - 1)];
		const groupOf = (index: number) => level.groups[level.of[index]];
		// how strongly each of the usual layers is drawn: in full, unless the intro is bringing it up
		const lit = frame ? frame.light : 1;
		const named = frame ? frame.names : 1;
		// how far the marked papers are drawn as a highlighted region rather than rings (lib/graph/mark-look.ts)
		const region = this.lookNow(now);
		const changingLook = region !== this.look.target;

		// 0. The star field, behind everything (drawn first, the glows add over it). In the intro it comes up with the
		// stars and stays as chosen in Settings; with none chosen, it is the intro's dust and goes as the communities light.
		if (frame) this.drawStarField(skyStarCount(this.stars), this.stars === "off" ? frame.dust : frame.stars);
		else this.drawStarField(FIELD_STARS[this.stars], 1);
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

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
		// turned down while the marked papers are a region, so that the region stands out
		ctx.globalAlpha = lit * (1 - (1 - REGION_GLOW) * region);
		const layer = this.glowLayerOf(level, data);
		if (lit <= 0) {
			// the intro's communities have not lit up yet
		} else if (layer) {
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
		// the marked papers, the hovered and the selected one come up with the names
		ctx.globalAlpha = named;

		// 1b. Links, under the papers: a line runs from a paper's centre to another's and crosses whatever lies between,
		// so every dot is drawn over it. The marked papers' links (those in focus), then the hovered paper's.
		const radius = dotRadius(k);
		// every shown mark's links and rings, giving way to the region; in the region, those of a marked paper hovered or
		// selected, which is shown in full
		const marks = region < 1 ? this.marksOf(data, radius) : null;
		const full = region > 0 ? this.markDrawing(this.markedInFull(), data, radius) : null;
		ctx.strokeStyle = MARK_LINK;
		ctx.lineWidth = 1 / k;
		if (marks) {
			ctx.globalAlpha = named * (1 - region);
			this.strokeLines(marks.links);
		}
		if (full) {
			ctx.globalAlpha = named * region;
			this.strokeLines(full.links);
		}
		ctx.globalAlpha = named;
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
		// In the intro, pale stars on their way to their places, then the same dots as always, pale and then in their
		// colours.
		if (frame && intro && !frame.settled) this.drawGathering(intro, frame, data, radius, now);
		else {
			const { dots, dimmed } = this.dotsOf(level, data, radius);
			const { hues } = this.paintOf(level);
			if (frame && lit < 1) {
				ctx.globalAlpha = frame.stars * (1 - lit);
				ctx.fillStyle = STAR;
				ctx.fill(dimmed);
				for (const path of dots) ctx.fill(path);
			}
			const fill = (paths: { dots: Path2D[]; dimmed: Path2D }, alpha: number) => {
				ctx.globalAlpha = alpha;
				ctx.fillStyle = DIM;
				ctx.fill(paths.dimmed);
				paths.dots.forEach((path, group) => {
					const hue = hues[group];
					ctx.fillStyle = hue === null ? GREY : hsl(hue, 74);
					ctx.fill(path);
				});
			};
			// the usual dots, and the region's (only the shown marked papers in colour), the one giving way to the other
			if (lit > 0 && region < 1) fill({ dots, dimmed }, lit * (1 - region));
			if (lit > 0 && region > 0) {
				// every paper dimmed, as out of focus, then the shown marked papers over them in their colours
				ctx.globalAlpha = lit * region;
				ctx.fillStyle = DIM;
				ctx.fill(dimmed);
				for (const path of dots) ctx.fill(path);
				fill(this.regionDotsOf(level, data, radius), lit * region);
				// each shown marked paper's soft halo, a few pixels across whatever the zoom: one image while it can be kept,
				// else those in view one by one
				ctx.globalCompositeOperation = "lighter";
				ctx.globalAlpha = lit * region;
				const halos = this.haloLayerOf(data, k);
				if (halos)
					ctx.drawImage(halos.canvas, halos.box.minX, halos.box.minY, halos.box.maxX - halos.box.minX, halos.box.maxY - halos.box.minY);
				else {
					const halo = this.markHaloOf();
					const reach = REGION_HALO_PX / k;
					const view = visibleWorld(this.transform, this.canvas.clientWidth, this.canvas.clientHeight, reach);
					for (const index of this.shownMarked()) {
						const point = data.points[index];
						if (inside(view, point.x, point.y)) ctx.drawImage(halo, point.x - reach, point.y - reach, reach * 2, reach * 2);
					}
				}
				ctx.globalCompositeOperation = "source-over";
			}
		}
		// 2b. The intro's constellations, over the papers they go through.
		if (frame && intro && frame.trace > 0 && frame.line > 0) this.drawConstellations(intro, frame, data, radius);
		ctx.globalAlpha = named;

		// 3. Marked papers (those in focus): the papers their links (1b) end at, then a yellow ring on a dark rim. The
		// ring sits outside where the selected paper's ring goes, so a paper that is both shows both.
		for (const [drawing, alpha] of [
			[marks, named * (1 - region)],
			[full, named * region],
		] as const) {
			if (!drawing || alpha <= 0) continue;
			ctx.globalAlpha = alpha;
			ctx.fillStyle = MARK_LINK;
			ctx.fill(drawing.ends);
			ctx.strokeStyle = MARK_EDGE;
			ctx.lineWidth = 4 / k;
			this.strokeRings(drawing.rings, 7 / k);
			ctx.strokeStyle = MARK;
			ctx.lineWidth = 2 / k;
			this.strokeRings(drawing.rings, 7 / k);
		}
		ctx.globalAlpha = named;

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

		// 5. Labels of the groups that have a paper in focus, in screen space so they stay one size. While the intro
		// fades them in, they are drawn whole into a layer of their own and the layer is faded: a label drawn faint
		// would show its dark outline through its own letters.
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		const fading = named > 0 && named < 1 ? this.labelLayerOf() : null;
		const pen = fading ?? ctx;
		pen.setTransform(dpr, 0, 0, dpr, 0, 0);
		const shown = new Set<number>();
		if (this.focus) for (const index of this.focus) shown.add(level.of[index]);
		pen.font = '600 11px "JetBrains Mono", ui-monospace, monospace';
		pen.textAlign = "center";
		pen.textBaseline = "middle";
		pen.lineJoin = "round";
		pen.lineWidth = 3;
		// Biggest groups first; a label that would cover one already placed waits until there is room (zoom in).
		const placed: { left: number; right: number; top: number; bottom: number }[] = [];
		const order = level.groups.map((_, index) => index).sort((a, b) => level.groups[b].size - level.groups[a].size);
		for (const index of order) {
			if (named <= 0) break; // the intro has not come to the names
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
			const half = Math.max(...lines.map((line) => pen.measureText(line).width)) / 2 + 4;
			const tall = (lines.length * LABEL_LINE) / 2 + 2;
			const box = { left: x - half, right: x + half, top: y - tall, bottom: y + tall };
			const covers = (other: typeof box) =>
				box.left < other.right && box.right > other.left && box.top < other.bottom && box.bottom > other.top;
			if (placed.some(covers)) continue;
			placed.push(box);
			lines.forEach((line, row) => {
				const lineY = y + (row - (lines.length - 1) / 2) * LABEL_LINE;
				pen.strokeStyle = "rgba(20, 20, 20, 0.85)";
				pen.strokeText(line, x, lineY);
				pen.fillStyle = hsl(group.hue, 86);
				pen.fillText(line, x, lineY);
			});
		}
		if (fading) {
			ctx.setTransform(1, 0, 0, 1, 0, 0);
			ctx.globalAlpha = named;
			ctx.drawImage(fading.canvas, 0, 0);
			ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		}

		ctx.globalAlpha = 1;
		// the intro's next frame; its last one was the usual graph, drawn just now
		if (frame || changingLook) this.markDirty();

		// 6. The lasso being drawn, in screen space: the line the pointer took, closed back to where it began.
		if (this.lasso && this.lasso.line.length > 1) {
			const [start, ...rest] = this.lasso.line;
			ctx.beginPath();
			ctx.moveTo(start.x, start.y);
			for (const place of rest) ctx.lineTo(place.x, place.y);
			ctx.closePath();
			ctx.fillStyle = LASSO_FILL;
			ctx.fill("evenodd");
			ctx.strokeStyle = MARK;
			ctx.lineWidth = 1.5;
			ctx.setLineDash([5, 4]);
			ctx.stroke();
			ctx.setLineDash([]);
		}
	}
}
