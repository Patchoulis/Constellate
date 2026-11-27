/**
 * Citing sentences and the kinds of citation, as they are said and drawn (engine analysis/citation_contexts.rs and
 * analysis/citation_kinds.rs). Pure functions, tested in tests/citation-kinds.test.ts.
 *
 * Every figure is a count of citing sentences with what it is out of, never a score; a kind is the model's reading
 * and is only ever shown beside the sentence it is about.
 */
import type {
	CitationGrouping,
	CitationJudgement,
	CitationKind,
	CitationKindsEstimate,
	CitationSection,
	CitationTally,
	CitingSentenceShown,
	PaperCitationContexts,
	BudgetOf,
} from "@rp/contracts";
import { centBudget } from "@/lib/cost";
import { count } from "@/lib/units";

/** Said wherever a kind is shown. */
export const KINDS_NOTE =
	"The model's reading of the citing sentence, an estimate: a kind other than a mention is shown only when the words that show it were found in the sentence or the sentence beside it, and those words are linked. Read the sentence before relying on it.";

export const SENTENCES_NOTE =
	"Found by a program in the citing paper's own text, with no model: the sentence whose citation marker points at this paper's entry in its reference list. Only papers whose PDFs Constellate has read are counted.";

/** The kinds in the order they are listed. */
export const KINDS: readonly CitationKind[] = ["baseline", "uses", "uses_data", "extends", "supports", "contrasts", "mention"];

/** How a kind is said in a list of counts: "6 as a baseline". */
const COUNTED: Record<CitationKind, string> = {
	baseline: "as a baseline",
	uses: "used",
	uses_data: "data or benchmark used",
	extends: "extended",
	supports: "supported",
	contrasts: "contrasted",
	mention: "mentions",
};

/** How a kind is said beside its sentence. */
export const KIND_LABEL: Record<CitationKind, string> = {
	baseline: "compared against as a baseline",
	uses: "uses its method",
	uses_data: "uses its data or benchmark",
	extends: "extends it",
	supports: "agrees with it",
	contrasts: "contrasts with it",
	mention: "mentions it",
};

/** A hue per kind, for the tags and the network's lines; mentions have none. */
export const KIND_HUE: Partial<Record<CitationKind, number>> = {
	baseline: 42,
	uses: 205,
	uses_data: 265,
	extends: 175,
	supports: 135,
	contrasts: 355,
};

/**
 * "cited 14 times in this library: 6 as a baseline, 3 used, 1 contrasted, 2 mentions, 2 not classified". Every
 * part is a count of citing sentences; together they make the first.
 */
export function tallyLine(tally: CitationTally, what = "cited"): string {
	const parts = KINDS.filter((kind) => kind !== "mention" && (tally.kinds[kind] ?? 0) > 0).map(
		(kind) => `${tally.kinds[kind]} ${COUNTED[kind]}`,
	);
	if (tally.mentions > 0) parts.push(count(tally.mentions, "mention"));
	if (tally.not_justified > 0) parts.push(`${tally.not_justified} not justified (counted as mentions)`);
	if (tally.not_classified > 0) parts.push(`${tally.not_classified} not classified`);
	const times = `${what} ${count(tally.sentences, "time")} in this library`;
	return parts.length ? `${times}: ${parts.join(", ")}` : times;
}

/** Whether any of a tally's sentences has been read by the model. */
export function classified(tally: CitationTally): boolean {
	return tally.not_classified < tally.sentences;
}

/** "Experiments", "Related work", "“3 Spectral Calibration”". */
export function sectionName(section: CitationSection): string {
	switch (section.kind) {
		case "related_work":
			return "Related work";
		case "other":
			return section.title ? `“${section.title}”` : "Before the first heading";
		default:
			return section.kind.charAt(0).toUpperCase() + section.kind.slice(1);
	}
}

/** "cited alone", "cited with 3 other works". */
export function groupingNote(grouping: CitationGrouping): string {
	return grouping.kind === "alone" ? "cited alone" : `cited with ${count(grouping.with, "other work")}`;
}

/** What the reading of one sentence is, in words, and whether it is a kind that was justified. */
export function judgementLabel(judgement: CitationJudgement): { label: string; note: string; kind: CitationKind | null } {
	switch (judgement.state) {
		case "justified":
			return { label: KIND_LABEL[judgement.kind], note: KINDS_NOTE, kind: judgement.kind };
		case "mention":
			return { label: KIND_LABEL.mention, note: KINDS_NOTE, kind: "mention" };
		case "not_justified":
			return {
				label: "mention (not justified)",
				note: `The model read it as “${KIND_LABEL[judgement.claimed]}”, but the words it gave were not found in the sentence or beside it: it is counted as a mention.`,
				kind: null,
			};
		case "not_classified":
			return { label: "not classified", note: "The model has not been asked about this sentence.", kind: null };
	}
}

