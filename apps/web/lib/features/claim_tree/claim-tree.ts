/**
 * The Claims view's logic (components/features/claim_tree/claims-view.tsx): the tree as an outline of rows, the "only contested
 * claims" filter, moving through it with the keyboard, and the words every figure is shown with. Plain functions,
 * so that they are tested (tests/claim-tree.test.ts).
 */
import type {
	BudgetOf,
	ClaimNode,
	ClaimNodeReviews,
	ClaimsEstimate,
	ClaimsLeftOut,
	PointMapping,
	PointPlacement,
	ReviewerRating,
	ReviewPoint,
	ReviewStance,
	UnplacedPoint,
} from "@rp/contracts";
import { codePoints } from "@/lib/evidence";
import { centBudget, costWords } from "@/lib/cost";
import { count, onScale, scaleName } from "@/lib/units";

/** Said wherever the model's reading is shown. */
export const MODEL_NOTE = "The model's reading of the paper, in its words; the quotes beside it are the paper's own, found in it.";
export const MAPPING_NOTE =
	"Placed on this claim by the model, which also gave the reviewer's words that justify it (marked). In eLife's pilot of reviews mapped onto claims about one placement in five was judged wrong: read both texts before relying on it.";

/** One row of the outline: a node, how deep it is, and whether it can be opened. */
export interface Row {
	node: ClaimNode;
	/** 1 for a central claim */
	depth: number;
	hasChildren: boolean;
	open: boolean;
	contested: boolean;
}

export function childrenOf(nodes: ClaimNode[], parent: string | null): ClaimNode[] {
	return nodes.filter((node) => node.parent === parent);
}

/** The reviews at one node (null: the root), if any were placed there. */
export function reviewsAt(reviews: ClaimNodeReviews[], node: string | null): ClaimNodeReviews | undefined {
	return reviews.find((here) => here.node === node);
}

/** Is `id`, or anything under it, contested? */
function contestedUnder(nodes: ClaimNode[], reviews: ClaimNodeReviews[], id: string): boolean {
	return Boolean(reviewsAt(reviews, id)?.contested) || childrenOf(nodes, id).some((child) => contestedUnder(nodes, reviews, child.id));
}

/**
 * The rows shown: every node whose parents are all open, in the tree's order. With `onlyContested`, only the
 * nodes reviewers contested and those above them (for context).
 */
export function outline(nodes: ClaimNode[], reviews: ClaimNodeReviews[], open: ReadonlySet<string>, onlyContested = false): Row[] {
	const rows: Row[] = [];
	const visit = (parent: string | null, depth: number) => {
		for (const node of childrenOf(nodes, parent)) {
			if (onlyContested && !contestedUnder(nodes, reviews, node.id)) continue;
			const children = childrenOf(nodes, node.id).filter((child) => !onlyContested || contestedUnder(nodes, reviews, child.id));
			const isOpen = open.has(node.id);
			rows.push({
				node,
				depth,
				hasChildren: children.length > 0,
				open: isOpen,
				contested: Boolean(reviewsAt(reviews, node.id)?.contested),
			});
			if (isOpen) visit(node.id, depth + 1);
		}
	};
	visit(null, 1);
	return rows;
}

/** What a key does in the outline (the WAI-ARIA tree pattern): which row to focus, and which to open or close. */
export type Move = { focus: string } | { toggle: string } | null;

export function move(rows: Row[], focused: string | null, key: string): Move {
	if (rows.length === 0) return null;
	const at = Math.max(
		0,
		rows.findIndex((row) => row.node.id === focused),
	);
	const row = rows[at];
	switch (key) {
		case "ArrowDown":
			return { focus: rows[Math.min(rows.length - 1, at + 1)].node.id };
		case "ArrowUp":
			return { focus: rows[Math.max(0, at - 1)].node.id };
		case "Home":
			return { focus: rows[0].node.id };
		case "End":
			return { focus: rows[rows.length - 1].node.id };
		case "ArrowRight":
			if (row.hasChildren && !row.open) return { toggle: row.node.id };
			if (row.open) return { focus: rows[Math.min(rows.length - 1, at + 1)].node.id };
			return null;
		case "ArrowLeft":
			if (row.open) return { toggle: row.node.id };
			return row.node.parent ? { focus: row.node.parent } : null;
		case "Enter":
		case " ":
			return row.hasChildren ? { toggle: row.node.id } : null;
		default:
			return null;
	}
}

/** Every node that has children: the outline opened all the way. */
export function allOpen(nodes: ClaimNode[]): Set<string> {
	return new Set(nodes.filter((node) => nodes.some((other) => other.parent === node.id)).map((node) => node.id));
}

/** What a node is, in words: "Claim", "Finding", "Evidence: table". */
export function kindLabel(node: ClaimNode): string {
	if (node.kind === "claim") return "Claim";
	if (node.kind === "finding") return "Finding";
	return `Evidence: ${node.evidence_kind}`;
}

/** A stance in words (it is also shown in colour, never in colour alone). */
export function stanceWords(stance: ReviewStance): string {
	return {
		supports: "supports",
		challenges: "challenges",
		asks: "asks about",
		presentation: "about presentation only",
		whole_paper: "about the paper as a whole",
	}[stance];
}

