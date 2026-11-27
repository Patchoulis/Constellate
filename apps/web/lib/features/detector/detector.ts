/**
 * Training the AI-text detector: what is said before the click (`GET /api/detector/estimate`, an estimate) and
 * while the job runs. The detector learns from abstracts the chosen model writes, so training costs what writing
 * them costs; what was written before is kept and asked for never again.
 */
import type { DetectorEstimate, JobProgress } from "@rp/contracts";
import { costWords } from "@/lib/cost";
import { count, type Currency, knownMoney } from "@/lib/units";

export interface DetectorEstimateWords {
	lines: string[];
	/** "It costs about USD 0.40, at most USD 0.90 (an estimate)."; null when nothing is asked */
	cost: string | null;
	/** nothing to write: training again asks nothing and costs nothing */
	free: boolean;
	/** why it cannot be trained now (no key for the provider); null when it can */
	cannot: string | null;
}

/** What the detector's estimate says, in sentences with their units. */
export function detectorEstimateLines(estimate: DetectorEstimate, currency?: Currency, locale?: string): DetectorEstimateWords {
	const lines = [
		`It learns from ${count(estimate.pool, "human abstract")} of 2019, of which ${estimate.pool_kept.toLocaleString("en-US")} ${estimate.pool_kept === 1 ? "is" : "are"} kept already, and from abstracts a language model writes for their titles.`,
	];
	if (estimate.to_generate === 0) {
		lines.push(
			estimate.built
				? "Everything it learns from was written before: training again asks no model and costs nothing."
				: `Every abstract it needs was written before (${count(estimate.mirrors_kept, "abstract")} kept): training asks no model and costs nothing.`,
		);
		return { lines, cost: null, free: true, cannot: null };
	}
	lines.push(
		`${estimate.model} would write up to ${count(estimate.to_generate, "abstract")}: the text it learns to recognise${
			estimate.mirrors_kept > 0 ? ` (${count(estimate.mirrors_kept, "abstract")} written before are kept and cost nothing)` : ""
		}.`,
	);
	if (!estimate.api_key_set) {
		return {
			lines,
			cost: null,
			free: false,
			cannot: `No key is set for ${estimate.model}'s provider: Language model, below, says which variable to set.`,
		};
	}
	return { lines, cost: `It costs ${costWords(estimate.cost, currency, locale)} (an estimate).`, free: false, cannot: null };
}

/** What the running job is doing: "Writing abstracts: 120 of 600 (USD 0.08 spent so far)". */
export function trainingLine(kind: "build" | "years", progress: JobProgress | null, currency: Currency, locale?: string): string {
	// the check by year counts nothing it says (engine features/detector/training.rs `check_years`)
	if (kind === "years") return "Scoring a week of arXiv abstracts in each year (arXiv only, no cost)…";
	if (!progress) return "Training the detector…";
	const spent = knownMoney(progress.usd > 0 ? progress.usd : null, currency, locale);
	return `Training the detector: ${progress.done.toLocaleString("en-US")} of ${count(progress.of, "abstract")} written${spent ? ` (${spent} spent so far)` : ""}`;
}
