/** The Ask box: what it says about coverage, cost and what was left out, and its state machine. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { AnswerEstimate, AnswerSummary, AnswerView, PassageFound, PassagesFound, SearchCoverage } from "@rp/contracts";
import { askingCost } from "./costs";
import {
	cannotAsk,
	costLine,
	costTitle,
	coverageLine,
	excerpt,
	historyOf,
	indexingLine,
	leftOutLine,
	missingRoute,
	reduceAsk,
	splitPassages,
	stateNote,
	type AskState,
} from "@/lib/answers";

const coverage: SearchCoverage = { papers: 739, full_text: 212, capped: 0, no_pdf: 520, unreadable: 7, missing: 0 };

const estimate: Extract<AnswerEstimate, { state: "ready" }> = {
	state: "ready",
	id: "a".repeat(64),
	provider: "anthropic",
	model: "claude-opus-5-5",
	kept: false,
	passages: 38,
	papers: 21,
	coverage,
	prompt_cap_tokens: 24_000,
	max_answer_tokens: 4_000,
	cost: askingCost(0.1371, 0.2271, {
		model: "claude-opus-5-5",
		tokens: { input: 12, output: 1_500, cache_read: 0, cache_write_5m: 20_000, cache_write_1h: 0 },
		most_tokens: 24_012,
	}),
	most_usd: 0.2271,
	most_tokens: 24_012,
	api_key_set: true,
};

const passage = (paper: string, cited: boolean): PassageFound => ({
	evidence: {
		paper,
		quote: "merging counts shared directions twice",
		established: { kind: "abstract", field: "abstract", start: 0, end: 38, matched: { kind: "exact" } },
	},
	cited,
});

const view = (state: AnswerView["state"], counts = { written: 4, shown: 3, not_found: 1, malformed: 0 }): AnswerView => ({
	id: "a".repeat(64),
	question: "Does merging count shared directions twice?",
	community: "c0",
	model: "claude-opus-5-5",
	asked_at: "2026-11-27T10:00:00Z",
	state,
	statements: [],
	counts,
	passages: [passage("p1", true), passage("p2", false)],
	cited_papers: ["p1"],
});

test("the coverage says what was searched in full and what by its abstract only", () => {
	assert.equal(
		coverageLine(coverage),
		"Searched the full text of 212 of 739 papers; 527 papers by title and abstract only (520 papers with no PDF kept, 7 papers whose PDF could not be read)",
	);
	assert.equal(coverageLine({ ...coverage, no_pdf: 0, unreadable: 0, capped: 3, missing: 1, papers: 216 }).split("; ").length, 3);
	assert.equal(indexingLine(200, 739), "Indexing the papers: 200 of 739 papers read");
});

test("the cost is shown in dollars before asking, with the tokens and that it is an estimate on hover", () => {
	assert.equal(costLine(estimate), "Ask the model: about USD 0.1371, at most USD 0.2271");
	assert.match(costTitle(estimate), /38 passages from 21 papers/);
	assert.match(costTitle(estimate), /at most 24k tokens/);
	assert.match(costTitle(estimate), /An estimate/);
	assert.equal(costLine({ ...estimate, kept: true, cost: null }), "Open the kept answer: no cost");
	// a model with no price: no dollar figure at all, only the tokens it can take
	const unpriced = { ...estimate, cost: { ...estimate.cost!, usd: null, most_usd: null }, most_usd: null };
	assert.equal(costLine(unpriced), "Ask the model: at most 24k tokens");
	assert.doesNotMatch(costLine(unpriced), /USD|not known|price/);
	const here = { ...unpriced, cost: { ...unpriced.cost, provider: "compatible" as const, on_this_computer: true } };
	assert.equal(costLine(here), "Ask the model: at most 24k tokens, runs on this computer: no API cost");
	assert.equal(cannotAsk(estimate), null);
	assert.match(cannotAsk({ ...estimate, passages: 0 }) ?? "", /nothing to answer it from/);
	assert.match(cannotAsk({ ...estimate, api_key_set: false }) ?? "", /key is not set/);
	assert.equal(cannotAsk({ ...estimate, kept: true, api_key_set: false }), null, "a kept answer needs no key");
});

test("what was left out is counted, never shown", () => {
	assert.equal(
		leftOutLine({ written: 4, shown: 3, not_found: 1, malformed: 0 }),
		"1 statement left out: their quotes were not found in the papers",
	);
	assert.equal(
		leftOutLine({ written: 5, shown: 2, not_found: 2, malformed: 1 }),
		"2 statements left out: their quotes were not found in the papers; 1 statement left out: not in the form asked for",
	);
	assert.equal(leftOutLine({ written: 2, shown: 2, not_found: 0, malformed: 0 }), null);
});

test("an answer that does not say, or could not be verified, says so instead of showing statements", () => {
	assert.equal(stateNote(view("answered")), null);
	assert.match(stateNote(view("not_in_papers")) ?? "", /^The papers in scope do not say/);
	assert.match(
		stateNote(view("unverified", { written: 4, shown: 1, not_found: 3, malformed: 0 })) ?? "",
		/only 1 of 4 statements were found/,
	);
	assert.match(stateNote(view("unanswered")) ?? "", /costs again/);
});

test("the passages an answer cites come first and the others are kept apart", () => {
	const { cited, unused } = splitPassages(view("answered").passages);
	assert.deepEqual([cited.map((found) => found.evidence.paper), unused.map((found) => found.evidence.paper)], [["p1"], ["p2"]]);
	assert.equal(excerpt("short"), "short");
	assert.equal(excerpt("one two three four five", 12), "one two…");
});

test("a scope's history is its own questions only", () => {
	const asked = (id: string, community: string | null): AnswerSummary => ({
		id,
		question: id,
		community,
		model: "m",
		asked_at: "",
		answered: true,
	});
	const all = [asked("a", null), asked("b", "c0"), asked("c", "c0.1")];
	assert.deepEqual(
		historyOf(all, null).map((question) => question.id),
		["a"],
	);
	assert.deepEqual(
		historyOf(all, "c0").map((question) => question.id),
		["b"],
	);
});

test("the Ask box sends nothing to the model until it is started, and waits for the answer it started", () => {
	const found: PassagesFound = { state: "ready", query: "q", coverage, passages: [passage("p1", false)] };
	let state: AskState = { kind: "idle" };
	state = reduceAsk(state, { type: "asked", question: "q" });
	assert.equal(state.kind, "searching");
	const indexing = reduceAsk(state, { type: "searched", found: { state: "indexing", done: 3, of: 9 }, estimate });
	assert.deepEqual(indexing, { kind: "indexing", question: "q", done: 3, of: 9 });
	state = reduceAsk(state, { type: "searched", found, estimate });
	assert.equal(state.kind, "found");
	// an answer arriving for a search no longer wanted changes nothing
	assert.equal(reduceAsk(state, { type: "searched", found, estimate }), state);
	assert.equal(reduceAsk({ kind: "idle" }, { type: "started", id: "x", job: 1 }).kind, "idle", "only a found question is started");
	state = reduceAsk(state, { type: "started", id: "a".repeat(64), job: 7 });
	assert.deepEqual(state, { kind: "asking", question: "q", id: "a".repeat(64), job: 7 });
	assert.equal(reduceAsk(state, { type: "viewed", view: view("asking") }), state, "still being asked");
	assert.equal(reduceAsk(state, { type: "viewed", view: { ...view("answered"), id: "other" } }), state, "another question");
	state = reduceAsk(state, { type: "viewed", view: view("answered") });
	assert.equal(state.kind, "answer");
	const failed = reduceAsk(state, { type: "failed", error: "The backend answered 500" });
	assert.deepEqual(failed, { kind: "failed", question: "Does merging count shared directions twice?", error: "The backend answered 500" });
	assert.equal(reduceAsk(failed, { type: "reset" }).kind, "idle");
	const opening = reduceAsk({ kind: "idle" }, { type: "opened", id: "a".repeat(64) });
	assert.equal(reduceAsk(opening, { type: "viewed", view: view("answered") }).kind, "answer");
	// two quick clicks: the answer of the first one, arriving late, does not replace the second
	assert.equal(reduceAsk(opening, { type: "viewed", view: { ...view("answered"), id: "first" } }), opening);
});

test("the Ask box is hidden only when the engine has no such route", () => {
	assert.equal(missingRoute("not found"), true);
	assert.equal(missingRoute("The backend answered 404"), true);
	assert.equal(missingRoute("The backend is not reachable. Start it with: npm run dev"), false);
	// a route that exists and failed: said, not taken for an older backend and hidden
	assert.equal(missingRoute("the run demo was not found"), false);
	assert.equal(missingRoute("the quote was not found in the paper"), false);
});
