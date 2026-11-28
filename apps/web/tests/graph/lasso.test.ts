/** A lasso on the graph: which papers a closed line holds, and what tells a lasso from a click. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { inside, LASSO_MIN, lassoed, reach } from "@/lib/graph/lasso";

const square = [
	{ x: 0, y: 0 },
	{ x: 10, y: 0 },
	{ x: 10, y: 10 },
	{ x: 0, y: 10 },
];

test("a place is inside a closed line or outside it, whichever way the line was drawn", () => {
	assert.ok(inside(square, 5, 5));
	assert.ok(inside([...square].reverse(), 5, 5), "drawn the other way round");
	for (const [x, y] of [
		[-1, 5],
		[11, 5],
		[5, -1],
		[5, 11],
	])
		assert.ok(!inside(square, x, y), `${x},${y}`);
});

test("a line that is not convex holds only what it goes round", () => {
	// an L: the square with its upper right quarter cut out
	const corner = [
		{ x: 0, y: 0 },
		{ x: 5, y: 0 },
		{ x: 5, y: 5 },
		{ x: 10, y: 5 },
		{ x: 10, y: 10 },
		{ x: 0, y: 10 },
	];
	assert.ok(inside(corner, 2, 2));
	assert.ok(inside(corner, 8, 8));
	assert.ok(!inside(corner, 8, 2), "in the quarter cut out");
});

test("the papers a lasso holds are those inside it, in their order, and only those it may take", () => {
	const places = [
		{ x: 1, y: 1 },
		{ x: 20, y: 20 },
		{ x: 9, y: 9 },
		{ x: 5, y: 5 },
		{ x: -3, y: 4 },
	];
	assert.deepEqual(lassoed(square, places), [0, 2, 3]);
	assert.deepEqual(
		lassoed(square, places, (index) => index !== 2),
		[0, 3],
		"a paper out of focus is not taken",
	);
	assert.deepEqual(lassoed(square, []), []);
});

test("a line of fewer than three places goes round nothing", () => {
	const places = [{ x: 0, y: 0 }];
	assert.deepEqual(lassoed([], places), []);
	assert.deepEqual(lassoed([{ x: 0, y: 0 }], places), []);
	assert.deepEqual(lassoed(square.slice(0, 2), places), []);
});

test("how far the pointer went tells a lasso from a click", () => {
	assert.equal(reach([]), 0);
	assert.equal(reach([{ x: 4, y: 4 }]), 0);
	// out and back again: the farthest it got counts, not where it ended
	const there = [
		{ x: 0, y: 0 },
		{ x: 30, y: 40 },
		{ x: 1, y: 1 },
	];
	assert.equal(reach(there), 50);
	assert.ok(reach(there) >= LASSO_MIN);
	const hardly = [
		{ x: 0, y: 0 },
		{ x: 2, y: 1 },
	];
	assert.ok(reach(hardly) < LASSO_MIN, "a hand's tremble while clicking");
});
