/** The dots after words that say something is under way (lib/ui/loading-dots.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { DOT_STEP_MS, dotOpacity, dotsShown } from "@/lib/ui/loading-dots";

test("one dot, two, three, then round again, a step about every 450 ms", () => {
	assert.equal(DOT_STEP_MS, 450);
	assert.equal(dotsShown(0), 1);
	assert.equal(dotsShown(449), 1);
	assert.equal(dotsShown(450), 2);
	assert.equal(dotsShown(900), 3);
	assert.equal(dotsShown(1349), 3);
	assert.equal(dotsShown(1350), 1, "round again");
	assert.equal(dotsShown(1350 * 1000 + 500), 2, "forever");
});

test("the first dot is always there, and the others are there once their step has come", () => {
	for (const elapsed of [0, 300, 700, 1200, 5000]) assert.equal(dotOpacity(0, elapsed), 1);
	assert.equal(dotOpacity(1, 100), 0);
	assert.equal(dotOpacity(2, 600), 0);
	assert.equal(dotOpacity(1, 800), 1);
	assert.equal(dotOpacity(2, 1100), 1);
	for (let elapsed = 0; elapsed < 1350; elapsed += 10) {
		const shown = dotsShown(elapsed);
		for (const dot of [1, 2]) if (dot >= shown) assert.equal(dotOpacity(dot, elapsed), 0, `dot ${dot} at ${elapsed} ms`);
	}
});

test("a dot fades in and out rather than popping", () => {
	const coming = dotOpacity(1, 450 + 60);
	assert.ok(coming > 0 && coming < 1, `${coming} just after it comes`);
	const going = dotOpacity(2, 1350 - 40);
	assert.ok(going > 0 && going < 1, `${going} just before the round ends`);
	// no step jumps by more than the fade allows, at the 60 frames a second the dots are drawn at
	for (const dot of [1, 2])
		for (let elapsed = 0; elapsed < 2700; elapsed += 16) {
			const change = Math.abs(dotOpacity(dot, elapsed + 16) - dotOpacity(dot, elapsed));
			assert.ok(change <= 0.2, `dot ${dot} jumps by ${change.toFixed(2)} at ${elapsed} ms`);
		}
	for (let elapsed = 0; elapsed < 2700; elapsed += 7)
		for (const dot of [0, 1, 2]) assert.ok(dotOpacity(dot, elapsed) >= 0 && dotOpacity(dot, elapsed) <= 1);
});
