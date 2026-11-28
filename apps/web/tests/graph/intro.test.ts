/** The constellation intro's time and places (lib/graph/intro.ts), its constellations, and when it plays. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	BEATS,
	INTRO_MS,
	INTRO_WHENS,
	LINE_ALPHA,
	LINE_KEPT,
	MOST_DELAY,
	beat,
	constellationPicks,
	constellationSize,
	easeOut,
	introFrame,
	introPlan,
	introPlays,
	openingOf,
	introProgress,
	packConstellations,
	paintGathering,
	paperGathered,
	TWINKLE_GROUPS,
	twinkleGroup,
	paperPlace,
	tracedTo,
	unitHash,
} from "@/lib/graph/intro";
import { communityTree } from "@/lib/community/community";
import { layoutGraph } from "@/lib/graph/layout";
import { introLines } from "@/components/graph/graph-view";
import { generatedRun } from "./generated-run";

const close = (actual: number, expected: number, message?: string) =>
	assert.ok(Math.abs(actual - expected) < 1e-9, `${message ?? ""} ${actual} is not ${expected}`);

test("the intro's progress goes from 0 to 1 over its length, and stays there", () => {
	assert.equal(INTRO_MS, 4500);
	assert.equal(introProgress(-20), 0);
	assert.equal(introProgress(0), 0);
	close(introProgress(INTRO_MS / 2), 0.5);
	assert.equal(introProgress(INTRO_MS), 1);
	assert.equal(introProgress(INTRO_MS * 3), 1);
	close(introProgress(1000, 2000), 0.5, "a length of its own");
	assert.equal(introProgress(10, 0), 1, "no length: it is over");
});

test("the beats overlap as they were shown, each eased to a standstill", () => {
	assert.deepEqual(BEATS, { stars: [0, 0.18], gather: [0.14, 0.55], trace: [0.5, 0.72], light: [0.68, 0.92], names: [0.82, 1] });
	assert.equal(easeOut(0), 0);
	assert.equal(easeOut(1), 1);
	assert.ok(easeOut(0.5) > 0.5, "quick at the start, slow at the end");
	for (const which of ["stars", "gather", "trace", "light", "names"] as const) {
		const [from, to]: readonly [number, number] = BEATS[which];
		assert.equal(beat(from, which), 0, which);
		assert.equal(beat(to, which), 1, which);
		assert.equal(beat(from - 0.01, which), 0, `${which} before it begins`);
		assert.equal(beat(to + 0.01, which), 1, `${which} after it ends`);
	}
	// the gathering begins while the stars still come; the names while the colours still come
	assert.ok(BEATS.gather[0] < BEATS.stars[1] && BEATS.names[0] < BEATS.light[1] && BEATS.trace[0] < BEATS.gather[1]);
});

test("at the end nothing of the intro is left: no lines, no dust, everything lit and named", () => {
	const end = introFrame(1);
	assert.deepEqual([end.stars, end.light, end.names, end.trace, end.gathered], [1, 1, 1, 1, 1]);
	assert.ok(end.settled);
	assert.equal(end.line, 0);
	assert.equal(end.dust, 0);
	const start = introFrame(0);
	assert.deepEqual([start.stars, start.light, start.names, start.trace, start.gathered, start.dust], [0, 0, 0, 0, 0, 0]);
	assert.ok(!start.settled);
});

test("the traced lines are drawn at 55% white, fall to a quarter of that as the communities light, and go with the names", () => {
	assert.equal(LINE_ALPHA, 0.55);
	close(introFrame(0.6).line, LINE_ALPHA, "while traced");
	close(introFrame(BEATS.light[1]).line, LINE_ALPHA * LINE_KEPT * (1 - beat(BEATS.light[1], "names")), "lit");
	assert.ok(introFrame(0.99).line < introFrame(0.9).line);
	close(introFrame(0.9999).line, 0, "almost over");
	// the dust is there while the stars are, and gone once the communities are lit
	assert.ok(introFrame(0.2).dust > 0.9);
	assert.equal(introFrame(BEATS.light[1]).dust, 0);
});

test("every paper starts at the same scattered place every time, inside the view, and waits at most a quarter of the beat", () => {
	const plan = introPlan(5000);
	const again = introPlan(5000);
	assert.deepEqual(plan.start, again.start);
	assert.deepEqual(plan.delay, again.delay);
	for (let index = 0; index < 5000; index++) {
		for (const share of [plan.start[index * 2], plan.start[index * 2 + 1]]) assert.ok(share > 0 && share < 1);
		assert.ok(plan.delay[index] >= 0 && plan.delay[index] <= MOST_DELAY);
	}
	// scattered: not all in one corner
	const left = Array.from({ length: 5000 }, (_, index) => plan.start[index * 2]).filter((u) => u < 0.5).length;
	assert.ok(left > 2200 && left < 2800, `${left} of 5,000 on the left`);
	// a smaller graph's papers start where the same papers of a larger one do
	assert.deepEqual(introPlan(10).start, plan.start.slice(0, 20));
	assert.equal(unitHash(7, 1), unitHash(7, 1));
	assert.notEqual(unitHash(7, 1), unitHash(8, 1));
});

test("a paper waits for its turn, goes to its place, and is there by the end of the beat", () => {
	assert.equal(paperGathered(0, 0), 0);
	assert.equal(paperGathered(0.2, 0.25), 0, "still waiting");
	assert.ok(paperGathered(0.5, 0.25) > 0 && paperGathered(0.5, 0.25) < 1);
	assert.equal(paperGathered(1, 0.25), 1);
	assert.equal(paperGathered(1, 0), 1);
	assert.ok(paperGathered(0.4, 0) > paperGathered(0.4, 0.2), "the later paper is behind");
	const start = { x: -100, y: 40 };
	const place = { x: 300, y: -60 };
	assert.deepEqual(paperPlace(start, place, 0), start);
	assert.deepEqual(paperPlace(start, place, 1), place);
	assert.deepEqual(paperPlace(start, place, 0.5), { x: 100, y: -10 });
	assert.ok(introFrame(BEATS.gather[1]).settled, "every paper is at its place once the beat has ended");
	assert.ok(!introFrame(BEATS.gather[1] - 0.01).settled);
});

/** Papers in a 400 x 300 world box, a camera that fits it into a canvas, and pixels as large as the canvas. */
function gathering(width: number, height: number, dpr: number) {
	const points = Array.from({ length: 2000 }, (_, index) => ({ x: unitHash(index, 7) * 400, y: unitHash(index, 8) * 300 }));
	const sizes = new Float32Array(points.length).fill(1);
	const k = Math.min(width / 400, height / 300) * 0.9;
	const camera = { x: (width - 400 * k) / 2, y: (height - 300 * k) / 2, k, dpr };
	const pixels = { words: new Uint32Array(width * dpr * height * dpr), width: width * dpr, height: height * dpr };
	return { points, sizes, camera, pixels, plan: introPlan(points.length) };
}
const lit = (words: Uint32Array) => words.filter((word) => word >>> 24 > 0).length;

