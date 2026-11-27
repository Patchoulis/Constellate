/** Where papers stand on a question: counted with what they are counts of, broken down, never one verdict. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Evidence, GraphNode, StanceCostEstimate, StanceFilter, StancePlaced, StanceResult, StanceRow } from "@rp/contracts";
import { breakdown, costLine, countsLine, coverageLine, groupSets, skippedLine, starter, tally } from "@/lib/stances";

const evidence: Evidence = {
	paper: "p",
	quote: "pre-training on synthetic data improves downstream accuracy",
	established: { kind: "abstract", field: "abstract", start: 0, end: 59, matched: { kind: "exact" } },
};

function row(paper: string, placed: StancePlaced, tier: StanceRow["tier"] = null, year: number | null = 2025): StanceRow {
	return { paper, title: `title of ${paper}`, tier, year, read: "abstract", placed, in_sample: false };
}

const rows: StanceRow[] = [
	row("a", { group: "supports", evidence: [evidence] }, "oral", 2024),
	row("b", { group: "supports", evidence: [evidence] }, "poster", 2025),
	row("c", { group: "mixed", condition: "with enough data", evidence: [evidence] }, "poster", 2025),
	row("d", { group: "contradicts", evidence: [evidence] }, null, null),
	row("e", { group: "not_addressed" }, "poster", 2024),
	row("f", { group: "not_verified", quotes_not_found: 2 }, "oral", 2024),
];

test("the counts name what they count, and papers not verified are counted apart", () => {
	const counts = tally(rows);
	assert.equal(counts.supports, 2);
	assert.equal(
		countsLine(counts),
		"2 papers support, 1 paper is mixed, 1 paper contradicts, 1 paper does not address it; 1 paper not verified",
	);
	const none = tally([]);
	assert.equal(countsLine(none), "0 papers support, 0 papers are mixed, 0 papers contradict, 0 papers do not address it");
});

test("the placements are broken down by acceptance tier, orals first, and by year, oldest first", () => {
	const tiers = breakdown(rows, "tier");
	assert.deepEqual(
		tiers.map((line) => [line.label, line.papers]),
		[
			["Orals", 2],
			["Posters", 3],
			["Not from a conference", 1],
		],
	);
	assert.equal(tiers[0].counts.not_verified, 1, "a paper not verified stays apart in its tier too");
	assert.deepEqual(
		breakdown(rows, "year").map((line) => line.label),
		["2024", "2025", "Year not known"],
	);
});

test("what the pre-filter skipped is said with its denominator and its words", () => {
	const filter: StanceFilter = { papers: 739, related: 327, unrelated: 412, include_unrelated: false, terms: ["synthetic", "pre"] };
	assert.equal(
		skippedLine(filter),
		"412 of 739 papers skipped: nothing in their title or abstract shares a word with the question (synthetic, pre).",
	);
	assert.match(skippedLine({ ...filter, include_unrelated: true }) ?? "", /included as you asked/);
	assert.equal(skippedLine({ ...filter, unrelated: 0 }), null);
});

test("coverage says how many papers are still to ask", () => {
	const filter: StanceFilter = { papers: 50, related: 45, unrelated: 5, include_unrelated: false, terms: [] };
	const result: StanceResult = {
		id: "x",
		question: "q?",
		asked_at: "",
		model: "m",
		filter,
		to_ask: 45,
		answered: 10,
		sample: 10,
		rows: [],
	};
	assert.equal(coverageLine(result), "Answers for 10 of 45 papers to ask: 35 papers not asked yet.");
	assert.equal(coverageLine({ ...result, answered: 45 }), "Answers for 45 of 45 papers to ask.");
});

test("a cost is shown with its currency and as an estimate, or as nothing to pay", () => {
	const estimate: StanceCostEstimate = { items: 10, kept: 0, to_ask: 10, by_model: [], usd: 0.42, most_usd: 1.1 };
	const usd = { code: "USD", rate: 1, date: null };
	assert.equal(costLine(estimate, usd, "en-US"), "about USD 0.4200, at most USD 1.10 (an estimate)");
	assert.equal(costLine({ ...estimate, to_ask: 0 }, usd, "en-US"), "nothing to pay: every answer is kept");
	assert.match(costLine({ ...estimate, usd: null, most_usd: null }, usd, "en-US"), /price not known/);
});

test("each group's papers are found on the graph by id", () => {
	const nodes = ["x", "a", "c", "f"].map((id) => ({ id }) as GraphNode);
	const sets = groupSets(rows, nodes);
	assert.deepEqual([...(sets.get("supports") ?? [])], [1]);
	assert.deepEqual([...(sets.get("mixed") ?? [])], [2]);
	assert.deepEqual([...(sets.get("not_verified") ?? [])], [3]);
	assert.equal(sets.get("contradicts"), undefined, "a paper not in the run is not drawn");
	assert.equal(starter("synthetic data"), "Does synthetic data help?");
});
