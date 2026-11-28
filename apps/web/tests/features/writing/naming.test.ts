/** Naming a made run's communities: what is said before the click, and what the click sends. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Community, CommunitySection, DescribeEstimate } from "@rp/contracts";
import {
	estimateQuery,
	isPlain,
	leftOutWords,
	namingRequest,
	namingWords,
	plainOf,
	sectionsOf,
	unnamedOf,
} from "@/lib/features/writing/naming";

const estimate = (more: Partial<DescribeEstimate["estimate"]> = {}): DescribeEstimate => ({
	provider: "openai",
	model: "a-model",
	communities: 24,
	named: 0,
	plain: 0,
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
	assert.deepEqual(namingRequest(estimate(), "name"), { provider: "openai", model: "a-model", accepted_usd: 0.25 });
	assert.deepEqual(namingRequest(estimate({ most_usd: null, usd: null }), "again"), {
		provider: "openai",
		model: "a-model",
		accepted_tokens: 40_000,
		redo: true,
	});
});

const section: CommunitySection = {
	heading: "Problem",
	text: "Papers on routers.",
	cites: [
		{
			title: "A title",
			evidence: {
				paper: "p1",
				quote: "We balance the router",
				established: { kind: "abstract", field: "abstract", start: 0, end: 21, matched: { kind: "exact" } },
			},
		},
	],
};
const described = (name: string | null, sections?: CommunitySection[], children: Community[] = []) =>
	({ name, description: name && "About it.", ...(sections ? { sections } : {}), children }) as unknown as Community;

test("a community named before sections were asked for is plain; one with sections, even none shown, is not", () => {
	assert.equal(isPlain(described("Routers")), true);
	assert.equal(isPlain(described("Routers", [section])), false);
	assert.equal(isPlain(described("Routers", [])), false, "rewritten: its sections were all left out");
	assert.equal(isPlain(described(null)), false, "not described at all: naming, not rewriting, is for it");
	assert.deepEqual(plainOf([described("a", undefined, [described("b", [section]), described("c")]), described(null)]), {
		communities: 4,
		plain: 2,
	});
});

test("sections are shown when there are some, and what was left out is said with its count", () => {
	assert.deepEqual(sectionsOf(described("a", [section])), [section]);
	assert.equal(sectionsOf(described("a", [])), null);
	assert.equal(sectionsOf(described("a")), null);
	assert.equal(leftOutWords(undefined), null);
	assert.equal(leftOutWords(0), null);
	assert.equal(leftOutWords(1), "1 section left out: its quotes were not found in the papers.");
	assert.equal(leftOutWords(3), "3 sections left out: their quotes were not found in the papers.");
});

test("rewriting plain descriptions has its own estimate, words and request", () => {
	assert.equal(estimateQuery("name"), "");
	assert.equal(estimateQuery("sections"), "?add_sections=true");
	assert.equal(estimateQuery("again"), "?redo=true");
	const words = namingWords({ ...estimate({ to_ask: 5, kept: 2 }), named: 20, plain: 12 }, undefined, undefined, "sections");
	assert.equal(words.counted, "24 communities, 12 of them with a plain description and no sections.");
	assert.match(
		words.asked,
		/^5 calls to a-model, each about one community or the two parts of one \(2 answers kept from before cost nothing\)\./,
	);
	assert.equal(words.cost, "about USD 0.1000, at most USD 0.2500");
	assert.equal(words.nothing, false);
	const none = namingWords({ ...estimate({ to_ask: 0, kept: 7 }), plain: 0 }, undefined, undefined, "sections");
	assert.deepEqual([none.nothing, none.cost], [true, null]);
	assert.equal(none.counted, "24 communities, none of them with a plain description and no sections.");
	assert.deepEqual(namingRequest(estimate(), "sections"), { provider: "openai", model: "a-model", accepted_usd: 0.25, add_sections: true });
});
