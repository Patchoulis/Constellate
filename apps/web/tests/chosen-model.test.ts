/** Which estimate may be accepted: only the one answered for what is asked now and the model chosen now. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { choiceKey, chosenModelOf, currentEstimate, isFor } from "@/lib/chosen-model";
import type { Async } from "@/lib/use-async";
import { askingCost } from "./costs";

const chosen = { provider: "anthropic" as const, model: "some-model", effort: "medium" as const };
const estimate = askingCost(0.12, 0.25);

test("an estimate is for the model chosen when it names the same provider and model", () => {
	assert.equal(isFor(estimate, chosen), true);
	assert.equal(isFor(estimate, { ...chosen, model: "another" }), false);
	assert.equal(isFor(estimate, { ...chosen, provider: "openai" }), false, "the same id at another provider is another model");
	assert.equal(isFor(estimate, null), true, "the choice not read yet: the engine's own is all there is");
	assert.equal(isFor(estimate, { ...chosen, provider: null }), true, "a backend that names no provider");
});

test("an estimate being asked for again, or that failed, is not offered to accept", () => {
	const ready: Async<typeof estimate> = { status: "ready", data: estimate };
	assert.equal(currentEstimate(ready, chosen), estimate);
	assert.equal(currentEstimate({ status: "reloading", data: estimate }, chosen), undefined);
	assert.equal(currentEstimate({ status: "failed", error: "busy", data: estimate }, chosen), undefined);
	assert.equal(currentEstimate({ status: "loading" }, chosen), undefined);
	assert.equal(currentEstimate({ status: "idle" }, chosen), undefined);
});

test("an estimate for the model chosen before is not offered once another is chosen", () => {
	assert.equal(currentEstimate({ status: "ready", data: estimate }, { ...chosen, provider: "openai", model: "gpt" }), undefined);
});

test("the choice is one dependency that changes with the provider, the model or the effort", () => {
	const keys = new Set([
		choiceKey(chosen),
		choiceKey({ ...chosen, provider: "openai" }),
		choiceKey({ ...chosen, model: "other" }),
		choiceKey({ ...chosen, effort: "high" }),
		choiceKey(null),
	]);
	assert.equal(keys.size, 5);
	assert.equal(choiceKey({ ...chosen }), choiceKey(chosen));
});

test("the choice is read from the settings, and is not known before them", () => {
	assert.equal(chosenModelOf(undefined), null);
	assert.deepEqual(chosenModelOf({ describe_model: "claude-opus-5-5" }), { provider: null, model: "claude-opus-5-5", effort: null });
	assert.deepEqual(chosenModelOf({ describe_provider: "openai", describe_model: "gpt", describe_effort: "high" }), {
		provider: "openai",
		model: "gpt",
		effort: "high",
	});
});
