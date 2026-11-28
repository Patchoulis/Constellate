/** How marked papers are shown: rings for a few, a highlighted region for many (lib/graph/mark-look.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { HALO_ZOOM_SLACK, LOOK_FADE_MS, REGION_FROM, haloLayerFits, lookTowards, markLook, regionFromOf } from "@/lib/graph/mark-look";

test("a few shown marks get rings, as many as the number chosen or more a region, and none get nothing", () => {
	assert.equal(markLook(0, 25), "rings", "nothing marked: nothing changes");
	assert.equal(markLook(10, 25), "rings");
	assert.equal(markLook(24, 25), "rings");
	assert.equal(markLook(25, 25), "region");
	assert.equal(markLook(500, 25), "region");
	assert.equal(markLook(5, 5), "region");
	// marks added one by one: the look changes once, at the number chosen, and back only below it
	const looks = Array.from({ length: 40 }, (_, shown) => markLook(shown, 25));
	assert.equal(looks.filter((look, index) => index > 0 && look !== looks[index - 1]).length, 1);
});

test("the number chosen starts at 25, is a whole number from 5 to 500, and anything else reads as 25", () => {
	assert.deepEqual(REGION_FROM, { initial: 25, least: 5, most: 500 });
	assert.equal(regionFromOf(40), 40);
	assert.equal(regionFromOf("60"), 60);
	assert.equal(regionFromOf(12.6), 13);
	assert.equal(regionFromOf(1), 5, "below the bounds: the least");
	assert.equal(regionFromOf(-3), 5);
	assert.equal(regionFromOf(9000), 500, "above them: the most");
	for (const value of [null, undefined, "", " ", "many", NaN, Infinity, {}, [], true]) assert.equal(regionFromOf(value), 25, String(value));
});

test("the halos drawn at one zoom are kept while the zoom stays within a quarter of it, so a pan never redraws them", () => {
	assert.ok(haloLayerFits(0.3, 0.3));
	assert.ok(haloLayerFits(0.3, 0.3 * 1.2));
	assert.ok(haloLayerFits(0.3, 0.3 / 1.2));
	assert.ok(!haloLayerFits(0.3, 0.3 * 1.3), "zoomed in further: drawn again");
	assert.ok(!haloLayerFits(0.3, 0.3 / 1.3));
	assert.ok(!haloLayerFits(0, 0.3));
	assert.equal(HALO_ZOOM_SLACK, 1.25);
});

test("one look gives way to the other over a fifth of a second, from wherever it was", () => {
	assert.equal(LOOK_FADE_MS, 200);
	assert.equal(lookTowards(0, 1, 0), 0);
	assert.equal(lookTowards(0, 1, 100), 0.5);
	assert.equal(lookTowards(0, 1, 200), 1);
	assert.equal(lookTowards(0, 1, 5000), 1);
	assert.equal(lookTowards(0.5, 0, 100), 0.25, "turned back halfway");
	assert.equal(lookTowards(0, 1, 10, 0), 1, "with less motion, at once");
});
