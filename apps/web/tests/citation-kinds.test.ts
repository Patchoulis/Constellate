/** Citing sentences and the kinds of citation, as they are said and drawn (lib/citation-kinds.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { CitationKindsEstimate, CitationTally, CitingSentenceShown, Evidence, PaperCitationContexts } from "@rp/contracts";
import { batchCost } from "./costs";
import {
	budgetFor,
	coverageLine,
	edgeKey,
	edgeKinds,
	groupingNote,
	judgementLabel,
	lineKind,
	mainKind,
	moreToRead,
	sectionName,
	sentencesOfKind,
	tallyLine,
	tokensPerSentence,
} from "@/lib/citation-kinds";

function tally(rest: Partial<CitationTally> = {}): CitationTally {
	return { sentences: 0, papers: 0, kinds: {}, mentions: 0, not_justified: 0, not_classified: 0, ...rest };
}

const evidence: Evidence = {
	paper: "seed",
	quote: "We compare against TIES as a baseline.",
	established: {
		kind: "verified",
		address: { page: 3, section: "4 Experiments", paragraph: 1, start: 10, end_page: 3, end: 48 },
		matched: { kind: "exact" },
	},
};

function sentence(judgement: CitingSentenceShown["judgement"]): CitingSentenceShown {
	return { evidence, before: null, after: null, section: { kind: "experiments" }, grouping: { kind: "alone" }, written: "[4]", judgement };
}

test("a tally is said as counts of citing sentences that add up to the first", () => {
	const cited = tally({ sentences: 14, papers: 9, kinds: { baseline: 6, uses: 3, contrasts: 1 }, mentions: 2, not_classified: 2 });
	assert.equal(tallyLine(cited), "cited 14 times in this library: 6 as a baseline, 3 used, 1 contrasted, 2 mentions, 2 not classified");
	assert.equal(
		tallyLine(tally({ sentences: 2, mentions: 1, not_justified: 1 })),
		"cited 2 times in this library: 1 mention, 1 not justified (counted as mentions)",
	);
	assert.equal(tallyLine(tally({ sentences: 1, not_classified: 1 })), "cited 1 time in this library: 1 not classified");
});

test("a sentence's reading says whether its words were found, and a reading not justified is no kind", () => {
	assert.equal(judgementLabel({ state: "justified", kind: "baseline", because: evidence }).kind, "baseline");
	const unjustified = judgementLabel({ state: "not_justified", claimed: "contrasts" });
	assert.equal(unjustified.kind, null);
	assert.match(unjustified.label, /not justified/);
	assert.match(unjustified.note, /not found/);
	assert.equal(judgementLabel({ state: "not_classified" }).label, "not classified");
});

test("the sentences behind a count are those read as its kind; mentions include those not justified", () => {
	const sentences = [
		sentence({ state: "justified", kind: "baseline", because: evidence }),
		sentence({ state: "mention" }),
		sentence({ state: "not_justified", claimed: "uses" }),
		sentence({ state: "not_classified" }),
	];
	assert.equal(sentencesOfKind(sentences, "baseline").length, 1);
	assert.equal(sentencesOfKind(sentences, "mention").length, 2);
	assert.equal(sentencesOfKind(sentences, "uses").length, 0, "a reading not justified is not counted as its kind");
	assert.equal(sentencesOfKind(sentences, null).length, 4);
});

test("sections, groups and coverage are said in words with their units", () => {
	assert.equal(sectionName({ kind: "related_work" }), "Related work");
	assert.equal(sectionName({ kind: "experiments" }), "Experiments");
	assert.equal(sectionName({ kind: "other", title: "3 Spectral Calibration" }), "“3 Spectral Calibration”");
	assert.equal(groupingNote({ kind: "alone" }), "cited alone");
	assert.equal(groupingNote({ kind: "grouped", with: 3 }), "cited with 3 other works");
	assert.equal(
		coverageLine({ papers: 739, read: 212, without_pdf: 480 }),
		"Citing sentences read for 212 of 739 papers of this run: 480 have no PDF kept",
	);
	assert.equal(moreToRead({ papers: 739, read: 212, without_pdf: 480 }), true);
	assert.equal(moreToRead({ papers: 739, read: 259, without_pdf: 480 }), false);
});

test("papers whose PDF cannot be read are not waiting to be read, and are said", () => {
	// 739 papers: 480 without a PDF, 7 with one that cannot be read, the other 252 read
	const coverage = { papers: 739, read: 252, without_pdf: 480, unreadable: 7 };
	assert.equal(moreToRead(coverage), false, "nothing left that reading again would read");
	assert.equal(moreToRead({ ...coverage, read: 251 }), true);
	assert.equal(
		coverageLine(coverage),
		"Citing sentences read for 252 of 739 papers of this run: 480 have no PDF kept, 7 have one that cannot be read",
	);
	assert.equal(moreToRead({ papers: 10, read: 9, without_pdf: 1 }), false, "an older engine that does not count them");
});

test("the network's lines take the main justified kind of the citations they stand for", () => {
	assert.equal(mainKind(tally({ kinds: { uses: 1, baseline: 3 } })), "baseline");
	assert.equal(mainKind(tally({ kinds: { uses: 2, baseline: 2 } })), "baseline", "a tie goes to the first listed");
	assert.equal(mainKind(tally({ mentions: 5 })), null);
	const between = (paper: string, kinds: CitationTally["kinds"]) => ({ paper, title: paper, tally: tally({ kinds }), sentences: [] });
	const contexts: PaperCitationContexts = {
		paper: "seed",
		state: "read",
		references_listed: 3,
		cites: [between("ties", { baseline: 1 }), between("arith", {})],
		cited_by: [between("later", { extends: 2 })],
		cited_by_total: 1,
		coverage: null,
		left_out: 0,
		model: "m",
	};
	const kinds = edgeKinds(contexts);
	assert.deepEqual(
		[...kinds.entries()],
		[
			[edgeKey("seed", "ties"), "baseline"],
			[edgeKey("later", "seed"), "extends"],
		],
	);
	const works = [
		{ key: "paper:seed", side: "seed" as const },
		{ key: "paper:ties", side: "prior" as const },
		{ key: "paper:later", side: "derivative" as const },
		{ key: "arxiv:2401.00001", side: "prior" as const },
	];
	assert.equal(lineKind(works, { from: 0, to: 1, seed: true }, kinds), "baseline");
	assert.equal(lineKind(works, { from: 0, to: 2, seed: true }, kinds), "extends", "a citing paper's line runs to the seed");
	assert.equal(lineKind(works, { from: 0, to: 3, seed: true }, kinds), null, "a work outside the library has no sentences");
	assert.equal(lineKind(works, { from: 2, to: 1, seed: false }, kinds), null);
	assert.equal(lineKind(works, { from: 0, to: 1, seed: true }, undefined), null);
});

test("a job is capped at the estimate's worst case, rounded up to the cent", () => {
	const estimate = (most: number | null, toAsk: number): CitationKindsEstimate => ({
		scope: "sample",
		provider: "anthropic",
		model: "m",
		sentences: 50,
		kept: 0,
		to_ask: toAsk,
		estimate: batchCost(most === null ? null : 0.05, most, 2, {
			tokens: { input: 9_000, output: 3_000, cache_read: 0, cache_write_5m: 3_000, cache_write_1h: 0 },
		}),
		can_ask: true,
	});
	assert.deepEqual(budgetFor(estimate(0.1234, 50)), { budget_usd: 0.13 });
	assert.deepEqual(budgetFor(estimate(0.0001, 50)), { budget_usd: 0.01 });
	assert.deepEqual(budgetFor(estimate(null, 50)), { budget_tokens: 28_000 }, "no price: capped in tokens, still started");
	assert.equal(tokensPerSentence(estimate(1, 50)), 300);
	assert.equal(tokensPerSentence(estimate(1, 0)), null);
});