test("every paper's star is in the pixels and plainly visible at every moment of the gathering", () => {
	const { points, sizes, camera, pixels, plan } = gathering(600, 400, 1.5);
	const full = [1, 1, 1, 1];
	for (const gathered of [0, 0.25, 0.5, 0.75, 0.95, 1]) {
		const inside = paintGathering(pixels, plan, points, sizes, camera, 2, gathered, full, 0xffe2d6);
		assert.equal(inside, points.length, `at ${gathered} of the beat`);
		assert.ok(lit(pixels.words) > points.length, `at ${gathered}: stars of two by two pixels, a few overlapping`);
		const strongest = pixels.words.reduce((most, word) => Math.max(most, word >>> 24), 0);
		assert.equal(strongest, 255, "a star of a pixel or more is drawn at its full strength");
	}
	// a star smaller than a pixel is one pixel, fainter by its area, never gone
	paintGathering(pixels, plan, points, sizes, camera, 0.5, 1, full, 0xffe2d6);
	const faint = new Set(Array.from(pixels.words, (word) => word >>> 24).filter(Boolean));
	assert.deepEqual([...faint], [Math.round(0.25 * 255)]);
});

test("pixels made again for a resized canvas hold every star, inside them, where the camera now puts it", () => {
	const before = gathering(600, 400, 1.5);
	paintGathering(before.pixels, before.plan, before.points, before.sizes, before.camera, 2, 0.5, [1, 1, 1, 1], 0xffffff);
	// a panel dragged wider, and the page zoomed: the canvas is smaller and has another pixel ratio
	for (const [width, height, dpr] of [
		[380, 400, 1.5],
		[600, 400, 2],
		[1200, 300, 1],
	]) {
		const after = gathering(width, height, dpr);
		const inside = paintGathering(after.pixels, after.plan, after.points, after.sizes, after.camera, 2, 0.5, [1, 1, 1, 1], 0xffffff);
		assert.equal(inside, after.points.length, `${width} x ${height} at ${dpr}x`);
		// once gathered, a star is where the camera puts its paper
		paintGathering(after.pixels, after.plan, after.points, after.sizes, after.camera, 1, 1, [1, 1, 1, 1], 0xffffff);
		const { x, y, k } = after.camera;
		const at = (point: { x: number; y: number }) =>
			Math.round((point.y * k + y) * dpr - 0.5) * after.pixels.width + Math.round((point.x * k + x) * dpr - 0.5);
		assert.ok(after.points.every((point) => after.pixels.words[at(point)] >>> 24 === 255));
	}
});

