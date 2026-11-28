/** What a paper's reviewers have in common is said with what each number counts, a few points at a time. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { commonPoints, commonPointsLine, firstPoints, reviewersSharing, SHOWN_POINTS } from "@/lib/community/common-points";

test("a point says how many of the paper's reviewers make it", () => {
	assert.equal(reviewersSharing({ reviewers: 3, all_reviewers: 4 }), "3 of 4 reviewers");
	assert.equal(reviewersSharing({ reviewers: 2, all_reviewers: 2 }), "2 of 2 reviewers");
});

test("only the first few points are shown until all are asked for", () => {
	const points = ["a", "b", "c", "d", "e", "f", "g"];
	assert.equal(SHOWN_POINTS, 5);
	assert.deepEqual(firstPoints(points, false), { shown: ["a", "b", "c", "d", "e"], hidden: 2 });
	assert.deepEqual(firstPoints(points, true), { shown: points, hidden: 0 });
	assert.deepEqual(firstPoints(["a", "b"], false), { shown: ["a", "b"], hidden: 0 });
	assert.deepEqual(firstPoints([], false), { shown: [], hidden: 0 });
});

test("a paper with nothing in common says so plainly, and one review says there is nothing to compare", () => {
	assert.deepEqual(commonPoints(0, [], []), { kind: "no_reviews" });
	assert.equal(commonPointsLine(commonPoints(0, [], [])), "");
	assert.equal(commonPointsLine(commonPoints(1, [], [])), "Only one review: nothing to compare it with.");
	assert.equal(commonPointsLine(commonPoints(4, [], [])), "The reviewers raise no point in common.");
});

test("the section's toggle says how many points of each tone it holds", () => {
	const points = commonPoints(4, [{}, {}, {}], [{}]);
	assert.deepEqual(points, { kind: "some", praise: 3, criticism: 1 });
	assert.equal(commonPointsLine(points), "Common praise and criticism: 3 points of praise, 1 point of criticism");
});
