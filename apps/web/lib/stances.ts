/**
 * Where a community's papers stand on a question (engine analysis/stances.rs), as the interface says it: the
 * groups and their counts with what they are counts of, broken down by acceptance tier and by year, and the sets
 * of papers each group holds, for the graph. Never one verdict: a placement is the model's reading of one paper,
 * and every paper counts once, whatever its design.
 */
import type { Acceptance, StanceCostEstimate, StanceFilter, StanceGroup, StanceResult, StanceRow } from "@rp/contracts";
import type { Colouring } from "@/lib/graph-colouring";
import { costWords } from "@/lib/cost";
import { count, type Currency } from "@/lib/units";

/** Said wherever placements are shown. */
export const STANCE_NOTE =
	"Each placement is the model's reading of one paper, shown with the paper's own words. It can be wrong: read the quotes. Every paper counts once, whatever its design or size, so the counts are not a verdict.";

/** The groups papers are counted in, in the order shown, and those shown apart (never counted for or against). */
export const COUNTED: readonly StanceGroup[] = ["supports", "mixed", "contradicts", "not_addressed"];
export const APART: readonly StanceGroup[] = ["not_verified", "unusable"];

export const GROUP_LABEL: Record<StanceGroup, string> = {
	supports: "Supports",
	mixed: "Mixed or conditional",
	contradicts: "Contradicts",
	not_addressed: "Does not address it",
	not_verified: "Not verified",
	unusable: "No usable answer",
};

export const GROUP_NOTE: Record<StanceGroup, string> = {
	supports: "The model read the paper's findings as saying yes; the quotes are the paper's.",
	mixed: "The model read the paper as saying yes only under a condition (in the model's words), or yes and no in different settings.",
	contradicts: "The model read the paper's findings as saying no; the quotes are the paper's.",
	not_addressed: "The model found no position on the question in the paper.",
	not_verified:
		"The model placed these papers, but its quotes were not found in them: what it said is not shown, and they are not counted for or against.",
	unusable: "The model's answer was not one the question allows (a mixed stance without its condition): ask again with Redo.",
};

/** The hue each group is drawn in, on the panel and on the graph (stanceColouring); null for grey. */
export const GROUP_HUE: Record<StanceGroup, number | null> = {
	supports: 145,
	mixed: 42,
	contradicts: 0,
	not_addressed: null,
	not_verified: null,
	unusable: null,
};

export type Tally = Record<StanceGroup, number>;

export function tally(rows: StanceRow[]): Tally {
	const counts: Tally = { supports: 0, mixed: 0, contradicts: 0, not_addressed: 0, not_verified: 0, unusable: 0 };
	for (const row of rows) counts[row.placed.group] += 1;
	return counts;
}

/** "9 papers support, 3 are mixed, 2 contradict, 31 do not address it; 4 not verified". */
export function countsLine(counts: Tally): string {
	const one = (amount: number, singular: string, plural: string) => `${count(amount, "paper")} ${amount === 1 ? singular : plural}`;
	const counted = [
		one(counts.supports, "supports", "support"),
		one(counts.mixed, "is mixed", "are mixed"),
		one(counts.contradicts, "contradicts", "contradict"),
		one(counts.not_addressed, "does not address it", "do not address it"),
	].join(", ");
	const apart = [
		counts.not_verified > 0 ? `${count(counts.not_verified, "paper")} not verified` : null,
		counts.unusable > 0 ? `${count(counts.unusable, "paper")} with no usable answer` : null,
	].filter((part): part is string => part !== null);
	return apart.length ? `${counted}; ${apart.join(", ")}` : counted;
}

/** One line of a breakdown: a tier or a year, and how its papers were placed. */
export interface Breakdown {
	key: string;
	label: string;
	counts: Tally;
	papers: number;
}

const TIERS: readonly (Acceptance | "none")[] = ["oral", "spotlight", "poster", "none"];
const TIER_LABEL: Record<Acceptance | "none", string> = {
	oral: "Orals",
	spotlight: "Spotlights",
	poster: "Posters",
	none: "Not from a conference",
};

/** The placements by acceptance tier (orals first) or by year (oldest first); groups with no paper left out. */
export function breakdown(rows: StanceRow[], by: "tier" | "year"): Breakdown[] {
	const groups = new Map<string, StanceRow[]>();
	for (const row of rows) {
		const key = by === "tier" ? (row.tier ?? "none") : row.year === null ? "unknown" : String(row.year);
		groups.set(key, [...(groups.get(key) ?? []), row]);
	}
	const keys =
		by === "tier"
			? TIERS.filter((tier) => groups.has(tier))
			: [...groups.keys()].sort((a, b) => (a === "unknown" ? 1 : b === "unknown" ? -1 : Number(a) - Number(b)));
	return keys.map((key) => {
		const placed = groups.get(key) ?? [];
		const label = by === "tier" ? TIER_LABEL[key as Acceptance | "none"] : key === "unknown" ? "Year not known" : key;
		return { key, label, counts: tally(placed), papers: placed.length };
	});
}

