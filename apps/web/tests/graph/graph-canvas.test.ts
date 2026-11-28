import assert from "node:assert/strict";
import { test } from "node:test";
import { glowLayerGeometry, inside, visibleWorld } from "@/lib/graph/glow-layer";
import { nearestPaper } from "@/lib/graph/pick";
import { randomFrom } from "./generated-run";

test("the glow layer holds every halo whole, as sharp as its sprite", () => {
	const points = [
		{ x: -100, y: 20 },
		{ x: 250, y: -40 },
		{ x: 30, y: 310 },
	];
	const layer = glowLayerGeometry(points, 48, 96)!;
	assert.equal(layer.scale, 1); // a 96-texel sprite over a 96-unit halo
	for (const { x, y } of points) {
		assert.ok(layer.box.minX <= x - 48 && layer.box.maxX >= x + 48, "a halo is cut left or right");
		assert.ok(layer.box.minY <= y - 48 && layer.box.maxY >= y + 48, "a halo is cut above or below");
	}
	// drawn back at exactly the scale it was drawn at: the box is the layer's whole pixels
	assert.equal((layer.box.maxX - layer.box.minX) * layer.scale, layer.width);
	assert.equal((layer.box.maxY - layer.box.minY) * layer.scale, layer.height);
	assert.equal(glowLayerGeometry(points, 24, 96)!.scale, 2);
});

test("a glow layer too large to keep, or with nothing in it, is not made", () => {
	const far = [
		{ x: 0, y: 0 },
		{ x: 10_000, y: 10_000 },
	];
	assert.equal(glowLayerGeometry(far, 48, 96), null);
	assert.ok(glowLayerGeometry(far, 48, 96, 200_000_000));
	assert.equal(glowLayerGeometry([], 48, 96), null);
});

test("papers with no finite place make no glow layer: a canvas of no size cannot be drawn", () => {
	assert.equal(glowLayerGeometry([{ x: Number.NaN, y: Number.NaN }], 48, 96), null);
	assert.equal(glowLayerGeometry([{ x: Infinity, y: 0 }], 48, 96), null);
});

test("the visible part of the world is the canvas seen through the camera", () => {
	const camera = { x: 100, y: -50, k: 2 };
	const box = visibleWorld(camera, 800, 600);
	// the canvas's corners, back in the world
	assert.deepEqual(box, { minX: -50, minY: 25, maxX: 350, maxY: 325 });
	const grown = visibleWorld(camera, 800, 600, 10);
	assert.deepEqual(grown, { minX: -60, minY: 15, maxX: 360, maxY: 335 });
	assert.ok(inside(box, -50, 325), "edges count as inside");
	assert.ok(!inside(box, -50.01, 100));
});

/** What the renderer did before the scan had its own module: kept here as the answer to compare with. */
function scanEveryPaper(
	points: { x: number; y: number }[],
	sizes: Float32Array,
	wx: number,
	wy: number,
	radius: number,
	margin: number,
	lit: (i: number) => boolean,
) {
	let best = Infinity;
	let index: number | null = null;
	points.forEach((point, i) => {
		const distance = (point.x - wx) ** 2 + (point.y - wy) ** 2;
		if (distance < best && distance < (radius * sizes[i] + margin) ** 2 && lit(i)) {
			best = distance;
			index = i;
		}
	});
	return index;
}

test("picking finds the same paper as scanning every paper, sizes, margin and focus included", () => {
	const random = randomFrom(7);
	// a coarse grid, so that many papers share a spot or are equally far from the pointer
	const points = Array.from({ length: 3000 }, () => ({ x: Math.round(random() * 60) * 5, y: Math.round(random() * 60) * 5 }));
	const sizes = Float32Array.from(points, () => [1, 1.45, 1.9][Math.floor(random() * 3)]);
	const lit = (index: number) => index % 7 !== 0;
	let found = 0;
	for (let trial = 0; trial < 2000; trial++) {
		const at = { x: Math.round(random() * 640) / 2 - 5, y: Math.round(random() * 640) / 2 - 5 };
		const radius = [2.4, 0.5, 8][trial % 3];
		const margin = [5, 0, 1.25][trial % 3];
		const expected = scanEveryPaper(points, sizes, at.x, at.y, radius, margin, lit);
		assert.equal(nearestPaper(points, sizes, at, radius, margin, lit), expected, `trial ${trial}`);
		if (expected !== null) found++;
	}
	assert.ok(found > 500, "most trials should find a paper");
});

test("of two papers equally near, the first is picked; one out of reach or out of focus is not", () => {
	const points = [
		{ x: 10, y: 0 },
		{ x: -10, y: 0 },
		{ x: 0, y: 30 },
	];
	const sizes = Float32Array.of(1, 1, 1);
	const all = () => true;
	assert.equal(nearestPaper(points, sizes, { x: 0, y: 0 }, 6, 5, all), 0);
	assert.equal(
		nearestPaper(points, sizes, { x: 0, y: 0 }, 6, 5, (index) => index !== 0),
		1,
	);
	assert.equal(nearestPaper(points, sizes, { x: 0, y: 0 }, 4, 5, all), null); // 10 away, reach 9
	// 8 away: a usual dot reaches 5, a larger one 8.6
	assert.equal(nearestPaper(points, sizes, { x: 0, y: 22 }, 4, 1, all), null);
	assert.equal(nearestPaper(points, Float32Array.of(1, 1, 1.9), { x: 0, y: 22 }, 4, 1, all), 2);
});
