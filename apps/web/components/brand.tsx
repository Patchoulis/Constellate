import type { Acceptance, AcceptanceTerm, AiVerdict, Ratings } from "@rp/contracts";
import { Stars } from "@/components/ui/stars";
import { tagText, tagTitle } from "@/lib/acceptance";
import { averageScore, plainChip, ratingTitle } from "@/lib/ratings";
import { starsOf } from "@/lib/units";
import { cn } from "@/lib/utils";

/**
 * The app's mark: bodies on a spiral orbit. The paths are the "universe" icon of Tabler Icons
 * (https://tabler.io/icons, MIT licence, (c) Pawel Kuna); see docs/licenses/third-party-notices.md.
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

/**
 * How a conference paper was accepted, in the conference's own words ("Highlight", "Notable top 5%"), drawn as
 * Constellate's tier (lib/acceptance.ts). An ordinary acceptance is the norm, so it gets no tag.
 */
export function AcceptanceTag({ state, term, small }: { state: Acceptance | null; term?: AcceptanceTerm | null; small?: boolean }) {
	if (state !== "oral" && state !== "spotlight") return null;
	return (
		<span
			title={tagTitle(state, term)}
			className={cn(TAG, small && SMALL, state === "oral" ? "border-success/50 bg-success/10 text-success" : "border-ring text-foreground")}
		>
			{tagText(state, term)}
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

/**
 * The scores a paper's reviewers gave, as tags: per field its name and the mean as stars out of the scale's own
 * points, and on hover the score ("3.8/4 Average Score") with every reviewer's score and what it means. Scales differ
 * between conferences, so fields are never merged into one number; a field whose scale is not known says its mean
 * as a plain number.
 */
export function RatingTags({ ratings, small, most }: { ratings: Ratings | null | undefined; small?: boolean; most?: number }) {
	if (!ratings) return null;
	const fields = Object.entries(ratings.ratings).filter(([field]) => field !== "Confidence");
	return (
		<>
			{fields.slice(0, most).map(([field, figures]) => {
				const stars = figures.scale && starsOf(figures.mean, figures.scale) ? figures.scale : null;
				return (
					<span
						key={field}
						title={ratingTitle(field, figures)}
						className={cn(TAG, small && SMALL, "flex-wrap gap-x-1 border-border text-muted-foreground")}
					>
						{stars ? (
							<>
								<span className="min-w-0">{field}</span>
								<Stars value={figures.mean} scale={stars} label={averageScore(figures.mean, stars)} size={small ? "size-2" : "size-2.5"} />
							</>
						) : (
							plainChip(field, figures.mean)
						)}
					</span>
				);
			})}
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
