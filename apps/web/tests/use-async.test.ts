/** Where a fetch stands (lib/use-async.ts): every change of state, and the changes that are refused. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { asked, asyncStep, ready, type Async } from "@/lib/use-async";

const idle: Async<string> = { status: "idle" };
const loading: Async<string> = { status: "loading" };
const shown: Async<string> = { status: "ready", data: "paper A" };
const reloading: Async<string> = { status: "reloading", data: "paper A" };

test("a first request is loading until it is answered or refused", () => {
	assert.deepEqual(asyncStep(idle, { kind: "ask" }), loading);
	assert.deepEqual(asyncStep(loading, { kind: "answer", data: "paper A" }), shown);
	assert.deepEqual(asyncStep(loading, { kind: "refuse", error: "not found" }), { status: "failed", error: "not found" });
});

test("asking about something else forgets the last answer", () => {
	assert.deepEqual(asyncStep(shown, { kind: "ask" }), loading);
	assert.equal(ready(asyncStep(shown, { kind: "ask" })), undefined);
	assert.deepEqual(asked(shown, true, false), loading, "new deps that say what is asked about");
});

test("asking again keeps the last answer in view until the new one comes", () => {
	assert.deepEqual(asyncStep(shown, { kind: "reask" }), reloading);
	assert.equal(ready(reloading), "paper A");
	assert.deepEqual(asyncStep(reloading, { kind: "answer", data: "paper A, newer" }), { status: "ready", data: "paper A, newer" });
	assert.deepEqual(asked(shown, true, true), reloading, "new deps that only say how fresh it should be");
});

test("a failed reload keeps what was shown beside the error", () => {
	const failed = asyncStep(reloading, { kind: "refuse", error: "The backend is not reachable" });
	assert.deepEqual(failed, { status: "failed", error: "The backend is not reachable", data: "paper A" });
	assert.equal(ready(failed), "paper A");
	// asking once more after that: still shown while it is asked
	assert.deepEqual(asyncStep(failed, { kind: "reask" }), reloading);
});

test("asking again after a first request failed is loading again, not stuck on the error", () => {
	const failed = asyncStep(loading, { kind: "refuse", error: "not found" });
	assert.deepEqual(asyncStep(failed, { kind: "reask" }), loading);
	assert.deepEqual(asyncStep(asyncStep(failed, { kind: "reask" }), { kind: "answer", data: "read" }), { status: "ready", data: "read" });
});

test("an answer or a refusal no request waits for changes nothing", () => {
	for (const state of [idle, shown, { status: "failed", error: "x" } as Async<string>]) {
		assert.equal(asyncStep(state, { kind: "answer", data: "late" }), state);
		assert.equal(asyncStep(state, { kind: "refuse", error: "late" }), state);
	}
});

test("nothing to load is idle, whatever was there", () => {
	assert.deepEqual(asked(shown, false, true), idle);
	assert.deepEqual(asked(loading, false, false), idle);
});

test("while loading or failed there is no answer to show, and never a spinner beside an error", () => {
	assert.equal(ready(loading), undefined);
	assert.equal(ready(idle), undefined);
	assert.equal(ready({ status: "failed", error: "x" }), undefined);
	const failed = asyncStep(reloading, { kind: "refuse", error: "x" });
	assert.notEqual(failed.status, "loading");
	assert.notEqual(failed.status, "reloading");
});
