/** Panel sizes: what each panel may be in a window, how a drag or a key changes it, and what storage gives back. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	draggedSize,
	fitSizes,
	GRAPH_MIN_HEIGHT,
	GRAPH_MIN_WIDTH,
	initialSizes,
	KEY_STEP,
	KEY_STEP_LARGE,
	keyedSize,
	limitsOf,
	PANELS,
	parsePanelSizes,
	serialisePanelSizes,
	type Sizes,
} from "@/lib/panel-sizes";

const WIDE = { width: 2560, height: 1440 };
const graphWidth = (sizes: Sizes, width: number) => width - sizes.sidebar - sizes.communities;

test("every panel starts at its initial size, which is within its limits", () => {
	for (const [id, spec] of Object.entries(PANELS)) {
		assert.ok(spec.min <= spec.initial && spec.initial <= spec.max, id);
		assert.equal(initialSizes()[id as keyof Sizes], spec.initial);
	}
	assert.deepEqual(fitSizes(initialSizes(), WIDE), initialSizes(), "a large window leaves them as they are");
});

test("a size beyond a panel's own limits is shown at the limit", () => {
	const sizes = fitSizes({ ...initialSizes(), sidebar: 10, communities: 5000, log: -3 }, null);
	assert.equal(sizes.sidebar, PANELS.sidebar.min);
	assert.equal(sizes.communities, PANELS.communities.max);
	assert.equal(sizes.log, PANELS.log.min);
});

test("in a narrow window the side panels give way, in proportion, and leave the graph its width", () => {
	const wanted = { ...initialSizes(), sidebar: PANELS.sidebar.min + 200, communities: PANELS.communities.min + 200 };
	const frame = { width: 1200, height: 800 };
	const fitted = fitSizes(wanted, frame);
	assert.ok(graphWidth(fitted, frame.width) >= GRAPH_MIN_WIDTH);
	assert.ok(graphWidth(fitted, frame.width) <= GRAPH_MIN_WIDTH + 2, "no more is taken than needed");
	// 200 above its minimum against 200: both give up about the same
	assert.ok(Math.abs(wanted.sidebar - fitted.sidebar - (wanted.communities - fitted.communities)) <= 1);
	// and they come back as chosen when the window is large again
	assert.deepEqual(fitSizes(wanted, WIDE), wanted);
});

test("no panel goes below its minimum, even in a window too small for that", () => {
	const fitted = fitSizes({ ...initialSizes(), reader: 1500, log: 800 }, { width: 500, height: 300 });
	for (const [id, spec] of Object.entries(PANELS)) assert.ok(fitted[id as keyof Sizes] >= spec.min, id);
});

test("the reader, the overview and the log leave the graph its width and height", () => {
	const frame = { width: 1600, height: 700 };
	const fitted = fitSizes({ ...initialSizes(), reader: 1800, overview: 1800, log: 900 }, frame);
	const free = graphWidth(fitted, frame.width);
	assert.equal(fitted.reader, free - GRAPH_MIN_WIDTH);
	assert.equal(fitted.overview, free - GRAPH_MIN_WIDTH);
	assert.ok(frame.height - fitted.log >= GRAPH_MIN_HEIGHT);
});

test("a panel can be dragged as far as the other panels leave room for the graph", () => {
	const frame = { width: 1400, height: 900 };
	const fitted = fitSizes(initialSizes(), frame);
	assert.deepEqual(limitsOf("sidebar", fitted, frame), {
		min: PANELS.sidebar.min,
		max: Math.min(PANELS.sidebar.max, frame.width - fitted.communities - GRAPH_MIN_WIDTH),
	});
	assert.equal(limitsOf("reader", fitted, frame).max, graphWidth(fitted, frame.width) - GRAPH_MIN_WIDTH);
	assert.equal(limitsOf("log", fitted, frame).max, Math.min(PANELS.log.max, frame.height - GRAPH_MIN_HEIGHT - 46));
	// a window too small: the range is just the minimum, never empty
	const tiny = { width: 400, height: 250 };
	for (const id of Object.keys(PANELS) as (keyof Sizes)[]) {
		const { min, max } = limitsOf(id, fitSizes(initialSizes(), tiny), tiny);
		assert.equal(max, min, id);
	}
	assert.deepEqual(limitsOf("reader", fitted, null), { min: PANELS.reader.min, max: PANELS.reader.max });
});

test("a drag moves the edge with the pointer, within the limits", () => {
	const limits = { min: 200, max: 500 };
	assert.equal(draggedSize(300, 100, 150, "right", limits), 350, "the right edge, dragged right: wider");
	assert.equal(draggedSize(300, 100, 150, "left", limits), 250, "the left edge, dragged right: narrower");
	assert.equal(draggedSize(300, 500, 420, "top", limits), 380, "the top edge, dragged up: taller");
	assert.equal(draggedSize(300, 100, 900, "right", limits), 500);
	assert.equal(draggedSize(300, 100, -900, "right", limits), 200);
	assert.equal(draggedSize(300, 100, 100.6, "right", limits), 301, "whole pixels");
});

test("the arrow keys move the edge, Shift further; Home, End and Enter go to the limits and back", () => {
	const limits = { min: 200, max: 500 };
	assert.equal(keyedSize("ArrowLeft", false, 300, "left", limits), 300 + KEY_STEP);
	assert.equal(keyedSize("ArrowRight", false, 300, "left", limits), 300 - KEY_STEP);
	assert.equal(keyedSize("ArrowRight", true, 300, "right", limits), 300 + KEY_STEP_LARGE);
	assert.equal(keyedSize("ArrowUp", false, 300, "top", limits), 300 + KEY_STEP);
	assert.equal(keyedSize("ArrowDown", true, 300, "top", limits), 300 - KEY_STEP_LARGE);
	assert.equal(keyedSize("ArrowRight", true, 490, "right", limits), 500, "not beyond the limit");
	assert.equal(keyedSize("Home", false, 300, "left", limits), 200);
	assert.equal(keyedSize("End", false, 300, "top", limits), 500);
	assert.equal(keyedSize("Enter", false, 300, "right", limits), "reset");
	assert.equal(keyedSize("ArrowUp", false, 300, "left", limits), null, "an arrow across the handle's axis is left alone");
	assert.equal(keyedSize("a", false, 300, "left", limits), null);
});

test("sizes read back from storage are what was written", () => {
	const sizes = { ...initialSizes(), sidebar: 333, reader: 1200, log: 120 };
	assert.deepEqual(parsePanelSizes(serialisePanelSizes(sizes)), sizes);
});

test("anything in storage that cannot be trusted gives the initial sizes", () => {
	for (const text of [
		null,
		undefined,
		"",
		"{",
		"null",
		"[]",
		"42",
		'"text"',
		'{"sizes":{"sidebar":300}}',
		'{"version":2,"sizes":{"sidebar":300}}',
		'{"version":1}',
		'{"version":1,"sizes":null}',
	])
		assert.deepEqual(parsePanelSizes(text), initialSizes(), String(text));
	// for one panel: only that panel
	const stored = JSON.stringify({
		version: 1,
		sizes: { sidebar: "300", communities: 1e9, reader: Number.NaN, overview: 10, log: 150.4, other: 5 },
	});
	assert.deepEqual(parsePanelSizes(stored), { ...initialSizes(), log: 150 });
});
