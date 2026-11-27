/** The Weaknesses section: reviewers' scores on their scales, replies' attachment, points in their section, what was left out, the estimate. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { Critique, CritiqueEstimate, CritiqueReviewer, Evidence } from "@rp/contracts";
import { WeaknessesOf } from "@/components/critique";
import {
	absenceLine,
	attachmentNote,
	estimateLine,
	estimateTitle,
	leftOutLines,
	pointInSection,
	readingsByPoint,
	reviewerName,
	scoreLine,
	scoreTitle,
} from "@/lib/critique";

const evidence = (quote: string): Evidence => ({
	paper: "iclr-2024-1",
	quote,
	established: {
		kind: "verified",
		address: { page: 2, section: "4. Experiments", paragraph: 1, start: 10, end_page: 2, end: 60 },
		matched: { kind: "exact" },
	},
});

// every word of this review is made up
const reviewer: CritiqueReviewer = {
	note: 2,
	reviewer: "Uv78",
	heading: "Official Review by Reviewer Uv78",
	rating: { field: "Rating", value: 8, meaning: "accept, good paper", scale: { minimum: 1, maximum: 10, source: "published" } },
	confidence: { field: "Confidence", value: 4, meaning: "", scale: { minimum: 1, maximum: 5, source: "assumed" } },
	sections: [
		{ index: 5, name: "Weaknesses", text: "Résumé: the experiments are limited to small models.", span: { start: 100, end: 153 } },
	],
	points: [
		{ id: "p1", concern: "weakness", section: 5, text: "the experiments are limited to small models.", span: { start: 108, end: 153 } },
	],
	not_split: ["Strengths And Weaknesses"],
	replies: [
		{
			note: 3,
			heading: "Official Comment by Authors",
			attachment: { kind: "guessed", basis: "follows_review" },
			text: "We add a larger model.",
			cut: false,
			span: { start: 200, end: 230 },
		},
	],
};

const estimate: CritiqueEstimate = {
	items: 1,
	kept: 0,
	to_ask: 1,
	by_model: [
		{
			model: "some-model",
			calls: 1,
			tokens: { input: 900, output: 3500, cache_read: 0, cache_write_5m: 20000, cache_write_1h: 0 },
			usd: 0.17,
			most_usd: 0.35,
			counting: { kind: "assumed", chars_per_token: 3 },
		},
	],
	usd: 0.17,
	most_usd: 0.35,
};

test("a reviewer is named and scored on the venue's scale, and the scale says where it is from", () => {
	assert.equal(reviewerName(reviewer), "Reviewer Uv78");
	assert.equal(reviewerName({ reviewer: null }), "A reviewer");
	assert.equal(scoreLine(reviewer.rating!), "8 of 10 rating");
	assert.equal(scoreLine(reviewer.confidence!), "4 of 5 confidence");
	assert.match(scoreTitle(reviewer.rating!), /“accept, good paper”.*venue's own scale/);
	assert.match(scoreTitle(reviewer.confidence!), /assumed.*estimate/);
});

test("a reply says how it is attached, and a guess says it is one", () => {
	assert.equal(attachmentNote({ kind: "named" }), "attached by its title, which names this reviewer");
	assert.match(attachmentNote({ kind: "guessed", basis: "follows_review" }), /^attached by a guess: it follows this review/);
	assert.match(attachmentNote({ kind: "guessed", basis: "same_reviewer" }), /by this review's reviewer/);
});

test("a point is marked in its section, counting characters as the engine does", () => {
	const [before, marked, after] = pointInSection(reviewer.sections[0], reviewer.points[0]);
	assert.deepEqual([before, marked, after], ["Résumé: ", "the experiments are limited to small models.", ""]);
	const wide = { ...reviewer.sections[0], text: "𝐖 x: the point. Rest", span: { start: 0, end: 20 } };
	assert.deepEqual(pointInSection(wide, { ...reviewer.points[0], span: { start: 5, end: 15 } }), ["𝐖 x: ", "the point.", " Rest"]);
});

test("what was left out is counted, never said, and nothing is said when nothing was", () => {
	assert.deepEqual(leftOutLines({ not_in_paper: 0, malformed: 0, unknown_point: 0, beyond_limit: 0, verdict: 0 }), []);
	assert.deepEqual(leftOutLines({ not_in_paper: 3, malformed: 1, unknown_point: 2, beyond_limit: 1, verdict: 1 }), [
		"3 statements left out: their quotes were not found in the paper.",
		"1 entry of the answer left out: not in the form asked for.",
		"2 entries left out: about no reviewer's point that was asked about.",
		"1 weakness past the first eight left out.",
		"1 statement left out: worded as a judgement of the paper, which this section does not give.",
	]);
});

test("an absence is always one to check, with what was searched for", () => {
	assert.equal(absenceLine({ kind: "not_claimed" }), null);
	assert.match(absenceLine({ kind: "none_found", searched: ["ablation", "seeds"] })!, /check yourself.*“ablation”, “seeds” and found none/);
	assert.match(
		absenceLine({ kind: "but_see", searched: ["ablation"], found: [evidence("an ablation of the router is in Table 3")] })!,
		/But .*see:$/,
	);
	assert.match(absenceLine({ kind: "not_searched" })!, /check yourself/);
});

test("the estimate is said in dollars with the code, as an estimate, and not at all when the price is unknown", () => {
	assert.equal(estimateLine(estimate, "en-US"), "about USD 0.1700, at most USD 0.3500");
	assert.equal(estimateLine({ ...estimate, usd: null, most_usd: null }), null);
	assert.match(estimateTitle(estimate), /An estimate, in US dollars.*assumed.*nothing is sent until you click/);
});

test("the model's reading is apart from the reviewers', hidden until asked for, and the section gives no score or verdict", () => {
	const critique: Critique = {
		paper_id: "iclr-2024-1",
		reviewers: {
			state: "read",
			address: "https://openreview.net/forum?id=x",
			read_at: "2026-11-27T10:00:00Z",
			reviewers: [reviewer],
			points: 1,
		},
		model: {
			state: "answered",
			model: "some-model",
			kept_at: "2026-11-27T11:00:00Z",
			points_asked: 1,
			cut: null,
			reading: {
				points: [{ point: "p1", location: null, paper_says: evidence("we add experiments on a larger model in the appendix") }],
				weaknesses: [
					{
						kind: "missing_ablation",
						weakness: "No ablation of the step size.",
						about: evidence("the step size is fixed for all runs"),
						absence: { kind: "none_found", searched: ["ablation"] },
					},
				],
				left_out: { not_in_paper: 2, malformed: 0, unknown_point: 0, beyond_limit: 0, verdict: 0 },
			},
		},
	};
	assert.equal(readingsByPoint(critique.model.state === "answered" ? critique.model.reading : null).get("p1")?.point, "p1");
	const hidden = renderToStaticMarkup(<WeaknessesOf critique={critique} shown={false} onShow={() => {}} onChanged={() => {}} />);
	assert.match(hidden, /What the reviewers raised/);
	assert.match(hidden, /8 of 10 rating/);
	assert.match(hidden, /The authors replied to this review \(attached by a guess/);
	assert.match(hidden, /The model&#x27;s reading, not a review/);
	assert.doesNotMatch(hidden, /No ablation of the step size/, "the model's reading is not shown by default");
	assert.doesNotMatch(hidden, /The paper says/);
	const shown = renderToStaticMarkup(<WeaknessesOf critique={critique} shown onShow={() => {}} onChanged={() => {}} />);
	assert.match(shown, /No ablation of the step size/);
	assert.match(shown, /The paper says: “we add experiments on a larger model in the appendix”/);
	assert.match(shown, /2 statements left out: their quotes were not found in the paper/);
	assert.match(shown, /Absence: check yourself/);
	// the model's weaknesses come after the reviewers' points, under their own heading
	assert.ok(shown.indexOf("No ablation of the step size") > shown.indexOf("The model&#x27;s reading, not a review"));
	assert.ok(shown.indexOf("the experiments are limited to small models.") < shown.indexOf("The model&#x27;s reading, not a review"));
	// no verdict of the model's: only the reviewer's own meaning of their rating says "accept"
	for (const verdict of [/\breject/i, /resolved/i, /quality/i, /\bverdict\b(?<!no verdict)/i]) {
		assert.doesNotMatch(shown.replace(/no score, no verdict|no score and no verdict|none is\s+a verdict/gi, ""), verdict);
	}
});

test("a paper with no reviews says how to get them, and one with no PDF says why the model's part is missing", () => {
	const none = renderToStaticMarkup(
		<WeaknessesOf
			critique={{ paper_id: "2601.00001", reviewers: { state: "none_kept" }, model: { state: "no_pdf" } }}
			shown={false}
			onShow={() => {}}
			onChanged={() => {}}
		/>,
	);
	assert.match(none, /No reviews are kept for this paper/);
	assert.match(none, /Reviews tab/);
	assert.match(none, /needs the paper&#x27;s PDF/);
	const ask = renderToStaticMarkup(
		<WeaknessesOf
			critique={{
				paper_id: "2601.00001",
				reviewers: { state: "none_kept" },
				model: { state: "not_asked", model: "some-model", estimate, api_key_set: true, points_asked: 0, cut: null },
			}}
			shown={false}
			onShow={() => {}}
			onChanged={() => {}}
		/>,
	);
	assert.match(ask, /Ask the model \(some-model\): about USD 0\.1700, at most USD 0\.3500, an estimate/);
});