test("a star's strength follows its twinkle group, and none is outside the pixels it is written into", () => {
	const { points, sizes, camera, pixels, plan } = gathering(300, 200, 1);
	paintGathering(pixels, plan, points, sizes, camera, 1, 1, [0.2, 0.4, 0.6, 0.8], 0xffffff);
	const strengths = new Set(Array.from(pixels.words, (word) => word >>> 24).filter(Boolean));
	for (const strength of strengths) assert.ok([51, 102, 153, 204].includes(strength), String(strength));
	assert.equal(pixels.words.length, pixels.width * pixels.height);
	for (const index of [0, 1, 2, 3])
		assert.equal(twinkleGroup(plan, index), Math.min(TWINKLE_GROUPS - 1, Math.floor(plan.twinkle[index] * 4)));
});

test("a traced line is drawn one segment after another", () => {
	assert.deepEqual(tracedTo(0, 6), { whole: 0, part: 0 });
	assert.deepEqual(tracedTo(0.5, 6), { whole: 3, part: 0 });
	const some = tracedTo(0.25, 6);
	assert.equal(some.whole, 1);
	close(some.part, 0.5);
	assert.deepEqual(tracedTo(1, 6), { whole: 6, part: 0 });
	assert.deepEqual(tracedTo(1.5, 6), { whole: 6, part: 0 });
	assert.deepEqual(tracedTo(0.5, 0), { whole: 0, part: 0 }, "a constellation of one paper has no line");
});

/** A small community: papers on a grid, one most central and a few well linked. */
function community() {
	const points = Array.from({ length: 100 }, (_, index) => ({ x: (index % 10) * 10, y: Math.floor(index / 10) * 10 }));
	const members = points.map((_, index) => index);
	const links = new Int32Array(100);
	for (const [index, count] of [
		[90, 9],
		[9, 8],
		[99, 7],
		[0, 6],
		[45, 5],
		[5, 4],
	])
		links[index] = count;
	return { points, members, links };
}

