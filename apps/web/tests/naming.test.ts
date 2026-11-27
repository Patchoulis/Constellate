/** Naming a made run's communities: what is said before the click, and what the click sends. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Community, DescribeEstimate } from "@rp/contracts";
import { namingRequest, namingWords, unnamedOf } from "@/lib/naming";

const estimate = (more: Partial<DescribeEstimate["estimate"]> = {}): DescribeEstimate => ({
	provider: "openai",
	model: "a-model",
	communities: 24,
	named: 0,
	api_key_set: true,
	estimate: {
		provider: "openai",
		model: "a-model",
		calls: 24,
		tokens: { input: 1000, output: 100, cache_read: 0, cache_write_5m: 0, cache_write_1h: 0 },
		usd: 0.1,
		most_usd: 0.25,
		most_tokens: 40_000,
		counting: { kind: "assumed", chars_per_token: 4 },
		caching: "not_reported",
		on_this_computer: false,
		items: 24,
		kept: 0,
		to_ask: 24,
		...more,
	},
});

const community = (name: string | null, children: Community[] = []) => ({ name, children }) as unknown as Community;

test("a run's unnamed communities are counted with their sub-communities", () => {
	assert.deepEqual(unnamedOf([community(null, [community("a"), community(null)]), community("b")]), { communities: 4, unnamed: 2 });
});

test("a priced model's cost is said in money, with how many communities are to be named", () => {
	const words = namingWords(estimate());
	assert.equal(words.counted, "24 communities, none of them named.");
	assert.equal(words.asked, "24 communities to name with a-model.");
	assert.equal(words.cost, "about USD 0.1000, at most USD 0.2500");
});

test("a model with no price is said in tokens only, and one on this computer says it has no API cost", () => {
	assert.equal(namingWords(estimate({ usd: null, most_usd: null })).cost, "at most 40k tokens");
	assert.equal(
		namingWords(estimate({ usd: null, most_usd: null, on_this_computer: true })).cost,
		"at most 40k tokens, runs on this computer: no API cost",
	);
});

test("names kept from before are said to cost nothing, and with nothing to ask no cost is shown", () => {
	assert.match(
		namingWords(estimate({ kept: 3, to_ask: 21 })).asked,
		/21 communities to name with a-model \(3 names kept from before cost nothing\)/,
	);
	const none = namingWords(estimate({ kept: 24, to_ask: 0 }));
	assert.equal(none.nothing, true);
	assert.equal(none.cost, null);
});

test("the click sends the model shown and its most, in dollars, or in tokens for a model with no price", () => {
	assert.deepEqual(namingRequest(estimate(), false), { provider: "openai", model: "a-model", accepted_usd: 0.25 });
	assert.deepEqual(namingRequest(estimate({ most_usd: null, usd: null }), true), {
		provider: "openai",
		model: "a-model",
		accepted_tokens: 40_000,
		redo: true,
	});
});
