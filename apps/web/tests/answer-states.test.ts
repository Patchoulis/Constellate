/** The engine's newer answer states, each said where it occurs: a count with its unit and what it means. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtractionRow, StanceFilter } from "@rp/contracts";
import { stateNote } from "@/lib/features/answers/answers";
import { allLeftOutLine, ignoredLine } from "@/lib/features/claim_tree/claim-tree";
import { lookupsStoppedLine } from "@/lib/citations";
import { leftOutLines } from "@/lib/features/critique/critique";
import { abstractOnlyNote, readFromLine } from "@/lib/features/extraction/extraction";
import { notInLibraryLine, quotesNotFoundLine } from "@/lib/features/stances/stances";

test("an answer with no statement at all says there is nothing to show, not that the papers do not say", () => {
	const note = stateNote({ state: "empty" } as Parameters<typeof stateNote>[0]);
	assert.match(note ?? "", /without writing a single statement/);
});

test("a claim tree whose every statement was left out says how many and why", () => {
	const line = allLeftOutLine({ unquoted: 4, orphaned: 2, no_parent: 0, misplaced: 0, over_bounds: 0 });
	assert.match(line, /none of it is shown: 6 statements left out/);
	assert.match(line, /their quotes were not found in the paper \(4\)/);
	assert.match(line, /they stood under a statement that was left out \(2\)/);
	assert.match(allLeftOutLine({ unquoted: 0, orphaned: 0, no_parent: 0, misplaced: 0, over_bounds: 0 }), /none of it is shown\. Ask again/);
});

test("placements of reviewers' points past the most read are counted, with what they are", () => {
	assert.equal(
		ignoredLine({ unknown_points: 0, repeated: 1, past_most: 3 }),
		"Not shown of the model's answer: 1 placement of a point placed again; 3 placements past the most read (two per point).",
	);
	assert.equal(ignoredLine({ unknown_points: 0, repeated: 0 }), null, "an older engine sends no past_most: nothing to say");
});

test("points the model gave no reading for are counted as reviewers' points", () => {
	const lines = leftOutLines({ not_in_paper: 0, malformed: 0, unknown_point: 0, beyond_limit: 0, verdict: 0, not_answered: 1 });
	assert.deepEqual(lines, ["1 reviewer's point the model gave no reading for: it has none beside it above."]);
	assert.deepEqual(leftOutLines({ not_in_paper: 0, malformed: 0, unknown_point: 0, beyond_limit: 0, verdict: 0 }), []);
});

test("a placement says how many of the model's quotes were not found, and one not verified that none was", () => {
	assert.equal(
		quotesNotFoundLine({ group: "supports", evidence: [], quotes_not_found: 2 }),
		"2 quotes the model gave were not found in the paper: left out.",
	);
	assert.equal(quotesNotFoundLine({ group: "contradicts", evidence: [] }), null);
	assert.equal(quotesNotFoundLine({ group: "not_addressed" }), null);
	assert.equal(
		quotesNotFoundLine({ group: "not_verified", quotes_not_found: 3 }),
		"None of the 3 quotes the model gave was found in the paper: what it said is not shown.",
	);
	assert.equal(
		quotesNotFoundLine({ group: "not_verified", quotes_not_found: 1 }),
		"The 1 quote the model gave was not found in the paper: what it said is not shown.",
	);
});

test("papers of the scope the library does not have are said to be neither asked nor skipped", () => {
	const filter: StanceFilter = { papers: 40, not_in_library: 4, related: 30, unrelated: 10, include_unrelated: false, terms: [] };
	assert.equal(notInLibraryLine(filter), "4 papers not in the library: neither asked nor skipped.");
	assert.equal(notInLibraryLine({ ...filter, not_in_library: 0 }), null);
	assert.equal(notInLibraryLine({ ...filter, not_in_library: undefined }), null);
});

test("lookups of cited works that stopped early say why, how many are left, and how to go on", () => {
	assert.equal(
		lookupsStoppedLine({ kind: "at_most", left: 12 }),
		"The lookups of the works this paper cites stopped early: one reading looks up only so many. 12 cited works were not looked up on arXiv yet; reading the references again continues where they stopped.",
	);
	assert.match(
		lookupsStoppedLine({ kind: "failed", why: "arXiv could not be reached", left: 1 }) ?? "",
		/stopped early: arXiv could not be reached\. 1 cited work was not/,
	);
	assert.equal(lookupsStoppedLine(null), null);
	assert.equal(lookupsStoppedLine(undefined), null);
});

test("an extraction table says how many papers are read from their abstract only, telling no PDF from an unreadable one", () => {
	const row = (text: ExtractionRow["text"]) => ({ text }) as ExtractionRow;
	assert.equal(
		readFromLine([row("pdf"), row("no_pdf"), row("no_pdf"), row("unreadable"), row("not_in_library")]),
		"3 papers read from their title and abstract only (2 with no PDF kept, 1 whose PDF could not be read); 1 paper not in the library, not asked.",
	);
	assert.equal(readFromLine([row("pdf")]), null);
	assert.match(abstractOnlyNote("unreadable"), /could not be read/);
	assert.match(abstractOnlyNote("no_pdf"), /No PDF of it is kept/);
});
