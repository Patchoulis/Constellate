/** How large a paper's dot is drawn (lib/graph/dot-size.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { SMALLEST_DOT_PX, screenDotRadius } from "@/lib/graph/dot-size";

test("zoomed in, a dot has its true size", () => {
	for (const k of [0.5, 1, 4, 40]) assert.equal(screenDotRadius(2.4, k), 2.4);
});

test("zoomed out, a dot keeps about its smallest size on screen, never more than about 2% off", () => {
	for (let k = 0.03; k < 0.354; k *= 1.013) {
		const onScreen = screenDotRadius(2.4, k) * k;
		assert.ok(Math.abs(onScreen / SMALLEST_DOT_PX - 1) <= 0.022, `${onScreen.toFixed(3)} px at zoom ${k.toFixed(4)}`);
		assert.ok(screenDotRadius(2.4, k) >= 2.4);
	}
});

test("a zoom changes the dot's size in steps, so the paths made for it are made again only now and then", () => {
	const sizes = new Set<number>();
	let frames = 0;
	// a zoom out by eight times, in small steps as a wheel or a glide makes them
	for (let k = 0.35; k > 0.35 / 8; k /= 1.01) {
		sizes.add(screenDotRadius(2.4, k));
		frames++;
	}
	assert.ok(frames > 200);
	assert.ok(sizes.size <= 3 * 16 + 1, `${sizes.size} sizes over three octaves`);
	// steps go one way as the zoom goes one way
	const ordered = [...sizes];
	for (let index = 1; index < ordered.length; index++) assert.ok(ordered[index] > ordered[index - 1]);
});
