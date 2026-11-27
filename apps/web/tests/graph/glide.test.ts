/** The camera's glide: where it is on the way, and that only one glide runs at a time. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { ease, Glide, GLIDE_MS, viewBetween, type Frames, type View } from "@/lib/graph/glide";

const size = { width: 800, height: 600 };
/** The world point a view has in the middle of the picture. */
const centre = (view: View) => ({ x: (size.width / 2 - view.x) / view.k, y: (size.height / 2 - view.y) / view.k });
const near = (a: number, b: number) => assert.ok(Math.abs(a - b) < 1e-9, `${a} is not ${b}`);

/** Frames a test steps by hand: `tick(ms)` moves the clock and runs the frame that was asked for. */
function frames() {
	let time = 0;
	let next = 0;
	const waiting = new Map<number, () => void>();
	const controlled: Frames = {
		request: (step) => (waiting.set(++next, step), next),
		cancel: (frame) => void waiting.delete(frame),
		now: () => time,
	};
	const tick = (ms: number) => {
		time += ms;
		const due = [...waiting.values()];
		waiting.clear();
		for (const step of due) step();
	};
	return { controlled, tick, waiting };
}

test("the easing starts and ends gently and is half done half way", () => {
	assert.equal(ease(0), 0);
	assert.equal(ease(1), 1);
	assert.equal(ease(0.5), 0.5);
	assert.ok(ease(0.1) < 0.1 && ease(0.9) > 0.9);
	assert.equal(ease(-3), 0);
	assert.equal(ease(7), 1);
});

test("a glide starts at the first view, ends at the second, and changes zoom by an even factor", () => {
	const from: View = { x: 100, y: 50, k: 1 };
	const to: View = { x: -2000, y: -900, k: 16 };
	assert.deepEqual(viewBetween(from, to, size, 0), from);
	assert.deepEqual(viewBetween(from, to, size, 1), to);
	const half = viewBetween(from, to, size, 0.5);
	near(half.k, 4); // the geometric middle of 1 and 16
	// the point in the middle of the picture is half way between the two views' middles
	near(centre(half).x, (centre(from).x + centre(to).x) / 2);
	near(centre(half).y, (centre(from).y + centre(to).y) / 2);
});

test("a glide shows views on the way and says once that it has arrived", () => {
	const { controlled, tick } = frames();
	const glide = new Glide(controlled);
	const shown: View[] = [];
	let arrivals = 0;
	const to: View = { x: 0, y: 0, k: 4 };
	glide.go(
		{ x: 0, y: 0, k: 1 },
		to,
		size,
		(view) => shown.push(view),
		() => arrivals++,
	);
	assert.ok(glide.running);
	tick(GLIDE_MS / 2);
	near(shown[0].k, 2);
	assert.equal(arrivals, 0);
	tick(GLIDE_MS);
	assert.deepEqual(shown.at(-1), to);
	assert.equal(arrivals, 1);
	assert.equal(glide.running, false);
	tick(GLIDE_MS); // nothing more happens
	assert.equal(shown.length, 2);
});

test("a new glide ends the one in progress and carries on from where the camera is", () => {
	const { controlled, tick, waiting } = frames();
	const glide = new Glide(controlled);
	let camera: View = { x: 0, y: 0, k: 1 };
	const show = (view: View) => (camera = view);
	const arrived: string[] = [];

	glide.go(camera, { x: 0, y: 0, k: 16 }, size, show, () => arrived.push("first"));
	tick(GLIDE_MS / 2);
	near(camera.k, 4); // half way to the first destination
	const midway = camera;

	const second: View = { x: 300, y: 200, k: 2 };
	glide.go(camera, second, size, show, () => arrived.push("second"));
	assert.equal(waiting.size, 1, "the first glide's next frame was called off");
	tick(1);
	// it starts from the half-way view: no jump back to the start, none ahead to the first destination
	assert.ok(Math.abs(camera.k - midway.k) < 0.01, `the camera jumped to zoom ${camera.k}`);
	tick(GLIDE_MS / 2);
	assert.ok(camera.k < midway.k && camera.k > second.k, "it is heading for the second destination");
	tick(GLIDE_MS);
	assert.deepEqual(camera, second);
	assert.deepEqual(arrived, ["second"]); // the first never arrived
});

test("stopping a glide leaves the camera where it is and nobody is told it arrived", () => {
	const { controlled, tick } = frames();
	const glide = new Glide(controlled);
	let camera: View = { x: 0, y: 0, k: 1 };
	let arrivals = 0;
	glide.go(
		camera,
		{ x: 0, y: 0, k: 16 },
		size,
		(view) => (camera = view),
		() => arrivals++,
	);
	tick(GLIDE_MS / 2);
	const stoppedAt = camera;
	glide.stop();
	tick(GLIDE_MS * 2);
	assert.deepEqual(camera, stoppedAt);
	assert.equal(arrivals, 0);
	assert.equal(glide.running, false);
});

test("a picture resized during a glide ends it at once at the destination worked out for the new size", () => {
	const { controlled, tick, waiting } = frames();
	const glide = new Glide(controlled);
	let camera: View = { x: 0, y: 0, k: 1 };
	let arrivals = 0;
	// centre the world point (100, 100) at zoom 4, whatever the size
	const aim = ({ width, height }: { width: number; height: number }): View => ({ x: width / 2 - 400, y: height / 2 - 400, k: 4 });
	glide.go(
		camera,
		aim,
		size,
		(view) => (camera = view),
		() => arrivals++,
	);
	tick(GLIDE_MS / 3);
	const smaller = { width: 500, height: 400 };
	glide.resized(smaller);
	assert.deepEqual(camera, aim(smaller), "the destination for the new size, not the one for the old");
	assert.equal(arrivals, 1, "told once that it arrived");
	assert.equal(glide.running, false);
	assert.equal(waiting.size, 0, "no frame left to run");
	tick(GLIDE_MS);
	assert.equal(arrivals, 1);
	glide.resized(size); // no glide under way: nothing moves
	assert.deepEqual(camera, aim(smaller));
	assert.equal(arrivals, 1);
});

test("a glide to a fixed view arrives there whatever the size became", () => {
	const { controlled, tick } = frames();
	const glide = new Glide(controlled);
	let camera: View = { x: 0, y: 0, k: 1 };
	const to: View = { x: 10, y: 20, k: 2 };
	glide.go(camera, to, size, (view) => (camera = view));
	tick(10);
	glide.resized({ width: 100, height: 100 });
	assert.deepEqual(camera, to);
});
