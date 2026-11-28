/** Figures: from a rectangle in PDF points to the pixels pdf.js draws it at, and what the sandboxed frame may do. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { PaperFigure } from "@rp/contracts";
import { BOOTSTRAP, framePage, framePolicy, isTold } from "@/lib/reader/figure-frame";
import { MOST_PIXELS, cropFor, shownSize, toViewport } from "@/lib/reader/figures";

const letter = { x0: 0, y0: 0, x1: 612, y1: 792 };
const figure = (rotation: number, crop = letter): Pick<PaperFigure, "rect" | "page_box"> => ({
	rect: { x0: 100, y0: 450, x1: 400, y1: 640 },
	page_box: { crop, rotation },
});

test("a point of an upright page is where pdf.js draws it: from the crop box's top left, y down", () => {
	assert.deepEqual(toViewport(letter, 0, 1, 0, 792), [0, 0]);
	assert.deepEqual(toViewport(letter, 0, 2, 100, 640), [200, 304]);
	// a crop box that does not start at the origin moves everything with it
	assert.deepEqual(toViewport({ x0: 10, y0: 20, x1: 602, y1: 782 }, 0, 1, 10, 782), [0, 0]);
});

test("a page turned a quarter, a half or three quarters puts the figure where the turned page shows it", () => {
	// turned clockwise a quarter: the page's bottom left is the top left of what is shown
	assert.deepEqual(toViewport(letter, 90, 1, 0, 0), [0, 0]);
	assert.deepEqual(toViewport(letter, 90, 1, 612, 792), [792, 612]);
	assert.deepEqual(toViewport(letter, 180, 1, 612, 0), [0, 0]);
	assert.deepEqual(toViewport(letter, 270, 1, 612, 792), [0, 0]);
	assert.deepEqual(toViewport(letter, -90, 1, 612, 792), [0, 0], "-90 is 270");
});

test("a figure is drawn as wide as asked, its crop the figure's own pixels, on an upright page and a turned one", () => {
	const upright = cropFor(figure(0), 600)!;
	assert.equal(upright.scale, 2);
	assert.deepEqual([upright.left, upright.top, upright.width, upright.height], [200, 304, 600, 380]);
	assert.deepEqual(shownSize(figure(0)), { width: 300, height: 190 });
	// turned a quarter, the figure is shown 190 wide and 300 high
	assert.deepEqual(shownSize(figure(90)), { width: 190, height: 300 });
	const turned = cropFor(figure(90), 380)!;
	assert.equal(turned.scale, 2);
	assert.deepEqual([turned.left, turned.top, turned.width, turned.height], [900, 200, 380, 600]);
});

test("no figure is drawn with more pixels than the limit, nor one with no size", () => {
	const huge = cropFor({ rect: { x0: 0, y0: 0, x1: 600, y1: 780 }, page_box: { crop: letter, rotation: 0 } }, 20_000)!;
	assert.ok(huge.width * huge.height <= MOST_PIXELS * 1.01, `${huge.width} x ${huge.height}`);
	assert.equal(cropFor({ rect: { x0: 5, y0: 5, x1: 5, y1: 9 }, page_box: { crop: letter, rotation: 0 } }, 400), null);
	assert.equal(cropFor(figure(0), 0), null);
});

test("the frame may reach nothing: no connection, no frame, no form, scripts only its own and the blobs it makes", () => {
	const policy = framePolicy("abc=");
	for (const rule of ["default-src 'none'", "connect-src 'none'", "frame-src 'none'", "form-action 'none'", "base-uri 'none'"]) {
		assert.ok(policy.includes(rule), rule);
	}
	assert.ok(policy.includes("script-src 'sha256-abc=' blob:"));
	const scripts = policy.split("; ").find((rule) => rule.startsWith("script-src"))!;
	assert.ok(!scripts.includes("'unsafe-eval'") && !scripts.includes("'self'"), scripts);
	// pdf.js reads with eval off, and the code comes in the page, base64, so the page holds no markup of it
	assert.ok(BOOTSTRAP.includes("isEvalSupported: false"));
	const page = framePage(btoa("</script><script>alert(1)</script>"), btoa("worker"));
	assert.ok(!page.includes("<script>alert"), "the carried code cannot close its own element");
});

test("only answers in the frame's shapes are taken from it", () => {
	assert.ok(isTold({ kind: "ready" }));
	assert.ok(isTold({ kind: "drawn", id: 3, png: new Blob([], { type: "image/png" }) }));
	assert.ok(!isTold({ kind: "drawn", id: 3, png: new Blob([], { type: "text/html" }) }), "only a PNG");
	assert.ok(isTold({ kind: "failed", id: null, why: "no PDF" }));
	for (const junk of [null, "drawn", { kind: "navigate", url: "https://example.org" }, { kind: "drawn", id: "3" }]) {
		assert.ok(!isTold(junk), JSON.stringify(junk));
	}
});
