/** Which estimate may be accepted: only the one answered for what is asked now and the model chosen now. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ModelCatalog, ModelProvider } from "@rp/contracts";
import { choiceKey, chosenModelOf, chosenModelsOf, currentEstimate, isFor } from "@/lib/chosen-model";
import type { Async } from "@/lib/use-async";
import { askingCost } from "./costs";

const chosen = { provider: "anthropic" as const, model: "some-model", effort: "medium" as const };
const estimate = askingCost(0.12, 0.25);

test("an estimate is for the model chosen when it names the same provider and model", () => {
	assert.equal(isFor(estimate, chosen), true);
	assert.equal(isFor(estimate, { ...chosen, model: "another" }), false);
	assert.equal(isFor(estimate, { ...chosen, provider: "openai" }), false, "the same id at another provider is another model");
	assert.equal(isFor(estimate, null), true, "the choice not read yet: the engine's own is all there is");
	assert.equal(isFor(estimate, { ...chosen, provider: null }), true, "an engine that names no provider");
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

/** A catalogue of two providers as the engine tells their defaults for reading: the Claude API's Haiku, none for OpenAI. */
function catalogue(): ModelCatalog {
	const row = (id: "anthropic" | "openai", reading_default: ModelProvider["reading_default"]): ModelProvider => ({
		id,
		label: id,
		key: { variable: "", needed: true, set: true },
		address: { variable: null, host: null, place: "provider" },
		available: { state: "ready" },
		ids: "listed",
		capabilities: { answers: "enforced", effort: "by_model", caching: "marked", counting: "endpoint" },
		limits: [],
		models: [],
		price_of_typed: { kind: "not_known" },
		docs: "",
		reading_default,
	});
	return {
		providers: [row("anthropic", { model: "claude-haiku-5-5", effort: "medium" }), row("openai", null)],
		chosen: { provider: "anthropic", model: "claude-opus-5-5", effort: "medium" },
	};
}

test("by default the Claude API's Haiku reads papers, known only from the engine's catalogue", () => {
	const claude = { describe_provider: "anthropic" as const, describe_model: "claude-opus-5-5", describe_effort: "high" as const };
	const both = chosenModelsOf(claude, catalogue());
	assert.deepEqual(both.reading, { provider: "anthropic", model: "claude-haiku-5-5", effort: "medium" });
	assert.deepEqual(both.naming, { provider: "anthropic", model: "claude-opus-5-5", effort: "high" });
	// an estimate shown for the naming model is not the one for the default
	const opus = askingCost(0.12, 0.25, { provider: "anthropic", model: "claude-opus-5-5" });
	assert.equal(currentEstimate({ status: "ready", data: opus }, both.reading), undefined);
	// the catalogue not told yet: not known (any estimate the engine answers is its own), never guessed
	assert.equal(chosenModelsOf(claude).reading, null);
	const same = chosenModelsOf({ ...claude, reading_same_as_naming: true }, catalogue());
	assert.deepEqual(same.reading, { provider: "anthropic", model: "claude-opus-5-5", effort: "high" });
});

test("the features that read papers follow the model that reads papers, the naming one until another is chosen", () => {
	const naming = { describe_provider: "openai" as const, describe_model: "gpt", describe_effort: "high" as const };
	assert.deepEqual(chosenModelOf(naming, "reading", catalogue()), { provider: "openai", model: "gpt", effort: "high" });
	const own = { ...naming, reading_provider: "compatible" as const, reading_model: "qwen3:32b", reading_effort: null };
	const both = chosenModelsOf(own);
	assert.deepEqual(both.reading, { provider: "compatible", model: "qwen3:32b", effort: "high" }, "no effort of its own: the naming one's");
	assert.deepEqual(both.naming, { provider: "openai", model: "gpt", effort: "high" });
	assert.notEqual(choiceKey(both.reading), choiceKey(both.naming), "a new reading model asks the estimates again");
	// an estimate seen for the naming model is not offered once another reads papers
	const seen = askingCost(0.12, 0.25, { provider: "openai", model: "gpt" });
	assert.equal(currentEstimate({ status: "ready", data: seen }, both.naming), seen);
	assert.equal(currentEstimate({ status: "ready", data: seen }, both.reading), undefined);
	assert.deepEqual(chosenModelsOf(undefined), { naming: null, reading: null });
});
