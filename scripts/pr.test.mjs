/**
 * The naming rule `npm run pr` checks: a caught error is called `error`.    node --test scripts
 *
 * The sample lines are put together when the test runs (C is the keyword), or the rule would find them in this file.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { caughtName } from "./pr.mjs";

const C = "catch";

test("every way TypeScript and JavaScript catch an error is seen, with its name", () => {
	for (const [line, name] of [
		[`} ${C} (e) {`, "e"],
		[`} ${C} (err: unknown) {`, "err"],
		[`} ${C} (error) {`, "error"],
		[`} ${C} ( error : unknown ) {`, "error"],
		[`api.save(changes).${C}((e) => setFailed(e.message));`, "e"],
		[`api.save(changes).${C}(e => null);`, "e"],
		[`api.save(changes).${C}(async (problem) => {`, "problem"],
		[`promise.${C}(function (ex) {`, "ex"],
		[`promise.${C}((error: Error) => report(error));`, "error"],
	]) {
		assert.equal(caughtName(line), name, line);
	}
});

test("a catch that names nothing, or hands the error on, or a comment about one, is not a name to check", () => {
	for (const line of [
		`} ${C} {`,
		`promise.${C}(() => {});`,
		`promise.${C}(console.error);`,
		`promise.${C}(reject);`,
		`// never write ${C} (e)`,
		` * the old way: ${C} (e)`,
		"const caught = 1;",
	]) {
		assert.equal(caughtName(line), null, line);
	}
});
