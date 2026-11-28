/** A reviewer's words as shown: a formula's pieces go back on the line they belong to. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { mendFormulaBreaks } from "@/lib/reader/review-text";

test("a formula the page put on lines of its own is read as one sentence again", () => {
	const kept =
		"Hyperparameter sensitivity: The performance appears to depend on key hyperparameters\n(\nλ\n,\nα\n)\n, but guidance on how to choose them across domains is limited or unclear.";
	assert.equal(
		mendFormulaBreaks(kept),
		"Hyperparameter sensitivity: The performance appears to depend on key hyperparameters (λ, α), but guidance on how to choose them across domains is limited or unclear.",
	);
	assert.equal(mendFormulaBreaks("The bound is\nO(n)\nin the worst case."), "The bound is O(n) in the worst case.");
	assert.equal(mendFormulaBreaks("with\nk=5\nneighbours"), "with k=5 neighbours");
});

test("the reviewer's own line breaks, paragraphs and lists are kept", () => {
	const paragraphs = "The method is sound and clearly presented.\n\nThe experiments are limited to two datasets.";
	assert.equal(mendFormulaBreaks(paragraphs), paragraphs);
	const lines = "The method is sound.\nThe experiments are limited.";
	assert.equal(mendFormulaBreaks(lines), lines, "two whole lines of words stay two lines");
	const list = "Weaknesses:\n1.\nThe baselines are weak.\n-\nNo ablation is given.\n- The writing is dense.";
	assert.equal(mendFormulaBreaks(list), list, "a list's marks are not pieces of a formula");
	const short = "Typos\nSeveral in section 3.";
	assert.equal(mendFormulaBreaks(short), short, "a short line of words is a line, not a formula");
});

test("text without such breaks comes back as it was", () => {
	for (const text of ["", "One line only.", "A (short) aside, and λ inline."]) assert.equal(mendFormulaBreaks(text), text);
});
