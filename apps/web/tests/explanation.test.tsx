/** "Explain this": a selection made a place in the paper, when to ask before spending, and how an explanation is shown. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { Evidence, Explanation, ExplanationEstimate, PaperPassages, PassageAddress, PassageKind } from "@rp/contracts";
import { Explained } from "@/components/passage-explainer";
import { askingCost } from "./costs";
import {
	LONGEST_SELECTION,
	askFirst,
	byPart,
	costLine,
	costNote,
	garbledNote,
	leftOutNote,
	placeKey,
	readAskBelow,
	resolveSelection,
	searchedLine,
	selectionQuote,
	symbolSource,
} from "@/lib/explanation";

const address = (page: number, start: number, end: number, section = "2. Method"): PassageAddress => ({
	page,
	section,
	paragraph: 1,
	start,
	end_page: page,
	end,
});

const passage = (kind: PassageKind, page: number, start: number, text: string) => ({
	kind,
	part: "body" as const,
	flow: "starts" as const,
	address: address(page, start, start + Array.from(text).length),
	text,
	sentences: [],
});

const paper: PaperPassages = {
	reader: 1,
	pages: [],
	passages: [
		passage("heading", 2, 0, "2. Method"),
		passage("text", 2, 11, "The merged update is the sum of the task updates, scaled by α."),
		passage("equation", 2, 75, "∆W = α ∑ i ∆Wi (1)"),
		passage("text", 2, 95, "where ∆Wi is the update of task i and α the coefficient."),
	],
};

test("a selection is explained as the passage it is in, and words not in the paper are refused", () => {
	const inEquation = resolveSelection(paper, { found: true, address: address(2, 80, 85), wording: "α ∑ i", matched: { kind: "exact" } }, 2);
	assert.deepEqual(inEquation, { kind: "passage", index: 2, note: null });
	// "α" alone is found at its first place in the paper; the passage it was selected in wins
	const short = resolveSelection(paper, { found: true, address: address(2, 72, 73), wording: "α", matched: { kind: "exact" } }, 3);
	assert.deepEqual(short, { kind: "passage", index: 3, note: null });
	const refused = resolveSelection(paper, { found: false }, 1);
	assert.equal(refused.kind, "refused");
	assert.match(refused.kind === "refused" ? refused.why : "", /not found in the paper/);
	const heading = resolveSelection(paper, { found: true, address: address(2, 0, 9), wording: "2. Method", matched: { kind: "exact" } }, 0);
	assert.equal(heading.kind, "refused");
	// a selection that runs on past its paragraph explains the paragraph it starts in, and says so
	const long = resolveSelection(paper, { found: true, address: address(2, 20, 90), wording: "x", matched: { kind: "exact" } }, null);
	assert.deepEqual(long, {
		kind: "passage",
		index: 1,
		note: "The selection runs past this paragraph: the paragraph it starts in is explained.",
	});
});

test("the selected words are tidied before they are located, and a selection too long is refused", () => {
	assert.deepEqual(selectionQuote("  the merged\n update  "), { quote: "the merged update" });
	assert.equal(selectionQuote("   "), null);
	assert.ok("refused" in (selectionQuote("x".repeat(LONGEST_SELECTION + 1)) ?? {}));
	assert.equal(placeKey(address(3, 120, 200)), "3:120");
});

const estimate: ExplanationEstimate = askingCost(0.021, 0.087, {
	tokens: { input: 200, output: 1_500, cache_read: 0, cache_write_5m: 5_000, cache_write_1h: 0 },
});

test("the cost is always asked about, unless the reader chose an amount the worst case stays under", () => {
	assert.equal(readAskBelow(null), null, "the default: always ask");
	assert.equal(readAskBelow("abc"), null);
	assert.equal(readAskBelow("-1"), null);
	assert.equal(readAskBelow("0.1"), 0.1);
	assert.equal(askFirst(estimate, null), true);
	assert.equal(askFirst(estimate, 0.1), false);
	assert.equal(askFirst(estimate, 0.05), true, "the most it could cost is compared, not the likely cost");
	assert.equal(askFirst({ ...estimate, most_usd: null, usd: null }, 100), true, "a model with no known price always asks");
});

test("the estimate is said with its currency and units and as an estimate", () => {
	const line = costLine(estimate, "en-US");
	assert.match(line, /^Estimate: about USD 0\.0210, at most USD 0\.0870 \(5\.2k tokens sent, about 1\.5k tokens back\)\.$/);
	assert.equal(
		costLine({ ...estimate, usd: null, most_usd: null }),
		"Estimate: at most 28k tokens (5.2k tokens sent, about 1.5k tokens back).",
	);
	assert.match(costNote(estimate), /assumed/);
});

test("what is sent of an equation or a table, and what was left out, are said", () => {
	assert.match(garbledNote("equation") ?? "", /not as an image/);
	assert.match(garbledNote("table_caption") ?? "", /layout/);
	assert.equal(garbledNote("text"), null);
	assert.equal(leftOutNote({ not_found: 0, malformed: 0, past_bound: 0 }), null);
	assert.equal(
		leftOutNote({ not_found: 3, malformed: 1, past_bound: 2 }),
		"3 statements left out: their quotes were not found in the paper; 2 statements left out: past the most read from one answer; 1 statement left out: not of the form asked for.",
	);
	assert.equal(symbolSource("\\Delta W_i"), "$\\Delta W_i$");
	assert.equal(symbolSource("$\\alpha$"), "$\\alpha$");
	assert.match(
		searchedLine({ nearby: 12, bound: 30, by_text: 9, by_abstract: 3, not_in_library: 0 }),
		/^Searched the 12 nearest papers of this run \(at most 30 papers: .*\): 9 papers by their text, 3 papers by their abstract only \(no PDF read\)\.$/,
	);
	assert.match(searchedLine({ nearby: 0, bound: 30, by_text: 0, by_abstract: 0, not_in_library: 0 }), /no neighbours/);
});

const quoted: Evidence = {
	paper: "2601.00001",
	quote: "where ∆Wi is the update of task i",
	established: { kind: "verified", address: address(2, 95, 128), matched: { kind: "exact" } },
};

const explanation: Explanation = {
	model: "claude-opus-5-5",
	kept_at: "2026-11-27T10:00:00Z",
	statements: [
		{ source: "background", part: "prerequisite", symbol: null, text: "A sum adds up terms." },
		{ source: "paper", part: "symbol", symbol: "\\Delta W_i", text: "The update of task $i$.", evidence: [quoted] },
		{ source: "paper", part: "explanation", symbol: null, text: "The updates are added.", evidence: [quoted] },
	],
	left_out: { not_found: 2, malformed: 0, past_bound: 0 },
};

test("statements are grouped by part, in a fixed order, each in the model's order", () => {
	assert.deepEqual(
		byPart(explanation.statements).map((group) => group.part),
		["explanation", "symbol", "prerequisite"],
	);
});

test("an explanation shows the paper's statements with their places and background apart, with no link", () => {
	const html = renderToStaticMarkup(<Explained explained={explanation} onRedo={() => {}} />);
	assert.match(html, /Background, not from this paper/);
	assert.match(html, /p\. 2/, "a paper's statement links to its place, by page");
	assert.match(html, /katex/, "symbols and math typeset");
	assert.match(html, /2 statements left out: their quotes were not found in the paper/);
	assert.match(html, /costs nothing/);
	// the background statement carries no citation button
	const background = html.slice(html.indexOf("Background, not from this paper"), html.indexOf("A sum adds up terms.") + 30);
	assert.doesNotMatch(background, /<button/);
	// the paper's words are what a citation shows on hover
	assert.match(html, /where ∆Wi is the update of task i/);
});
