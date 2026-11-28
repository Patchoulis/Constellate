/**
 * The constellation intro: what the graph shows in the seconds after a run is made, before it is the usual graph.
 *
 * Every paper starts as a pale star somewhere in the view, the stars gather to their papers' places, a constellation
 * is traced through a few papers of each community, the communities take their colours and glows, and their names
 * come last. With progress `p` from 0 to 1 the beats overlap (each eased, slow at its end):
 *
 *     stars appear         0.00 - 0.18
 *     papers gather        0.14 - 0.55   (each paper a little later than the last, up to a quarter of the beat)
 *     constellations       0.50 - 0.72
 *     communities light    0.68 - 0.92
 *     names                0.82 - 1.00
 *
 * This module is the time and the places; the renderer (components/graph/graph-canvas.ts) only draws what it says.
 * At p = 1 everything here is at its end and the renderer draws the usual graph.
 */
import type { Point } from "@/lib/graph/layout";

/** How long the intro lasts, by default. */
export const INTRO_MS = 4500;

export type Beat = "stars" | "gather" | "trace" | "light" | "names";

/** Where each beat begins and ends, in progress from 0 to 1. */
export const BEATS: Record<Beat, readonly [number, number]> = {
	stars: [0, 0.18],
	gather: [0.14, 0.55],
	trace: [0.5, 0.72],
	light: [0.68, 0.92],
	names: [0.82, 1],
};

/** The most a paper's gathering is held back, as a share of the beat. */
export const MOST_DELAY = 0.25;

/** How strong the traced lines are while they are drawn (white), and the share of it they keep once the communities
 *  have lit up, before they go. */
export const LINE_ALPHA = 0.55;
export const LINE_KEPT = 0.25;

/** Quick at the start, slow at the end; 0 at 0 and 1 at 1. */
export function easeOut(t: number): number {
	const clamped = Math.min(1, Math.max(0, t));
	return 1 - (1 - clamped) ** 3;
}

/** The intro's progress, 0 to 1, `elapsed` milliseconds after it began. */
export function introProgress(elapsed: number, duration = INTRO_MS): number {
	if (!(duration > 0)) return 1;
	return Math.min(1, Math.max(0, elapsed / duration));
}

/** How far a beat has got at progress `p`, eased: 0 before it begins, 1 once it has ended. */
export function beat(p: number, which: Beat): number {
	const [from, to] = BEATS[which];
	return easeOut((p - from) / (to - from));
}

/** Every beat at progress `p`, and how strong the parts that come and go are. */
export interface IntroFrame {
	p: number;
	stars: number;
	trace: number;
	light: number;
	names: number;
	/** the raw (not eased) share of the gathering beat that has passed: each paper eases its own part of it */
	gathered: number;
	/** whether every paper is at its place */
	settled: boolean;
	/** the traced lines' opacity */
	line: number;
	/** the background dust's opacity */
	dust: number;
}

export function introFrame(p: number): IntroFrame {
	const [from, to] = BEATS.gather;
	const gathered = Math.min(1, Math.max(0, (p - from) / (to - from)));
	const light = beat(p, "light");
	const names = beat(p, "names");
	const stars = beat(p, "stars");
	return {
		p,
		stars,
		trace: beat(p, "trace"),
		light,
		names,
		gathered,
		settled: gathered >= 1,
		// drawn at LINE_ALPHA, down to a quarter as the communities light, and gone with the names
		line: p >= 1 ? 0 : LINE_ALPHA * (1 - (1 - LINE_KEPT) * light) * (1 - names),
		dust: p >= 1 ? 0 : stars * (1 - light),
	};
}

/** A number from 0 to 1 that is always the same for the same `index` and `salt` (a small integer hash). */
export function unitHash(index: number, salt = 0): number {
	let h = (Math.imul(index | 0, 0x9e3779b1) ^ Math.imul(salt | 0, 0x85ebca77)) | 0;
	h = Math.imul(h ^ (h >>> 16), 0x7feb352d);
	h = Math.imul(h ^ (h >>> 15), 0x846ca68b);
	h ^= h >>> 16;
	return (h >>> 0) / 4294967296;
}

/** What the intro works out once per graph, so that no frame works anything out again. */
export interface IntroPlan {
	/** where each paper starts, as a share of the view's width and height: [u0, v0, u1, v1, ...] */
	start: Float32Array;
	/** how long each paper waits before it gathers, as a share of the beat (0 to MOST_DELAY) */
	delay: Float32Array;
	/** how each paper twinkles: a phase from 0 to 1 */
	twinkle: Float32Array;
}

