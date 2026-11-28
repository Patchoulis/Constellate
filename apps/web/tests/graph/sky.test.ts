/** The loading sky (lib/graph/sky.ts): its stars, its constellations' loop, and how far a run being made has got. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunStep } from "@rp/contracts";
import { FIGURES, LOOP_MS, MOST_STARS, figureFrame, litStars, runShare, skyFigure, skyStars, starCount, twinkle } from "@/lib/graph/sky";

const step = (number: number, count: RunStep["count"] = null): RunStep => ({
	step: "embed",
	title: "Embed",
	number,
	of: 10,
	count,
	detail: null,
});

test("every figure has 6 to 8 stars, and its lines join stars it has, each star reached", () => {
	assert.ok(FIGURES.length >= 2, "a different constellation follows");
	for (const [index, figure] of FIGURES.entries()) {
		assert.ok(figure.stars.length >= 6 && figure.stars.length <= 8, `figure ${index} has ${figure.stars.length} stars`);
		const reached = new Set<number>();
		for (const [a, b] of figure.lines) {
			assert.ok(a !== b && a < figure.stars.length && b < figure.stars.length, `figure ${index}`);
			reached.add(a).add(b);
		}
		assert.equal(reached.size, figure.stars.length, `figure ${index} has a star no line reaches`);
		for (const [x, y] of figure.stars) assert.ok(Math.abs(x) <= 1 && Math.abs(y) <= 1);
	}
});

test("a figure's lines draw one after another, hold, fade, and the next figure follows, about every 6 seconds", () => {
	assert.equal(LOOP_MS, 6000);
	const first = FIGURES[0].lines.length;
	assert.deepEqual(skyFigure(0), { figure: 0, whole: 0, part: 0, alpha: 1 });
	const midway = skyFigure(LOOP_MS * 0.25);
	assert.equal(midway.figure, 0);
	assert.ok(midway.whole > 0 && midway.whole < first, "half its lines");
	assert.deepEqual(skyFigure(LOOP_MS * 0.6), { figure: 0, whole: first, part: 0, alpha: 1 }, "held, whole");
	const fading = skyFigure(LOOP_MS * 0.8);
	assert.ok(fading.alpha > 0 && fading.alpha < 1);
	assert.equal(skyFigure(LOOP_MS * 0.95).alpha, 0, "a moment of empty sky");
	assert.equal(skyFigure(LOOP_MS * 1.01).figure, 1, "a different constellation");
	assert.equal(skyFigure(LOOP_MS * FIGURES.length).figure, 0, "and round again, forever");
	assert.deepEqual(skyFigure(123456), skyFigure(123456), "the same time, the same sky");
});

test("with less motion the sky is still: one whole constellation", () => {
	for (const ms of [0, 2500, 99_999])
		assert.deepEqual(skyFigure(ms, true), { figure: 0, whole: FIGURES[0].lines.length, part: 0, alpha: 1 });
});

test("the stars are always in the same places, a few hundred at most, and a larger sky only adds stars", () => {
	const stars = skyStars();
	assert.deepEqual(skyStars().place, stars.place);
	assert.equal(stars.bright.length, MOST_STARS);
	assert.ok(MOST_STARS <= 400);
	for (let index = 0; index < MOST_STARS; index++) {
		assert.ok(stars.place[index * 2] >= 0 && stars.place[index * 2] < 1);
		assert.ok(stars.bright[index] >= 0.25 && stars.bright[index] <= 0.85);
	}
	assert.equal(starCount(0, 500), 0);
	assert.ok(starCount(300, 200) < starCount(1600, 900));
	assert.equal(starCount(10_000, 10_000), MOST_STARS);
	assert.ok(starCount(40, 40) >= 40, "even a small box has a sky");
	// twinkling stays near the star's own brightness
	for (const ms of [0, 1000, 3333]) {
		const bright = twinkle(0.6, 0.3, ms);
		assert.ok(bright <= 0.6 + 1e-9 && bright >= 0.6 * 0.64 - 1e-9);
	}
});

test("the constellation sits near the middle, above the words, and fits whatever box it is in", () => {
	for (const [width, height] of [
		[1600, 900],
		[400, 300],
		[200, 120],
	]) {
		const { x, y, unit } = figureFrame(width, height);
		assert.equal(x, width / 2);
		assert.ok(y < height / 2 || y === unit + 8, `${width}x${height}`);
		assert.ok(unit > 0 && unit <= Math.max(width, height));
	}
});

test("how far a run has got is the steps done and the counted part of the step under way, and nothing where nothing is said", () => {
	assert.equal(runShare(null), null);
	assert.equal(runShare(undefined), null);
	assert.equal(runShare({ ...step(1), of: 0 }), null, "a step of no steps says nothing");
	assert.equal(runShare(step(1)), 0);
	assert.equal(runShare(step(3)), 0.2, "two of ten steps done, the third not counting");
	assert.equal(runShare(step(3, { done: 50, of: 100, unit: "papers" })), 0.25);
	assert.equal(runShare(step(10, { done: 1, of: 1, unit: "communities" })), 1);
	assert.equal(runShare(step(3, { done: 5, of: 0, unit: "papers" })), 0.2, "a count of nothing adds nothing");
	assert.equal(runShare(step(3, { done: 500, of: 100, unit: "papers" })), 0.3, "never past the step");
});

test("the share of stars lit is the share of the run done, and every star when there is no number", () => {
	assert.equal(litStars(200, null), 200);
	assert.equal(litStars(200, 0), 0);
	assert.equal(litStars(200, 0.25), 50);
	assert.equal(litStars(200, 1), 200);
	assert.equal(litStars(200, 1.5), 200);
	assert.equal(litStars(200, -1), 0);
});
