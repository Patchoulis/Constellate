/**
 * What one paper's reviewers have in common (engine analysis/reviews.rs `in_common`), as the strip above the
 * reader's Reviews tab says it: how many reviewers share each point, the first few points of each tone, and a
 * plain line when there is nothing to show.
 */
import { count } from "@/lib/units";

/** Points of each tone shown before "show all". */
export const SHOWN_POINTS = 5;

/** "3 of 4 reviewers": of the paper's reviewers, how many make the point. */
export function reviewersSharing(point: { reviewers: number; all_reviewers: number }): string {
	return `${point.reviewers} of ${count(point.all_reviewers, "reviewer")}`;
}

/** The points shown of one tone: the first SHOWN_POINTS, or all of them when asked, and how many are left out. */
export function firstPoints<Point>(points: Point[], all: boolean, limit = SHOWN_POINTS): { shown: Point[]; hidden: number } {
	const shown = all ? points : points.slice(0, limit);
	return { shown, hidden: points.length - shown.length };
}

/** What there is to say about a paper's reviewers in common. */
export type CommonPoints =
	{ kind: "no_reviews" } | { kind: "one_review" } | { kind: "none_in_common" } | { kind: "some"; praise: number; criticism: number };

/** From the paper's number of reviews and its points of praise and of criticism. */
export function commonPoints(reviews: number, praise: unknown[], criticism: unknown[]): CommonPoints {
	if (reviews === 0) return { kind: "no_reviews" };
	if (reviews === 1 && praise.length === 0 && criticism.length === 0) return { kind: "one_review" };
	if (praise.length === 0 && criticism.length === 0) return { kind: "none_in_common" };
	return { kind: "some", praise: praise.length, criticism: criticism.length };
}

/** The line that stands for the section: the toggle's label when there are points, else what is plain to say. */
export function commonPointsLine(points: CommonPoints): string {
	switch (points.kind) {
		case "no_reviews":
			return "";
		case "one_review":
			return "Only one review: nothing to compare it with.";
		case "none_in_common":
			return "The reviewers raise no point in common.";
		case "some":
			return `Common praise and criticism: ${count(points.praise, "point")} of praise, ${count(points.criticism, "point")} of criticism`;
	}
}