export function introPlan(papers: number): IntroPlan {
	const start = new Float32Array(papers * 2);
	const delay = new Float32Array(papers);
	const twinkle = new Float32Array(papers);
	for (let index = 0; index < papers; index++) {
		// a little inside the view's edges, so that no star starts cut in half
		start[index * 2] = 0.03 + 0.94 * unitHash(index, 1);
		start[index * 2 + 1] = 0.03 + 0.94 * unitHash(index, 2);
		delay[index] = MOST_DELAY * unitHash(index, 3);
		twinkle[index] = unitHash(index, 4);
	}
	return { start, delay, twinkle };
}

/** How far one paper has gone from its start to its place (0 to 1, eased), `gathered` of the way through the beat. */
export function paperGathered(gathered: number, delay: number): number {
	if (gathered >= 1) return 1;
	return easeOut((gathered - delay) / (1 - delay));
}

/** The papers twinkle in this many groups, each in its own time. */
export const TWINKLE_GROUPS = 4;

/** Which twinkle group a paper is in. */
export function twinkleGroup(plan: IntroPlan, index: number): number {
	return Math.min(TWINKLE_GROUPS - 1, Math.floor(plan.twinkle[index] * TWINKLE_GROUPS));
}

/** Pixels the gathering stars are written into, one 32-bit word per pixel (red in the low byte), as large as the
 *  canvas they are drawn on: made again whenever the canvas's size in device pixels changes. */
export interface StarPixels {
	words: Uint32Array;
	width: number;
	height: number;
}

/** The camera, as the renderer has it: a world point (wx, wy) is at ((wx * k + x) * dpr, (wy * k + y) * dpr) in device
 *  pixels. */
export interface StarCamera {
	x: number;
	y: number;
	k: number;
	dpr: number;
}

/**
 * Write every paper's star into `pixels`, `gathered` of the way through the gathering beat: from where it started
 * (its share of the pixels' width and height) to its place under the camera. `side` is a star's side in device pixels
 * before its paper's own size; a star smaller than a pixel is one pixel, fainter by its area. `groupAlpha` is how
 * strong each twinkle group is, `colour` the stars' red, green and blue (red in the low byte). Clears the pixels
 * first. Answers how many stars are inside them.
 */
export function paintGathering(
	pixels: StarPixels,
	plan: IntroPlan,
	points: ArrayLike<Point>,
	sizes: ArrayLike<number>,
	camera: StarCamera,
	side: number,
	gathered: number,
	groupAlpha: ArrayLike<number>,
	colour: number,
): number {
	const { words, width, height } = pixels;
	words.fill(0);
	const { start, delay } = plan;
	const { x: tx, y: ty, k, dpr } = camera;
	let inside = 0;
	for (let index = 0; index < points.length; index++) {
		const along = paperGathered(gathered, delay[index]);
		const point = points[index];
		const sx = start[index * 2] * width;
		const sy = start[index * 2 + 1] * height;
		const x = sx + ((tx + point.x * k) * dpr - sx) * along;
		const y = sy + ((ty + point.y * k) * dpr - sy) * along;
		const size = side * sizes[index];
		const cells = size < 1 ? 1 : Math.round(size);
		const strength = (size < 1 ? size * size : 1) * groupAlpha[twinkleGroup(plan, index)];
		const word = ((Math.round(Math.min(1, Math.max(0, strength)) * 255) << 24) | colour) >>> 0;
		const left = Math.round(x - cells / 2);
		const top = Math.round(y - cells / 2);
		let drawn = false;
		for (let row = Math.max(0, top); row < Math.min(height, top + cells); row++)
			for (let column = Math.max(0, left); column < Math.min(width, left + cells); column++) {
				words[row * width + column] = word;
				drawn = true;
			}
		if (drawn) inside++;
	}
	return inside;
}

/** Where a paper is drawn: `along` (paperGathered) of the way from where it started to its place. */
export function paperPlace(start: Point, place: Point, along: number): Point {
	return { x: start.x + (place.x - start.x) * along, y: start.y + (place.y - start.y) * along };
}

/** How much of a traced line of `segments` segments is drawn at `trace` (0 to 1): the segments drawn whole, and the
 *  share of the next one. */