/** "412 of 739 papers skipped: nothing in their title or abstract shares a word with the question (synthetic, pretrain)". */
export function skippedLine(filter: StanceFilter): string | null {
	if (filter.unrelated === 0) return null;
	const words = filter.terms.length ? ` (${filter.terms.join(", ")})` : "";
	const what = `${filter.unrelated} of ${count(filter.papers, "paper")}`;
	return filter.include_unrelated
		? `${what} share no word with the question${words}, and are included as you asked.`
		: `${what} skipped: nothing in their title or abstract shares a word with the question${words}.`;
}

/** "4 papers not in the library: neither asked nor skipped." The scope's papers the library does not have. */
export function notInLibraryLine(filter: StanceFilter): string | null {
	const missing = filter.not_in_library ?? 0;
	if (missing === 0) return null;
	return `${count(missing, "paper")} not in the library: neither asked nor skipped.`;
}

/** What of one placement's quotes was not found in its paper: "2 quotes the model gave were not found in the
 *  paper: left out." (a placement still shown on the quotes that were); for one not verified, that none was. */
export function quotesNotFoundLine(placed: StanceRow["placed"]): string | null {
	if (placed.group === "not_verified")
		return `${placed.quotes_not_found === 1 ? "The 1 quote" : `None of the ${count(placed.quotes_not_found, "quote")}`} the model gave ${placed.quotes_not_found === 1 ? "was not" : "was"} found in the paper: what it said is not shown.`;
	if (!("quotes_not_found" in placed) || !placed.quotes_not_found) return null;
	const missing = placed.quotes_not_found;
	return `${count(missing, "quote")} the model gave ${missing === 1 ? "was" : "were"} not found in the paper: left out.`;
}

/** "Read 10 of 45 papers to ask: 35 not asked yet". */
export function coverageLine(result: StanceResult): string {
	const rest = result.to_ask - result.answered;
	const asked = `Answers for ${result.answered} of ${count(result.to_ask, "paper")} to ask`;
	return rest > 0 ? `${asked}: ${count(rest, "paper")} not asked yet.` : `${asked}.`;
}

/** "with 3 papers read from their abstract only". */
export function abstractOnlyLine(rows: StanceRow[]): string | null {
	const fromAbstract = rows.filter((row) => row.read === "abstract").length;
	if (fromAbstract === 0) return null;
	return `${count(fromAbstract, "paper")} read from the abstract only (no PDF kept): a weaker reading, marked "abstract".`;
}

/** What a part would cost, labelled as an estimate: "about USD 0.42, at most USD 1.10 (an estimate)", or that it is free. */
export function costLine(estimate: StanceCostEstimate, currency: Currency, locale?: string): string {
	if (estimate.to_ask === 0) return "nothing to pay: every answer is kept";
	// a model with no price: only the tokens it can take, never a dollar figure that is not known
	return `${costWords(estimate, currency, locale)} (an estimate)`;
}

/** The hover text of a cost: how it was reckoned. */
export function costNote(estimate: StanceCostEstimate): string {
	const kept = estimate.kept ? ` ${count(estimate.kept, "answer")} kept from before cost nothing.` : "";
	const counted = estimate.counting.kind === "measured" ? "measured" : "assumed";
	return `An estimate, not a bill: tokens are reckoned from the papers' characters (a ratio ${counted}), the answers' length is a guess, and repeated prompts are counted as cache reads.${kept} The job never spends more than the "at most" figure.`;
}

/** A question to start from, for a term the community's papers keep using. */
export function starter(term: string): string {
	return `Does ${term} help?`;
}

/**
 * The graph coloured by where the papers stand (lib/graph-colouring.ts): each group that has papers, in the order
 * shown, in its hue (grey for those not counted for or against). The graph finds the papers by id; those not in
 * the run are counted apart in its legend.
 */
export function stanceColouring(question: string, rows: StanceRow[]): Colouring {
	return {
		title: `“${question}” (the model's reading)`,
		groups: [...COUNTED, ...APART].flatMap((group) => {
			const papers = rows.filter((row) => row.placed.group === group).map((row) => row.paper);
			return papers.length > 0 ? [{ name: GROUP_LABEL[group], hue: GROUP_HUE[group], papers }] : [];
		}),
	};
}

/** The rows of one group, most central first (the engine's order). */
export function rowsOf(rows: StanceRow[], group: StanceGroup): StanceRow[] {
	return rows.filter((row) => row.placed.group === group);
}
