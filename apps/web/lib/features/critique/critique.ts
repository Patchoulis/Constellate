/**
 * What the Weaknesses section says, in words (components/features/critique/critique.tsx draws it): who a reviewer is and what they
 * scored, how a reply is attached, where a point is in its section, what the model's reading left out, what asking
 * would cost. Plain functions, tested in tests/features/critique/critique.test.tsx.
 */
import type {
	Absence,
	CritiqueEstimate,
	CritiqueLeftOut,
	CritiquePoint,
	CritiqueReading,
	CritiqueReviewer,
	CritiqueScore,
	CritiqueSection,
	PointReading,
	ReplyAttachment,
	ReviewConcern,
	WeaknessKind,
} from "@rp/contracts";
import { codePoints } from "@/lib/reader/evidence";
import { costTitle, costWords } from "@/lib/cost";
import { count, onScale } from "@/lib/units";

/** Said wherever the model's part is shown: what it is, and what it is not. */
export const MODEL_NOTE =
	"Written by the model from the paper's text; each quote beside it was found in the paper. It is a reading to check, not a review: no score, no verdict.";

/** Said wherever the reviewers' points are shown. */
export const REVIEWERS_NOTE =
	"The reviewers' own words, from the OpenReview page you opened in the reader, cut into points by a program: check the page before relying on it.";

/** The bills are in US dollars: estimates are shown in them, with the code. */
const DOLLARS = { code: "USD", rate: 1, date: null };

export function reviewerName(reviewer: Pick<CritiqueReviewer, "reviewer">): string {
	return reviewer.reviewer ? `Reviewer ${reviewer.reviewer}` : "A reviewer";
}

/** "6 of 10 rating", "4 of 5 confidence", "3 of 5 overall recommendation". */
export function scoreLine(score: CritiqueScore): string {
	return onScale(score.value, score.scale, score.field.toLowerCase());
}

/** What hovering a score says: what it means as the review puts it, and where its scale comes from. */
export function scoreTitle(score: CritiqueScore): string {
	const meaning = score.meaning ? ` (“${score.meaning}”)` : "";
	const scale =
		score.scale.source === "published" ? "on the venue's own scale for that year" : "on a scale assumed for this venue: an estimate";
	return `${score.field} ${score.value}${meaning}, ${scale}. Read by a program from the reviews page.`;
}

export const CONCERN_NAME: Record<ReviewConcern, string> = { weakness: "Weakness", question: "Question", limitation: "Limitation" };

export const KIND_NAME: Record<WeaknessKind, string> = {
	missing_baseline: "Missing baseline or comparison",
	few_datasets_or_seeds: "Few datasets or seeds",
	no_significance_or_variance: "No significance or variance reported",
	claim_broader_than_evidence: "Claim broader than the evidence",
	missing_ablation: "Missing ablation",
	reproducibility_detail_missing: "Reproducibility detail missing",
	limitation_not_discussed: "Limitation not discussed",
	other: "Other",
};

/** How an authors' reply is attached to a review, in words: the page's own naming, or a guess and its ground. */
export function attachmentNote(attachment: ReplyAttachment): string {
	if (attachment.kind === "named") return "attached by its title, which names this reviewer";
	return {
		same_reviewer: "attached by a guess: it is by this review's reviewer",
		mentions_reviewer: "attached by a guess: its first lines address this reviewer",
		follows_review: "attached by a guess: it follows this review on the page",
	}[attachment.basis];
}

/** A point's section in three parts: before the point, the point, after it (characters are code points). */
export function pointInSection(section: CritiqueSection, point: CritiquePoint): [string, string, string] {
	const from = point.span.start - section.span.start;
	const to = point.span.end - section.span.start;
	return [codePoints(section.text, 0, from), codePoints(section.text, from, to), codePoints(section.text, to)];
}

/** The model's reading of each point, by the point's id. */
export function readingsByPoint(reading: CritiqueReading | null | undefined): Map<string, PointReading> {
	return new Map((reading?.points ?? []).map((point) => [point.point, point]));
}

/** What of the model's answer is not shown, one line each: never what it said. */
export function leftOutLines(leftOut: CritiqueLeftOut): string[] {
	const lines: string[] = [];
	if (leftOut.not_in_paper > 0)
		lines.push(`${count(leftOut.not_in_paper, "statement")} left out: their quotes were not found in the paper.`);
	if (leftOut.malformed > 0)
		lines.push(`${count(leftOut.malformed, "entry", "entries")} of the answer left out: not in the form asked for.`);
	if (leftOut.unknown_point > 0)
		lines.push(`${count(leftOut.unknown_point, "entry", "entries")} left out: about no reviewer's point that was asked about.`);
	if (leftOut.beyond_limit > 0) lines.push(`${count(leftOut.beyond_limit, "weakness", "weaknesses")} past the first eight left out.`);
	if (leftOut.verdict > 0)
		lines.push(`${count(leftOut.verdict, "statement")} left out: worded as a judgement of the paper, which this section does not give.`);
	if ((leftOut.not_answered ?? 0) > 0)
		lines.push(
			`${count(leftOut.not_answered ?? 0, "reviewer's point", "reviewers' points")} the model gave no reading for: ${leftOut.not_answered === 1 ? "it has" : "they have"} none beside ${leftOut.not_answered === 1 ? "it" : "them"} above.`,
		);
	return lines;
}

/** What hovering a weakness that says something is absent says, and the line shown under it. */
export function absenceLine(absence: Absence): string | null {
	const words = (searched: string[]) => searched.map((term) => `“${term}”`).join(", ");
	switch (absence.kind) {
		case "not_claimed":
			return null;
		case "not_searched":
			return "Absence: check yourself. A quote cannot show that something is missing, and the model named no words to search for.";
		case "none_found":
			return `Absence: check yourself. Constellate searched the paper's text for ${words(absence.searched)} and found none of them.`;
		case "but_see":
			return `Absence: check yourself. But the paper's text has ${words(absence.searched)}; see:`;
	}
}

/** "about USD 0.17, at most USD 0.35": an estimate; for a model with no price, only the tokens it can take
 *  ("at most 38k tokens"), never a dollar figure that is not known. */
export function estimateLine(estimate: CritiqueEstimate, locale?: string): string {
	return costWords(estimate, DOLLARS, locale);
}

/** What hovering the estimate says: how it was reckoned. */
export function estimateTitle(estimate: CritiqueEstimate): string {
	return `${costTitle(estimate)} The call is capped at the worst case: nothing is sent until you click.`;
}
