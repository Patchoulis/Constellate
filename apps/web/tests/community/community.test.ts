/** The bars of the list of communities, and figures that divide by a community's size. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { barScale, barWidth, highlightShare } from "@/lib/community/community";

test("with no figure known yet (citations not counted), the bars are empty and finite, not NaN wide", () => {
	const scale = barScale([null, null], false);
	assert.deepEqual(scale, { low: 0, span: 1 });
	assert.equal(barWidth(null, scale), 0);
	assert.deepEqual(barScale([], true), { low: 0, span: 1 });
	assert.deepEqual(barScale([Number.NaN], true), { low: 0, span: 1 });
});

test("counts are bars from zero, and a narrow band starts just under its lowest figure", () => {
	const counts = barScale([10, null, 40], false);
	assert.equal(barWidth(40, counts), 100);
	assert.equal(barWidth(10, counts), 25);
	const band = barScale([0.2, 0.3], true);
	assert.ok(Math.abs(band.low - 0.18) < 1e-9);
	assert.equal(barWidth(0.3, band), 100);
	// every figure the same: full bars, not a division by 0
	assert.equal(barWidth(5, barScale([5, 5], false)), 100);
	assert.equal(barWidth(Number.NaN, counts), 0);
});

test("a community of no papers has no share of highlights, rather than NaN or Infinity of them", () => {
	assert.ok(Number.isNaN(highlightShare({ size: 0, acceptance: { oral: 1 } })));
	assert.equal(highlightShare({ size: 4, acceptance: { oral: 1, spotlight: 1 } }), 0.5);
});
