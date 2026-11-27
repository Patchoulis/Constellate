/** Training the AI-text detector: what is said before the click and while it trains. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { DetectorEstimate } from "@rp/contracts";
import { DOLLAR } from "@/lib/cost";
import { detectorEstimateLines, trainingLine } from "@/lib/detector";

const estimate = (more: Partial<DetectorEstimate> = {}, cost: Partial<DetectorEstimate["cost"]> = {}): DetectorEstimate => ({
	provider: "openai",
	model: "a-model",
	api_key_set: true,
	pool_kept: 400,
	pool: 600,
	mirrors_kept: 0,
	to_generate: 600,
	built: false,
	cost: {
		provider: "openai",
		model: "a-model",
		calls: 600,
		tokens: { input: 1000, output: 100, cache_read: 0, cache_write_5m: 0, cache_write_1h: 0 },
		usd: 0.4,
		most_usd: 0.9,
		most_tokens: 900_000,
		counting: { kind: "assumed", chars_per_token: 4 },
		caching: "not_reported",
		on_this_computer: false,
		items: 600,
		kept: 0,
		to_ask: 600,
		...cost,
	},
	...more,
});

test("the estimate says how many abstracts which model would write, and what that costs, as an estimate", () => {
	const words = detectorEstimateLines(estimate());
	assert.match(words.lines.join(" "), /600 human abstracts of 2019, of which 400 are kept already/);
	assert.match(words.lines.join(" "), /a-model would write up to 600 abstracts: the text it learns to recognise/);
	assert.equal(words.cost, "It costs about USD 0.4000, at most USD 0.9000 (an estimate).");
	assert.equal(words.cannot, null);
});

test("a model with no price is said in tokens, and without a key nothing can be started", () => {
	assert.equal(detectorEstimateLines(estimate({}, { usd: null, most_usd: null })).cost, "It costs at most 900k tokens (an estimate).");
	const keyless = detectorEstimateLines(estimate({ api_key_set: false }));
	assert.equal(keyless.cost, null);
	assert.match(keyless.cannot ?? "", /No key is set/);
});

test("with everything written before, training again asks nothing and shows no cost", () => {
	const again = detectorEstimateLines(estimate({ to_generate: 0, built: true }));
	assert.equal(again.free, true);
	assert.equal(again.cost, null);
	assert.match(again.lines.join(" "), /costs nothing/);
});

test("while it trains, the abstracts written are counted and what was spent is said only when known", () => {
	assert.equal(
		trainingLine("build", { done: 120, of: 600, usd: 0.08 }, DOLLAR),
		"Training the detector: 120 of 600 abstracts written (USD 0.0800 spent so far)",
	);
	assert.equal(trainingLine("build", { done: 0, of: 600, usd: 0 }, DOLLAR), "Training the detector: 0 of 600 abstracts written");
	assert.match(trainingLine("years", null, DOLLAR), /no cost/);
});
