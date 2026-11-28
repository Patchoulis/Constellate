/**
 * A paper's reviewers' scores as its rating chips say them: stars out of the scale's points in the chip, the
 * score itself on hover ("3.8/4 Average Score"). Scales differ between conferences and years, so each score is
 * said on its own scale and a field without a known scale is said as a plain number.
 */
import type { FieldRatings, Scale } from "@rp/contracts";
import { scaleName } from "@/lib/units";

const score = (value: number) => (Number.isInteger(value) ? String(value) : value.toFixed(1));

/** Where a scale comes from, said: an assumed one is an estimate. */
export function scaleNote(scale: Scale): string {
	return scale.source === "assumed"
		? `On ${scaleName(scale)}, assumed: Constellate does not know this venue's form for that year, so the scale is an estimate.`
		: `On ${scaleName(scale)}, as the venue published it for that year.`;
}

/**
 * The mean on its scale: "3.8/4 Average Score", "4/5 Average Score"; a scale that does not start at 1 is named
 * ("4/10 Average Score (0 to 10 scale)"); without a scale, "2.8 Average Score".
 */
export function averageScore(mean: number, scale?: Pick<Scale, "minimum" | "maximum">): string {
	if (!scale) return `${score(mean)} Average Score`;
	const named = scale.minimum === 1 ? "" : ` (${scale.minimum} to ${scale.maximum} scale)`;
	return `${score(mean)}/${scale.maximum} Average Score${named}`;
}

/** A chip without stars (its field has no known scale): "Novelty: 2.8". */
export function plainChip(field: string, mean: number): string {
	return `${field}: ${score(mean)}`;
}

/**
 * A rating chip's hover: the score first, then the field with every reviewer's score and what it means, where it
 * was read, and, when the scale is assumed rather than published, that it is an estimate.
 */
export function ratingTitle(field: string, ratings: FieldRatings): string {
	const { values, meanings, mean, scale } = ratings;
	const each = values.map((value, index) => (meanings[index] ? `${value} (${meanings[index]})` : String(value))).join(", ");
	return [
		`${averageScore(mean, scale)}.`,
		`${field}, as scored by ${values.length} reviewer${values.length === 1 ? "" : "s"}: ${each}.`,
		"Read from the paper's OpenReview page; the scale is the conference's own.",
		...(scale?.source === "assumed" ? [scaleNote(scale)] : []),
	].join(" ");
}