/** The colour that goes with a stance's words. */
export function stanceTone(stance: ReviewStance): string {
	return {
		supports: "text-success",
		challenges: "text-warning",
		asks: "text-primary",
		presentation: "text-muted-foreground",
		whole_paper: "text-muted-foreground",
	}[stance];
}

/** "challenged by 2 of 4 reviewers", "supported by 1 of 3 reviewers". */
export function byReviewers(verb: string, names: string[], reviewers: number): string {
	return `${verb} by ${names.length} of ${count(reviewers, "reviewer")}`;
}

/** A reviewer's score with its scale and field: "6 of 10 rating" ("4 of 0 to 10 rating" on a scale from 0). */
export function ratingLabel(rating: ReviewerRating): string {
	return onScale(rating.value, rating.scale, rating.field.toLowerCase());
}

/** What hovering a score says: the scale, and whether it is the venue's own or assumed. */
export function ratingNote(rating: ReviewerRating): string {
	return rating.scale.source === "assumed"
		? `On ${scaleName(rating.scale)}, assumed: the venue's form for that year is not known, so the scale is an estimate.`
		: `On ${scaleName(rating.scale)}, as the venue published it for that year.`;
}

/** "3 statements left out: their quotes were not found in the paper (2); they stood under one that was (1)". */
export function leftOutLine(left: ClaimsLeftOut): string | null {
	const reasons: [number, string][] = [
		[left.unquoted, "their quotes were not found in the paper"],
		[left.orphaned, "they stood under a statement that was left out"],
		[left.no_parent + left.misplaced, "they did not fit in the tree"],
		[left.over_bounds, "past the size the tree is kept to"],
	];
	const shown = reasons.filter(([amount]) => amount > 0);
	const total = shown.reduce((sum, [amount]) => sum + amount, 0);
	if (total === 0) return null;
	return `${count(total, "statement")} left out: ${shown.map(([amount, why]) => (shown.length > 1 ? `${why} (${amount})` : why)).join("; ")}`;
}

/** A tree was written but every statement of it was left out: how many, and why, so that "nothing" is explained. */
export function allLeftOutLine(left: ClaimsLeftOut): string {
	const why = leftOutLine(left);
	return why
		? `The model wrote a claim tree, but none of it is shown: ${why}. Ask again with the button below.`
		: "The model wrote a claim tree, but none of it is shown. Ask again with the button below.";
}

/** What the model answered about reviewers' points that is not shown, counted: "2 placements past the most read". */
export function ignoredLine(ignored: PointMapping["ignored"]): string | null {
	const parts = [
		ignored.unknown_points > 0 ? `${count(ignored.unknown_points, "placement")} of points that were not sent` : null,
		ignored.repeated > 0 ? `${count(ignored.repeated, "placement")} of a point placed again` : null,
		(ignored.past_most ?? 0) > 0 ? `${count(ignored.past_most ?? 0, "placement")} past the most read (two per point)` : null,
	].filter((part): part is string => part !== null);
	return parts.length > 0 ? `Not shown of the model's answer: ${parts.join("; ")}.` : null;
}

/** "about USD 0.0412, at most USD 0.3120, for 2 calls": an estimate, and it says so where it is shown. For a model
 *  with no price, only the tokens it can take: no dollar figure at all. */
export function estimateLine(estimate: ClaimsEstimate): string {
	if (estimate.to_ask === 0) return "Nothing to ask: every answer is kept.";
	const words = costWords(estimate);
	return `${words.charAt(0).toUpperCase()}${words.slice(1)}, for ${count(estimate.to_ask, "call")} (an estimate)`;
}

export const ESTIMATE_NOTE =
	"Estimated from the paper's length, not counted: tokens are reckoned at three characters each, which errs high. The budget sent with the request is the estimate's most, rounded up to the cent: nothing past it is spent.";

/** The most the calls may spend: the estimate's worst case, rounded up to the cent (never past the engine's USD 10);
 *  for a model with no price, its worst case in tokens. */
export function budgetFor(estimate: ClaimsEstimate): BudgetOf {
	return centBudget(estimate, 10);
}

/** The points placed on `node` (null: the root), in the page's order, with their placements. */
export function pointsAt(
	node: string | null,
	mapping: PointMapping,
	points: ReviewPoint[],
): { point: ReviewPoint; placement: PointPlacement }[] {
	return points.flatMap((point) => {
		const placement = mapping.placed.find((placed) => placed.point === point.id && placed.node === node);
		return placement ? [{ point, placement }] : [];
	});
}

/** A point's text cut around the words that justify its placement: [before, the words, after]. Spans are code
 *  points of the page; the point starts at its own span. */
export function marked(point: ReviewPoint, placement: PointPlacement): [string, string, string] {
	const from = Math.max(0, placement.because_span.start - point.span.start);
	const to = Math.max(from, placement.because_span.end - point.span.start);
	return [codePoints(point.text, 0, from), codePoints(point.text, from, to), codePoints(point.text, to)];
}

/** Why a point was not placed, in words. */
export function unplacedWhy(why: UnplacedPoint["why"]): string {
	return {
		not_answered: "the model did not place it",
		unknown_node: "the model placed it on a claim the tree does not have",
		not_in_point: "the words the model gave for it are not in it",
		too_short: "the model gave too few of its words to justify a placement",
	}[why];
}
