/**
 * The sky shown while a run is loaded or made (components/graph/sky-loading.tsx): stars that twinkle slowly, and a
 * small constellation near the middle whose lines draw themselves one after another, hold, fade, and give way to
 * the next. One figure takes LOOP_MS; the loop goes on for as long as the sky is shown.
 *
 * While a run is being made, the share of the stars that are lit is how far the run has got, where the engine says
 * so; with no number to go by, every star is lit and nothing pretends to move on.
 */
import type { RunStep } from "@rp/contracts";
import { easeOut, unitHash } from "@/lib/graph/intro";

/** The stars' pale white-blue: the loading sky's, the intro's (components/graph/graph-canvas.ts) and the dots after
 *  the words shown over the sky (components/ui/loading-dots.tsx). */
export const STAR_COLOUR = "rgb(214, 226, 255)";

/** How long one constellation takes, from its first line to the next figure's. */
export const LOOP_MS = 6000;

/** Within one figure's time: its lines are drawn, held, faded, and then the sky is empty for a moment. */
const DRAW = 0.5;
const HOLD = 0.72;
const FADE = 0.9;

/** A figure: its stars, in a square from -1 to 1 (y down), and its lines as pairs of stars, in the order they are drawn. */
export interface Figure {
	stars: readonly (readonly [number, number])[];
	lines: readonly (readonly [number, number])[];
}

/** Shapes after a few constellations of the northern sky, each of 6 to 8 stars. */
export const FIGURES: readonly Figure[] = [
	// a plough
	{
		stars: [
			[-1, -0.35],
			[-0.62, -0.42],
			[-0.28, -0.3],
			[0.05, -0.12],
			[0.12, 0.32],
			[0.72, 0.42],
			[0.78, -0.02],
		],
		lines: [
			[0, 1],
			[1, 2],
			[2, 3],
			[3, 4],
			[4, 5],
			[5, 6],
			[6, 3],
		],
	},
	// a swan
	{
		stars: [
			[0, -0.95],
			[0, -0.45],
			[0, 0.05],
			[0, 0.62],
			[-0.62, -0.05],
			[-1, 0.28],
			[0.62, 0.1],
			[0.95, 0.45],
		],
		lines: [
			[0, 1],
			[1, 2],
			[2, 3],
			[2, 4],
			[4, 5],
			[2, 6],
			[6, 7],
		],
	},
	// a crown
	{
		stars: [
			[-0.95, -0.2],
			[-0.72, 0.25],
			[-0.32, 0.5],
			[0.12, 0.55],
			[0.52, 0.35],
			[0.82, 0],
		],
		lines: [
			[0, 1],
			[1, 2],
			[2, 3],
			[3, 4],
			[4, 5],
		],
	},
	// a hunter
	{
		stars: [
			[-0.45, -0.85],
			[0.42, -0.7],
			[-0.14, -0.05],
			[0.02, 0],
			[0.18, 0.05],
			[-0.52, 0.82],
			[0.55, 0.75],
		],
		lines: [
			[0, 2],
			[2, 3],
			[3, 4],
			[4, 1],
			[2, 5],
			[4, 6],
		],
	},
	// a W
	{
		stars: [
			[-1, -0.3],
			[-0.5, 0.3],
			[0, -0.05],
			[0.48, 0.4],
			[0.95, -0.25],
			[0.3, -0.55],
		],
		lines: [
			[0, 1],
			[1, 2],
			[2, 3],
			[3, 4],
			[2, 5],
		],
	},
];

/**
 * The star field: tiny stars on one square tile, repeated in every direction, in screen space (CSS pixels). It is
 * the loading sky's stars, the intro's dust and the stars behind the graph (components/graph/graph-canvas.ts), so
 * one hands over to the next without a star moving. Behind the graph it moves a little with the camera (PARALLAX),
 * never scales, and is drawn as one image repeated, never star by star.
 */
export type StarField = "off" | "few" | "many";

export const STAR_FIELDS: readonly StarField[] = ["off", "few", "many"];

/** The side of the field's tile, in CSS pixels. */
export const FIELD_TILE = 512;
/** Stars on one tile, for each choice. "off" still has the few the loading sky and the intro show. */
export const FIELD_STARS: Record<StarField, number> = { off: 0, few: 34, many: 90 };
/** How far the field moves for the camera's pan: a small share, so the graph moves over a sky that barely moves. */
export const PARALLAX = 0.04;

/** The stars of one tile: where each is on it (0 to 1), how large (CSS pixels across) and how bright. Star i is the
 *  same at every level, and a level with more stars only adds stars, so changing it moves none. */
export interface Field {
	place: Float32Array;
	size: Float32Array;
	bright: Float32Array;
	/** where each star is in its slow twinkle, 0 to 1 (only the loading sky twinkles) */
	phase: Float32Array;
}

