/** What the window says when its page stops working. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { errorLine, isStaleInterface, PAGE_ERROR } from "@/lib/shell/page-error";

test("the reason is one line of what the page reported, with its reference when it has one", () => {
	assert.equal(errorLine(new Error("layout.discs.get(...) is undefined")), "layout.discs.get(...) is undefined");
	assert.equal(errorLine({ message: "first line\n    at somewhere", digest: "1234" }), "first line (reference 1234)");
	assert.equal(errorLine({ message: "x".repeat(400) }), `${"x".repeat(300)}…`);
});

test("a page that gave no reason says so, never an empty line", () => {
	assert.equal(errorLine(null), "No reason was given.");
	assert.equal(errorLine({}), "No reason was given.");
	assert.equal(errorLine({ message: "   ", digest: "9" }), "No reason was given (reference 9).");
	assert.equal(errorLine({ message: 5 }), "No reason was given.");
});

test("a part of the interface that could not be fetched is told from any other failure", () => {
	assert.ok(isStaleInterface({ name: "ChunkLoadError", message: "Loading chunk 12 failed." }));
	assert.ok(isStaleInterface(new Error("Failed to fetch dynamically imported module: /_next/static/chunks/a.js")));
	assert.ok(!isStaleInterface(new Error("Cannot read properties of undefined (reading 'x')")));
	assert.ok(!isStaleInterface(null));
});

test("the words name the app's data as safe and say where the log is", () => {
	assert.match(PAGE_ERROR.other, /library, runs and marks are not affected/);
	assert.match(PAGE_ERROR.stale, /Loading the window again fixes it/);
	assert.match(PAGE_ERROR.log, /engine\.log/);
});
