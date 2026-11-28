/** The loading sky (lib/graph/sky.ts): its stars, its constellations' loop, and how far a run being made has got. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunStep } from "@rp/contracts";
import {
	FIELD_STARS,
	FIELD_TILE,
	FIGURES,
	LOOP_MS,
	PARALLAX,
	STAR_FIELDS,
	fieldOffset,
	fieldStars,
	figureFrame,
	runShare,
	skyFigure,
	skyStarCount,
	starLit,
	tilesCovering,
	twinkle,
} from "@/lib/graph/sky";

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

test("the field's stars are tiny, faint and always in the same places, and more stars only add stars", () => {
	const many = fieldStars(FIELD_STARS.many);
	assert.deepEqual(fieldStars(FIELD_STARS.many).place, many.place);
	assert.deepEqual(fieldStars(FIELD_STARS.few).place, many.place.slice(0, FIELD_STARS.few * 2), "a few are the first of many");
	for (let index = 0; index < FIELD_STARS.many; index++) {
		assert.ok(many.place[index * 2] >= 0 && many.place[index * 2] < 1);
		assert.ok(many.size[index] >= 0.5 && many.size[index] <= 1.4, `star ${index} is ${many.size[index]} px`);
		assert.ok(many.bright[index] > 0 && many.bright[index] <= 0.7);
	}
	const bright = [...many.bright].filter((value) => value >= 0.6).length;
	assert.ok(bright > 0 && bright < FIELD_STARS.many / 5, `${bright} brighter stars`);
	// twinkling stays near the star's own brightness
	for (const ms of [0, 1000, 3333]) {
		const shine = twinkle(0.6, 0.3, ms);
		assert.ok(shine <= 0.6 + 1e-9 && shine >= 0.6 * 0.64 - 1e-9);
	}
});

test("the choices give no stars, a few or many a tile, and the loading sky and the intro always have some", () => {
	assert.deepEqual([...STAR_FIELDS], ["off", "few", "many"]);
	assert.equal(FIELD_STARS.off, 0);
	assert.ok(0 < FIELD_STARS.few && FIELD_STARS.few < FIELD_STARS.many);
	// a few hundred at most on a large screen
	const tiles = tilesCovering(1920, 1080, { x: 0, y: 0 }).length;
	assert.ok(tiles * FIELD_STARS.many <= 1200, `${tiles * FIELD_STARS.many} stars on a 1920 x 1080 sky`);
	assert.equal(skyStarCount("off"), FIELD_STARS.few);
	assert.equal(skyStarCount("few"), FIELD_STARS.few);
	assert.equal(skyStarCount("many"), FIELD_STARS.many);
});

test("the field's tiles cover any box from any offset, without a gap and without a tile that shows nothing", () => {
	for (const [width, height] of [
		[1, 1],
		[300, 200],
		[1600, 900],
		[3840, 2160],
	])
		for (const offset of [
			{ x: 0, y: 0 },
			{ x: 100, y: 400 },
			{ x: FIELD_TILE - 1, y: 1 },
		]) {
			const tiles = tilesCovering(width, height, offset);
			for (const [x, y] of [
				[0, 0],
				[width - 0.5, 0],
				[0, height - 0.5],
				[width - 0.5, height - 0.5],
				[width / 2, height / 2],
			])
				assert.ok(
					tiles.some((tile) => x >= tile.x && x < tile.x + FIELD_TILE && y >= tile.y && y < tile.y + FIELD_TILE),
					`${x},${y} of ${width}x${height} from ${JSON.stringify(offset)}`,
				);
			for (const tile of tiles) assert.ok(tile.x < width && tile.y < height && tile.x + FIELD_TILE > 0 && tile.y + FIELD_TILE > 0);
			// tiles line up with the offset
			for (const tile of tiles) assert.equal((((tile.x - offset.x) % FIELD_TILE) + FIELD_TILE) % FIELD_TILE, 0);
		}
});

test("the field moves a small share of the camera's pan, the same way, and not at all before the camera has moved", () => {
	assert.ok(PARALLAX >= 0.03 && PARALLAX <= 0.06);
	assert.deepEqual(fieldOffset(0, 0), { x: 0, y: 0 }, "where the loading sky had it");
	assert.deepEqual(fieldOffset(100, 0), { x: 100 * PARALLAX, y: 0 });
	const back = fieldOffset(-100, -50);
	assert.ok(
		Math.abs(back.x - (FIELD_TILE - 100 * PARALLAX)) < 1e-9 && Math.abs(back.y - (FIELD_TILE - 50 * PARALLAX)) < 1e-9,
		"wraps round",
	);
	for (const pan of [-1e6, -12345, 7, 1e6]) {
		const { x, y } = fieldOffset(pan, pan);
		assert.ok(x >= 0 && x < FIELD_TILE && y >= 0 && y < FIELD_TILE);
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

test("about the share of stars lit is the share of the run done, every star when there is no number", () => {
	const lit = (share: number | null) => Array.from({ length: 1000 }, (_, index) => starLit(index, share)).filter(Boolean).length;
	assert.equal(lit(null), 1000);
	assert.equal(lit(0), 0);
	assert.equal(lit(1), 1000);
	assert.ok(Math.abs(lit(0.25) - 250) < 40, `${lit(0.25)} of 1,000 at a quarter`);
	// a star lit stays lit as the run goes on
	for (let index = 0; index < 1000; index++) if (starLit(index, 0.3)) assert.ok(starLit(index, 0.6));
});