export function tracedTo(trace: number, segments: number): { whole: number; part: number } {
	if (segments <= 0) return { whole: 0, part: 0 };
	const drawn = Math.min(1, Math.max(0, trace)) * segments;
	const whole = Math.min(segments, Math.floor(drawn));
	return { whole, part: whole >= segments ? 0 : drawn - whole };
}

/** How many papers a community's constellation goes through: more for a larger community, 5 to 7, never more papers
 *  than it has. */
export function constellationSize(papers: number): number {
	const size = papers < 60 ? 5 : papers < 400 ? 6 : 7;
	return Math.min(size, papers);
}

/** How many of a community's best-linked papers its constellation may go through, besides those nearest its centre. */
const HUB_POOL = 24;

/**
 * The papers of one community a constellation goes through, in the order its line is drawn.
 *
 * It starts at the community's most central paper (the first of `central`, its papers nearest the community's mean,
 * nearest first) and, from each paper, goes to the closest paper on screen that is not too near it (at least
 * `apart`, a share of the community's radius), among the papers nearest the centre and the papers with the most
 * links. The same community always gives the same figure: ties go to the lower index.
 */
export function constellationPicks(
	members: readonly number[],
	points: readonly Point[],
	central: readonly number[],
	links: ArrayLike<number>,
	radius: number,
	count = constellationSize(members.length),
	apart = 0.28,
): number[] {
	if (members.length === 0 || count <= 0) return [];
	const inside = new Set(members);
	const hubs = [...members].sort((a, b) => links[b] - links[a] || a - b).slice(0, HUB_POOL);
	const pool = [...new Set([...central.filter((index) => inside.has(index)), ...hubs])];
	// no paper is known to be central: the best-linked one starts
	const first = pool[0];
	const picks = [first];
	const taken = new Set(picks);
	const near = (radius * apart) ** 2;
	const gap = (a: number, b: number) => (points[a].x - points[b].x) ** 2 + (points[a].y - points[b].y) ** 2;
	// the pool runs out before `count` in a small community: any member will do then
	const candidates = (from: number[]) => from.filter((index) => !taken.has(index));
	while (picks.length < count) {
		const at = picks[picks.length - 1];
		let left = candidates(pool);
		if (left.length === 0) left = candidates([...members].sort((a, b) => a - b));
		if (left.length === 0) break;
		const far = left.filter((index) => gap(at, index) >= near);
		const from = far.length > 0 ? far : left;
		let next = from[0];
		for (const index of from) {
			const better = gap(at, index) - gap(at, next);
			if (better < 0 || (better === 0 && index < next)) next = index;
		}
		picks.push(next);
		taken.add(next);
	}
	return picks;
}

/** The constellations of all the communities, flat: `papers[offsets[c]] .. papers[offsets[c + 1] - 1]` is
 *  community c's line. */
export interface Constellations {
	papers: Int32Array;
	offsets: Int32Array;
}

export function packConstellations(lines: readonly (readonly number[])[]): Constellations {
	const offsets = new Int32Array(lines.length + 1);
	lines.forEach((line, index) => (offsets[index + 1] = offsets[index] + line.length));
	const papers = new Int32Array(offsets[lines.length]);
	lines.forEach((line, index) => papers.set(line, offsets[index]));
	return { papers, offsets };
}

/** What "Play the constellation intro" can be set to (lib/ui/prefs.ts). */
export type IntroWhen = "opened" | "made" | "never";

export const INTRO_WHENS: readonly IntroWhen[] = ["opened", "made", "never"];

/** Whether the intro plays for a run shown now: whenever one is opened (the initial choice), only one just made, or
 *  never, as chosen; never for a run only shown again, nor for someone who asked their system for less motion. */
export function introPlays(when: IntroWhen, opening: Opening, lessMotion: boolean): boolean {
	if (lessMotion || opening === "refreshed") return false;
	if (when === "opened") return true;
	return when === "made" && opening === "made";
}

/**
 * Why a run's graph is being shown: a job just made it; it was opened (from the list, or the app started on it); or
 * it was already open and is only shown again (a job named its communities again or read its declarations), which
 * is not an opening and never plays the intro.
 */
export type Opening = "made" | "opened" | "refreshed";

/** How a run comes to be shown when a job that finished opens it: made by that job, the run already open shown
 *  again, or another run opened. */
export function openingOf(finished: string, made: boolean, open: string | null): Opening {
	if (made) return "made";
	return finished === open ? "refreshed" : "opened";
}