test("a constellation starts at the community's most central paper and never steps to a paper right beside it", () => {
	const { points, members, links } = community();
	const picks = constellationPicks(members, points, [44, 33], links, 64);
	assert.equal(picks[0], 44);
	assert.equal(picks.length, constellationSize(100));
	assert.equal(new Set(picks).size, picks.length, "no paper twice");
	for (let step = 1; step < picks.length; step++) {
		const [a, b] = [points[picks[step - 1]], points[picks[step]]];
		assert.ok(Math.hypot(a.x - b.x, a.y - b.y) >= 64 * 0.28, `step ${step} is too short`);
	}
	// only central or well-linked papers
	const hubs = new Set([...[...members].sort((a, b) => links[b] - links[a] || a - b).slice(0, 24), 44, 33]);
	for (const index of picks) assert.ok(hubs.has(index), `${index} is neither central nor well linked`);
});

test("a constellation is the same every time, and a paper outside the community never starts it", () => {
	const { points, members, links } = community();
	assert.deepEqual(constellationPicks(members, points, [44], links, 64), constellationPicks(members, points, [44], links, 64));
	const without = constellationPicks(members.slice(50), points, [44, 77], links, 64);
	assert.equal(without[0], 77, "44 is not one of its papers");
	for (const index of without) assert.ok(index >= 50);
	// nothing known to be central: the best-linked paper starts it
	assert.equal(constellationPicks(members, points, [], links, 64)[0], 90);
});

test("larger communities get longer constellations, 5 to 7 papers, and a tiny one all it has", () => {
	assert.equal(constellationSize(20), 5);
	assert.equal(constellationSize(200), 6);
	assert.equal(constellationSize(5000), 7);
	assert.equal(constellationSize(3), 3);
	const points = [
		{ x: 0, y: 0 },
		{ x: 1, y: 0 },
		{ x: 0, y: 1 },
	];
	assert.deepEqual(new Set(constellationPicks([0, 1, 2], points, [], [0, 0, 0], 1)), new Set([0, 1, 2]));
	assert.deepEqual(constellationPicks([], [], [], [], 1), []);
});

test("every community of a large run gets a constellation inside it, packed flat for drawing", () => {
	const { graph, communities } = generatedRun({ papers: 3000, communities: 12 });
	const roots = communityTree(communities);
	const lines = introLines(graph, roots, layoutGraph(graph, roots));
	assert.equal(lines.length, roots.length);
	lines.forEach((line, index) => {
		assert.ok(line.length >= 5 && line.length <= 7, `community ${index}: ${line.length} papers`);
		const members = new Set(roots[index].community.members);
		for (const paper of line) assert.ok(members.has(paper));
	});
	const packed = packConstellations(lines);
	assert.equal(packed.offsets.length, lines.length + 1);
	lines.forEach((line, index) => assert.deepEqual([...packed.papers.slice(packed.offsets[index], packed.offsets[index + 1])], line));
});

test("the intro plays for every run opened, or only for one just made, as chosen, and never with less motion", () => {
	assert.deepEqual([...INTRO_WHENS], ["opened", "made", "never"]);
	assert.equal(introPlays("opened", "opened", false), true, "any run opened");
	assert.equal(introPlays("opened", "made", false), true);
	assert.equal(introPlays("made", "made", false), true, "made");
	assert.equal(introPlays("made", "opened", false), false, "made, but this run was only opened");
	assert.equal(introPlays("never", "made", false), false);
	assert.equal(introPlays("never", "opened", false), false);
	const openings = ["made", "opened", "refreshed"] as const;
	for (const when of INTRO_WHENS) {
		for (const opening of openings) assert.equal(introPlays(when, opening, true), false, `${when}, ${opening}, less motion`);
		assert.equal(introPlays(when, "refreshed", false), false, `${when}: a run only shown again is not an opening`);
	}
});

test("a job that finishes opens its run as made, as the open run shown again, or as another run opened", () => {
	assert.equal(openingOf("iclr", true, "iclr"), "made", "made again, the same name");
	assert.equal(openingOf("iclr", true, null), "made", "the first run");
	assert.equal(openingOf("iclr", false, "iclr"), "refreshed", "its communities named again: the same run, not an opening");
	assert.equal(openingOf("iclr", false, "neurips"), "opened", "another run's job finished: that run is opened");
	assert.equal(openingOf("iclr", false, null), "opened");
});