/** The sentences of a list read as `kind` (all of them when null). */
export function sentencesOfKind(sentences: CitingSentenceShown[], kind: CitationKind | null): CitingSentenceShown[] {
	if (!kind) return sentences;
	return sentences.filter((sentence) =>
		kind === "mention"
			? sentence.judgement.state === "mention" || sentence.judgement.state === "not_justified"
			: sentence.judgement.state === "justified" && sentence.judgement.kind === kind,
	);
}

/** The kind most of a tally's justified readings are, other than mention (ties: in the order of KINDS); null when none. */
export function mainKind(tally: CitationTally): CitationKind | null {
	let best: CitationKind | null = null;
	for (const kind of KINDS) {
		if (kind === "mention") continue;
		const times = tally.kinds[kind] ?? 0;
		if (times > 0 && (best === null || times > (tally.kinds[best] ?? 0))) best = kind;
	}
	return best;
}

/** The key of a citation from one paper to another: what `edgeKinds` is keyed by. */
export const edgeKey = (citing: string, cited: string) => `${citing}>${cited}`;

/** The main kind of each citation between the paper and the papers it cites or that cite it, by `edgeKey`. */
export function edgeKinds(contexts: PaperCitationContexts): Map<string, CitationKind> {
	const kinds = new Map<string, CitationKind>();
	for (const cited of contexts.cites) {
		const kind = mainKind(cited.tally);
		if (kind) kinds.set(edgeKey(contexts.paper, cited.paper), kind);
	}
	for (const citing of contexts.cited_by) {
		const kind = mainKind(citing.tally);
		if (kind) kinds.set(edgeKey(citing.paper, contexts.paper), kind);
	}
	return kinds;
}

/** The paper a citation network's key names ("paper:<id>"), or null for a work outside the library. */
const paperOf = (key: string) => (key.startsWith("paper:") ? key.slice("paper:".length) : null);

/**
 * The kind of a line of the citation network's picture (lib/reader/citations.ts `layoutNetwork`), when one is known: a
 * line from the seed to a work it cites, from a work citing it to the seed, or between two works (from cites to).
 */
export function lineKind(
	works: { key: string; side: "seed" | "prior" | "derivative" }[],
	line: { from: number; to: number; seed: boolean },
	kinds: Map<string, CitationKind> | undefined,
): CitationKind | null {
	if (!kinds?.size) return null;
	const from = paperOf(works[line.from]?.key ?? "");
	const to = paperOf(works[line.to]?.key ?? "");
	if (!from || !to) return null;
	const [citing, cited] = line.seed && works[line.to]?.side === "derivative" ? [to, from] : [from, to];
	return kinds.get(edgeKey(citing, cited)) ?? null;
}

/** How far the citing sentences of a run's (or a community's) papers were read; `unreadable` is absent from older engines. */
export interface SentencesCoverage {
	papers: number;
	read: number;
	without_pdf: number;
	/** papers with a PDF that could not be read: neither read nor waiting to be */
	unreadable?: number;
}

/** "Citing sentences read for 212 of 739 papers of this run: 480 have no PDF kept, 7 have one that cannot be read". */
export function coverageLine(coverage: SentencesCoverage): string {
	const base = `Citing sentences read for ${coverage.read.toLocaleString()} of ${count(coverage.papers, "paper")} of this run`;
	const unreadable = coverage.unreadable ?? 0;
	const why = [
		coverage.without_pdf > 0 ? `${count(coverage.without_pdf, "has", "have")} no PDF kept` : null,
		unreadable > 0 ? `${count(unreadable, "has", "have")} one that cannot be read` : null,
	].filter((part) => part !== null);
	return why.length > 0 ? `${base}: ${why.join(", ")}` : base;
}

/** Are there papers of the run with a readable PDF whose citing sentences have not been read? */
export function moreToRead(coverage: SentencesCoverage): boolean {
	return coverage.read < coverage.papers - coverage.without_pdf - (coverage.unreadable ?? 0);
}

/**
 * The cap a job is started with: the estimate's worst case (every answer as long as allowed, nothing cached),
 * rounded up to the cent; for a model with no price, its worst case in tokens.
 */
export function budgetFor(estimate: CitationKindsEstimate): BudgetOf {
	return centBudget(estimate.estimate);
}

/** Tokens an estimate expects per citing sentence asked, all kinds of token together. */
export function tokensPerSentence(estimate: CitationKindsEstimate): number | null {
	if (estimate.to_ask === 0) return null;
	const { input, output, cache_read, cache_write_5m, cache_write_1h } = estimate.estimate.tokens;
	return Math.round((input + output + cache_read + cache_write_5m + cache_write_1h) / estimate.to_ask);
}
