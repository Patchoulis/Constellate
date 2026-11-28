/** A rating chip shows stars; its hover says the score on its scale first. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { averageScore, plainChip, ratingTitle, scaleNote } from "@/lib/ratings";

const published = { minimum: 1, maximum: 5, source: "published" as const };
const assumed = { minimum: 1, maximum: 4, source: "assumed" as const };

test("the score is said as mean/maximum Average Score, to one decimal unless whole", () => {
	assert.equal(averageScore(4, published), "4/5 Average Score");
	assert.equal(averageScore(3.75, { minimum: 1, maximum: 4 }), "3.8/4 Average Score");
	assert.equal(averageScore(2.8, assumed), "2.8/4 Average Score");
});

test("a scale that does not start at 1 is named, and a field without a scale says the number alone", () => {
	assert.equal(averageScore(4, { minimum: 0, maximum: 10 }), "4/10 Average Score (0 to 10 scale)");
	assert.equal(averageScore(2.8), "2.8 Average Score");
	assert.equal(plainChip("Novelty", 2.8), "Novelty: 2.8");
	assert.equal(plainChip("Novelty", 3), "Novelty: 3");
});

test("the hover starts with the score, then every reviewer's score, and says when the scale is assumed", () => {
	const told = ratingTitle("Overall Recommendation", { values: [4, 4, 5, 3], mean: 4, meanings: ["accept", "", "", ""], scale: published });
	assert.equal(
		told,
		"4/5 Average Score. Overall Recommendation, as scored by 4 reviewers: 4 (accept), 4, 5, 3. Read from the paper's OpenReview page; the scale is the conference's own.",
	);
	const guessed = ratingTitle("Originality", { values: [3, 3, 2, 3, 3], mean: 2.8, meanings: [], scale: assumed });
	assert.ok(guessed.startsWith("2.8/4 Average Score. Originality, as scored by 5 reviewers: 3, 3, 2, 3, 3."), guessed);
	assert.ok(guessed.endsWith(scaleNote(assumed)), guessed);
	assert.match(scaleNote(assumed), /assumed/);
	const unknown = ratingTitle("Novelty", { values: [2], mean: 2, meanings: [] });
	assert.equal(
		unknown,
		"2 Average Score. Novelty, as scored by 1 reviewer: 2. Read from the paper's OpenReview page; the scale is the conference's own.",
	);
});