export function fieldStars(count: number): Field {
	const place = new Float32Array(count * 2);
	const size = new Float32Array(count);
	const bright = new Float32Array(count);
	const phase = new Float32Array(count);
	for (let index = 0; index < count; index++) {
		place[index * 2] = unitHash(index, 11);
		place[index * 2 + 1] = unitHash(index, 12);
		const shine = unitHash(index, 13);
		// most faint and small, a few larger and brighter
		size[index] = 0.5 + 0.9 * shine * shine;
		bright[index] = shine > 0.93 ? 0.7 : 0.16 + 0.34 * shine;
		phase[index] = unitHash(index, 14);
	}
	return { place, size, bright, phase };
}

/** Where the field's tiles begin, in CSS pixels from the canvas's corner (0 to FIELD_TILE), for a camera panned
 *  (dx, dy) from where it was when the graph was first shown: then, the field is where the loading sky had it. */
export function fieldOffset(dx: number, dy: number): { x: number; y: number } {
	const wrap = (value: number) => ((value % FIELD_TILE) + FIELD_TILE) % FIELD_TILE;
	return { x: wrap(dx * PARALLAX), y: wrap(dy * PARALLAX) };
}

/** The tiles a box of this size needs, from a tile beginning at `offset`: their corners, in CSS pixels. */
export function tilesCovering(width: number, height: number, offset: { x: number; y: number }): { x: number; y: number }[] {
	const tiles: { x: number; y: number }[] = [];
	const left = offset.x > 0 ? offset.x - FIELD_TILE : 0;
	const top = offset.y > 0 ? offset.y - FIELD_TILE : 0;
	for (let y = top; y < height; y += FIELD_TILE) for (let x = left; x < width; x += FIELD_TILE) tiles.push({ x, y });
	return tiles;
}

/** How many stars the loading sky and the intro show for a choice: the choice's own, or a few when it is off. */
export function skyStarCount(field: StarField): number {
	return field === "off" ? FIELD_STARS.few : FIELD_STARS[field];
}

/** How bright a star is at `ms`: it breathes slowly, between about two thirds of its brightness and all of it. */
export function twinkle(bright: number, phase: number, ms: number): number {
	const turn = ms / 4200 + phase;
	return bright * (0.82 + 0.18 * Math.sin(turn * Math.PI * 2));
}

/** What the constellation is doing at `ms`. */
export interface SkyFigure {
	/** which of FIGURES */
	figure: number;
	/** lines drawn whole, and the share drawn of the next */
	whole: number;
	part: number;
	/** how strong the figure is (it fades before the next) */
	alpha: number;
}

export function skyFigure(ms: number, still = false): SkyFigure {
	if (still) return { figure: 0, whole: FIGURES[0].lines.length, part: 0, alpha: 1 };
	const loops = Math.max(0, ms) / LOOP_MS;
	const figure = Math.floor(loops) % FIGURES.length;
	const within = loops - Math.floor(loops);
	const { length } = FIGURES[figure].lines;
	const drawn = Math.min(1, within / DRAW) * length;
	const whole = Math.min(length, Math.floor(drawn));
	const part = whole >= length ? 0 : easeOut(drawn - whole);
	const alpha = within < HOLD ? 1 : within < FADE ? 1 - (within - HOLD) / (FADE - HOLD) : 0;
	return { figure, whole, part, alpha };
}

/** Where a figure goes in a sky of this size: its middle and the length of its unit, in CSS pixels. Above the middle,
 *  so that the words shown over the sky stay clear of it. */
export function figureFrame(width: number, height: number): { x: number; y: number; unit: number } {
	const unit = Math.min(110, Math.max(28, Math.min(width, height) * 0.14));
	return { x: width / 2, y: Math.max(unit + 8, height / 2 - unit * 1.6), unit };
}

/**
 * How far a run being made has got, from 0 to 1, as the engine reports it: the steps done, and the part of the step
 * under way that its count says is done. A step that does not count adds nothing until it ends. Null when the engine
 * says nothing (no run is being made, or it has not said where it is): then nothing is shown as progress.
 */
export function runShare(step: RunStep | null | undefined): number | null {
	if (!step || !(step.of > 0) || !(step.number >= 1)) return null;
	const count = step.count;
	const within = count && count.of > 0 ? Math.min(1, Math.max(0, count.done / count.of)) : 0;
	return Math.min(1, Math.max(0, (step.number - 1 + within) / step.of));
}

/** Whether star `index` of the field is lit for a share of the run done (null: every star is). The stars that light
 *  first are scattered over the sky, and about that share of them is lit. */
export function starLit(index: number, share: number | null): boolean {
	return share === null || unitHash(index, 15) < share;
}
