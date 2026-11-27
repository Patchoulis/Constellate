import type { Acceptance, AiVerdict, Ratings } from "@rp/contracts";
import { cn } from "@/lib/utils";

/**
 * The app's mark: bodies on a spiral orbit. The paths are the "universe" icon of Tabler Icons
 * (https://tabler.io/icons, MIT licence, (c) Pawel Kuna); see THIRD_PARTY_NOTICES.md.
 */
export function Logo({ className }: { className?: string }) {
	return (
		<svg
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth="1.75"
			strokeLinecap="round"
			strokeLinejoin="round"
			className={cn("size-4", className)}
			aria-hidden
		>
			<path d="M7.027 11.477a5 5 0 1 0 5.496 -4.45a4.951 4.951 0 0 0 -3.088 .681" />
			<path d="M5.636 5.636a9 9 0 1 0 3.555 -2.188" />
			<path d="M17 5a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
			<path d="M11 12a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
			<path d="M8 16a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
		</svg>
	);
}

const TAG = "inline-flex max-w-full items-center rounded-sm border px-1.5 py-px font-ui text-xs";

/** How a conference paper was accepted. An ordinary acceptance is the norm, so it gets no tag. */
export function AcceptanceTag({ state, small }: { state: Acceptance | null; small?: boolean }) {
	if (state !== "oral" && state !== "spotlight") return null;
	return (
		<span
			title={
				state === "oral"
					? "Accepted as an oral: the conference's committee picked it for a talk, which only the top few percent of accepted papers get."
					: "Accepted as a spotlight (or highlight): singled out by the conference's committee, a tier below an oral and above an ordinary acceptance."
			}
			className={cn(TAG, small && SMALL, state === "oral" ? "border-success/50 bg-success/10 text-success" : "border-ring text-foreground")}
		>
			{state === "oral" ? "Oral" : "Spotlight"}
		</span>
	);
}

/** Something the authors declare that is worth noticing (a flag of Catalog.flags, by its meaning); `note` says
 *  what raised it, on hover. */
export function FlagTag({ note, children }: { note?: string; children: React.ReactNode }) {
	return (
		<span
			title={`${note ? `${note} ` : ""}Read from the paper's PDF by a program: check the paper before relying on it.`}
			className={cn(TAG, "border-warning/50 text-warning")}
		>
			{children}
		</span>
	);
}

const SMALL = "px-1 text-[0.65rem] leading-tight";

/** One field's mean score, said as what it is: "Quality: mean score 2.3". */
const score = (field: string, mean: number) => `${field}: mean score ${Number.isInteger(mean) ? mean : mean.toFixed(1)}`;

/**
 * The scores a paper's reviewers gave, as tags: the mean per field, and on hover every reviewer's score with
 * what it means. Scales differ between conferences, so fields are never merged into one number.
 */
export function RatingTags({ ratings, small, most }: { ratings: Ratings | null | undefined; small?: boolean; most?: number }) {
	if (!ratings) return null;
	const fields = Object.entries(ratings.ratings).filter(([field]) => field !== "Confidence");
	return (
		<>
			{fields.slice(0, most).map(([field, { values, mean, meanings }]) => (
				<span
					key={field}
					title={
						`${field}, as scored by ${values.length} reviewer${values.length === 1 ? "" : "s"}: ` +
						values.map((value, index) => (meanings[index] ? `${value} (${meanings[index]})` : value)).join(", ") +
						". Read from the paper's OpenReview page; the scale is the conference's own."
					}
					className={cn(TAG, small && SMALL, "border-border text-muted-foreground")}
				>
					{score(field, mean)}
				</span>
			))}
		</>
	);
}

/**
 * The tag on a paper the AI-text detector flagged. It is a statistical estimate, so hovering it says what was
 * measured rather than leaving "AI detected" to stand as a fact.
 */
export function AiTag({
	verdict,
	small,
	hedged,
	className,
}: {
	verdict?: AiVerdict | null;
	small?: boolean;
	/** where its explanation cannot be hovered (inside a hover label), the tag itself says it is an estimate */
	hedged?: boolean;
	className?: string;
}) {
	const title = verdict
		? `An estimate from word choice, not proof. Detector score ${verdict.score.toFixed(2)}; detector scores above ${verdict.threshold.toFixed(2)} are tagged, ` +
			`which about ${Math.round(verdict.false_alarms * 100)} in 100 human-written abstracts also reach. ` +
			`What raised it most: ${verdict.markers.join(", ")}.`
		: "An estimate from word choice, not proof: this abstract's wording is of the kind language models produce.";
	return (
		<span title={title} className={cn(TAG, small && SMALL, "border-warning/50 bg-warning/10 text-warning", className)}>
			{hedged ? "AI detected (est.)" : "AI detected"}
		</span>
	);
}
