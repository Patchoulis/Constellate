/** The Claims view's logic: the outline, its filter and keys, and the words its figures are shown with. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ClaimNode, ClaimNodeReviews, ClaimsEstimate, Evidence, PointMapping, ReviewPoint } from "@rp/contracts";
import { batchCost } from "./costs";
import {
	allOpen,
	budgetFor,
	byReviewers,
	estimateLine,
	kindLabel,
	leftOutLine,
	marked,
	move,
	outline,
	pointsAt,
	ratingLabel,
	stanceWords,
	unplacedWhy,
} from "@/lib/features/claim_tree/claim-tree";

const quote: Evidence = {
	paper: "iclr-2024-7",
	quote: "matches dense experts at a fraction of the compute",
	established: {
		kind: "verified",
		address: { page: 1, section: "1. Introduction", paragraph: 1, start: 30, end_page: 1, end: 80 },
		matched: { kind: "exact" },
	},
};

const node = (id: string, parent: string | null, kind: "claim" | "finding" | "table"): ClaimNode =>
	kind === "table"
		? { id, parent, kind: "evidence", evidence_kind: "table", statement: `statement ${id}`, evidence: [quote], points_at: null }
		: { id, parent, kind, statement: `statement ${id}`, evidence: [quote], points_at: null };

// n1 > n2 > n3, n1 > n4; n5 (another claim) > n6
const nodes = [
	node("n1", null, "claim"),
	node("n2", "n1", "finding"),
	node("n3", "n2", "table"),
	node("n4", "n1", "table"),
	node("n5", null, "claim"),
	node("n6", "n5", "finding"),
];

const at = (id: string | null, contested: boolean): ClaimNodeReviews => ({
	node: id,
	points: ["p1"],
	reviewers: { supports: [], challenges: contested ? ["Reviewer Ab12"] : [], asks: [], presentation: [], whole_paper: [] },
	contested,
});

const ids = (rows: ReturnType<typeof outline>) => rows.map((row) => row.node.id);

test("the outline shows every node whose parents are open, in the tree's order, with its depth", () => {
	const rows = outline(nodes, [], allOpen(nodes));
	assert.deepEqual(ids(rows), ["n1", "n2", "n3", "n4", "n5", "n6"]);
	assert.deepEqual(
		rows.map((row) => row.depth),
		[1, 2, 3, 2, 1, 2],
	);
	assert.deepEqual(ids(outline(nodes, [], new Set(["n1"]))), ["n1", "n2", "n4", "n5"]);
	assert.equal(
		outline(nodes, [], new Set()).every((row) => !row.open),
		true,
	);
});

test("only contested claims keeps the contested nodes and what they stand under", () => {
	const reviews = [at(null, true), at("n3", true), at("n6", false)];
	const rows = outline(nodes, reviews, allOpen(nodes), true);
	assert.deepEqual(ids(rows), ["n1", "n2", "n3"]);
	assert.deepEqual(
		rows.map((row) => row.contested),
		[false, false, true],
	);
	assert.deepEqual(ids(outline(nodes, [], allOpen(nodes), true)), [], "nothing contested: nothing shown");
});

test("the arrow keys move through the outline as a tree does", () => {
	const open = allOpen(nodes);
	const rows = outline(nodes, [], open);
	assert.deepEqual(move(rows, "n1", "ArrowDown"), { focus: "n2" });
	assert.deepEqual(move(rows, "n1", "ArrowUp"), { focus: "n1" });
	assert.deepEqual(move(rows, "n3", "ArrowLeft"), { focus: "n2" }, "a leaf moves to its parent");
	assert.deepEqual(move(rows, "n2", "ArrowLeft"), { toggle: "n2" }, "an open node closes");
	assert.deepEqual(move(rows, "n1", "End"), { focus: "n6" });
	assert.deepEqual(move(rows, "n6", "Home"), { focus: "n1" });
	const closed = outline(nodes, [], new Set());
	assert.deepEqual(move(closed, "n1", "ArrowRight"), { toggle: "n1" }, "a closed node opens");
	assert.deepEqual(move(rows, "n1", "ArrowRight"), { focus: "n2" }, "an open node moves into its first child");
	assert.deepEqual(move(rows, "n3", "Enter"), null, "a leaf has nothing to open");
	assert.equal(move([], null, "ArrowDown"), null);
});

test("kinds and stances are said in words", () => {
	assert.equal(kindLabel(nodes[0]), "Claim");
	assert.equal(kindLabel(nodes[2]), "Evidence: table");
	assert.equal(stanceWords("challenges"), "challenges");
	assert.equal(stanceWords("whole_paper"), "about the paper as a whole");
	assert.equal(unplacedWhy("not_in_point"), "the words the model gave for it are not in it");
});

test("counts and ratings carry their units and the venue's scale", () => {
	assert.equal(byReviewers("challenged", ["Reviewer Ab12", "Reviewer Cd34"], 4), "challenged by 2 of 4 reviewers");
	assert.equal(byReviewers("supported", ["Reviewer Ab12"], 1), "supported by 1 of 1 reviewer");
	assert.equal(ratingLabel({ field: "Rating", value: 6, scale: { minimum: 1, maximum: 10, source: "published" } }), "6 of 10 rating");
	assert.equal(ratingLabel({ field: "Rating", value: 4, scale: { minimum: 0, maximum: 10, source: "published" } }), "4 of 0 to 10 rating");
});

test("what was left out is counted with why, and nothing is said when nothing was", () => {
	const none = { unquoted: 0, orphaned: 0, no_parent: 0, misplaced: 0, over_bounds: 0 };
	assert.equal(leftOutLine(none), null);
	assert.equal(leftOutLine({ ...none, unquoted: 3 }), "3 statements left out: their quotes were not found in the paper");
	assert.equal(
		leftOutLine({ ...none, unquoted: 2, orphaned: 1 }),
		"3 statements left out: their quotes were not found in the paper (2); they stood under a statement that was left out (1)",
	);
});

const estimate = (usd: number | null, most: number | null, toAsk = 2): ClaimsEstimate =>
	batchCost(usd, most, toAsk, { items: 2, kept: 2 - toAsk });

test("the estimate says it is one, in its currency, and the budget is its most rounded up to the cent", () => {
	assert.equal(estimateLine(estimate(0.0412, 0.3101)), "About USD 0.0412, at most USD 0.3101, for 2 calls (an estimate)");
	assert.equal(estimateLine(estimate(0, 0, 0)), "Nothing to ask: every answer is kept.");
	assert.equal(estimateLine(estimate(null, null)), "At most 28k tokens, for 2 calls (an estimate)", "no price: no dollar figure");
	assert.deepEqual(budgetFor(estimate(0.0412, 0.3101)), { budget_usd: 0.32 });
	assert.deepEqual(budgetFor(estimate(0.0001, 0.0002)), { budget_usd: 0.01 });
	assert.deepEqual(budgetFor(estimate(null, null)), { budget_tokens: 28_000 }, "no price: held to its tokens");
	assert.deepEqual(budgetFor(estimate(30, 40)), { budget_usd: 10 }, "never past what the engine accepts");
});

test("a point's justifying words are marked in the reviewer's own text, counted in code points", () => {
	const point: ReviewPoint = {
		id: "p1",
		review: 2,
		reviewer: "Reviewer Ab12",
		section: "Weaknesses",
		kind: "weakness",
		text: "𝐖 is unclear, and the compute comparison is unfair.",
		span: { start: 100, end: 151 },
	};
	const mapping: PointMapping = {
		placed: [
			{
				point: "p1",
				node: "n1",
				stance: "challenges",
				because: "the compute comparison is unfair",
				because_span: { start: 100 + 18, end: 100 + 50 },
				matched: { kind: "exact" },
			},
		],
		unplaced: [],
		ignored: { unknown_points: 0, repeated: 0 },
	};
	assert.deepEqual(marked(point, mapping.placed[0]), ["𝐖 is unclear, and ", "the compute comparison is unfair", "."]);
	assert.equal(pointsAt("n1", mapping, [point]).length, 1);
	assert.equal(pointsAt(null, mapping, [point]).length, 0);
	assert.equal(pointsAt("n2", mapping, [point]).length, 0);
});
