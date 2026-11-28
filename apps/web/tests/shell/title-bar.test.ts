/** The window's own top bar: its height and the room it leaves the system's window buttons. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { BAR_HEIGHT, barInsets } from "@/lib/shell/title-bar";

test("the bar leaves the system's buttons the room the browser reports, and none in a browser tab", () => {
	const { paddingLeft, paddingRight } = barInsets();
	// the free area's own figures, with fallbacks that come to no room at all where there is no overlay
	assert.match(paddingRight, /^calc\(100vw - env\(titlebar-area-x, 0px\) - env\(titlebar-area-width, 100vw\)\)$/);
	assert.match(paddingLeft, /^max\(1rem, env\(titlebar-area-x, 0px\)\)$/);
});

test("the shell gives the system's buttons the height of the page's bar", () => {
	const shell = readFileSync(new URL("../../../desktop/main.cjs", import.meta.url), "utf8");
	const said = /const BAR_HEIGHT = (\d+);/.exec(shell);
	assert.ok(said, "apps/desktop/main.cjs states the bar's height");
	assert.equal(Number(said[1]), BAR_HEIGHT);
	assert.match(shell, /titleBarStyle: "hidden"/);
	assert.match(shell, /titleBarOverlay:/);
});
