/**
 * "Explain this" in the reader, as plain functions (tests/explanation.test.ts): how a selection becomes a place in
 * the paper, when to ask before spending, and what is said about costs, garbled text and statements left out.
 * components/passage-explainer.tsx draws them.
 */
import type {
	Explanation,
	ExplanationEstimate,
	ExplanationPart,
	ExplanationStatement,
	LocateAnswer,
	PaperPassages,
	PassageAddress,
	PassageKind,
} from "@rp/contracts";
import { passageAt } from "@/lib/evidence";
import { costWords } from "@/lib/cost";
import { count, tokens } from "@/lib/units";

/** The longest selection looked for, in characters: as the engine's locate route allows (api/passages.rs). */
export const LONGEST_SELECTION = 5_000;

/** Said wherever an explanation's statements are shown. */
export const MODEL_NOTE =
	"Written by the model from the paper's text: a reading of it, not the paper's own words. The quotes are the paper's.";

/** Said beside a statement of general knowledge. */
export const BACKGROUND_NOTE =
	"Background knowledge from the model, not from this paper: it has no place in the paper to link to. Check it elsewhere before relying on it.";

/** Where a selection lands, or why it does not. */
export type Resolution = { kind: "passage"; index: number; note: string | null } | { kind: "refused"; why: string };

/** The selected text, as it is sent to be located, or why it is not sent. */
export function selectionQuote(selected: string): { quote: string } | { refused: string } | null {
	const quote = selected.replace(/\s+/g, " ").trim();
	if (!quote) return null;
	if (Array.from(quote).length > LONGEST_SELECTION)
		return { refused: `The selection is longer than ${count(LONGEST_SELECTION, "character")}: select a paragraph or less.` };
	return { quote };
}

const EXPLAINABLE: PassageKind[] = ["text", "equation", "figure_caption", "table_caption", "footnote", "fragment"];

/** Can a passage of this kind be explained? Titles and headings have nothing to explain. */
export function explainable(kind: PassageKind): boolean {
	return EXPLAINABLE.includes(kind);
}

/**
 * The passage a selection is explained as. `located` is where the engine found the selected words; `within` is the
 * passage the selection was made in on the page, when it lies in one. Short words ("α") are found at their first
 * place in the paper, which may be another passage than the one selected in: the one selected in wins then, since
 * the words were found in the paper and are visibly there. Words not found in the paper are refused: what is
 * explained is a place in the paper, not free text.
 */
export function resolveSelection(passages: PaperPassages, located: LocateAnswer, within: number | null): Resolution {
	if (!located.found)
		return { kind: "refused", why: "These words were not found in the paper's text, so they cannot be explained as a place in it." };
	const at = passageAt(passages, located.address);
	const index = within !== null && within >= 0 && within < passages.passages.length && within !== at ? within : at;
	const passage = passages.passages[index];
	if (!passage) return { kind: "refused", why: "These words are not in a passage of the paper's text." };
	if (!explainable(passage.kind))
		return { kind: "refused", why: "A title or a heading cannot be explained: select words in a paragraph, an equation or a caption." };
	const runsOn =
		index === at &&
		(located.address.end_page > passage.address.end_page ||
			(located.address.end_page === passage.address.page && located.address.end > passage.address.end));
	return {
		kind: "passage",
		index,
		note: runsOn ? "The selection runs past this paragraph: the paragraph it starts in is explained." : null,
	};
}

/** A passage's address as a key, for the set of passages with an explanation kept. */
export function placeKey(address: PassageAddress): string {
	return `${address.page}:${address.start}`;
}

/** The localStorage key of "do not ask again below": this browser only. */
export const ASK_BELOW_KEY = "constellate.explain.ask-below-usd";

/** The amount below which an explanation is asked for without a confirmation; null (the default) always asks. */
export function readAskBelow(stored: string | null): number | null {
	if (stored === null) return null;
	const amount = Number(stored);
	return Number.isFinite(amount) && amount > 0 ? amount : null;
}

/**
 * Must the reader confirm before asking? Always, unless they chose to be asked only above an amount and the most
 * this can cost (not the likely cost) is known and below it.
 */
export function askFirst(estimate: ExplanationEstimate, askBelow: number | null): boolean {
	if (askBelow === null || estimate.most_usd === null) return true;
	return estimate.most_usd > askBelow;
}

/** Everything that decides whether a passage's explanation is asked for without a click. */
export interface Unconfirmed {
	/** the reader's "do not ask below" amount, as it was when the panel opened; null: off (the default) */
	askBelow: number | null;
	/** the estimate the panel shows, when it is the one for this passage and the model chosen now (null otherwise) */
	current: ExplanationEstimate | null;
	/** is this the first answer the panel got for the passage? a later one (another model, read again) never asks */
	first: boolean;
	keySet: boolean;
	/** was this passage already asked about, redone or stopped while the paper is open? */
	askedBefore: boolean;
	/** is nothing being asked for it (no job starting or running, no failure shown)? */
	idle: boolean;
}

/**
 * May the explanation be asked for with no click? Only when the reader turned it on and chose the amount, the
 * estimate is the current one and has a known price whose most is within that amount, and it is the passage's
 * first estimate with nothing asked for it yet. Any doubt is a "no": the reader clicks.
 */
export function asksUnconfirmed(asking: Unconfirmed): boolean {
	const { askBelow, current, first, keySet, askedBefore, idle } = asking;
	if (askBelow === null || current === null || !first || !keySet || askedBefore || !idle) return false;
	return !askFirst(current, askBelow);
}

const USD = { code: "USD", rate: 1, date: null };

/** What explaining would cost, with its currency, said to be an estimate; for a model with no price, only its
 *  tokens (no dollar figure at all). */
export function costLine(estimate: ExplanationEstimate, locale?: string): string {
	const { input, cache_read, cache_write_5m, cache_write_1h, output } = estimate.tokens;
	const asked = `${tokens(input + cache_read + cache_write_5m + cache_write_1h)} sent, about ${tokens(output)} back`;
	return `Estimate: ${costWords(estimate, USD, locale)} (${asked}).`;
}

/** How the estimate was reckoned, for its hover. */
export function costNote(estimate: ExplanationEstimate): string {
	const rate = `${estimate.counting.chars_per_token.toFixed(1)} characters a token`;
	return estimate.counting.kind === "assumed"
		? `An estimate: the prompt's tokens are reckoned from its characters at ${rate}, assumed (it errs high). "At most" is if the answer takes all the room it is given. Amounts are in US dollars, the currency the API bills in.`
		: `An estimate: the prompt's tokens are reckoned from its characters at ${rate}, as measured on an earlier prompt. Amounts are in US dollars, the currency the API bills in.`;
}

/** What is said about the passage's text as sent: an equation's or a table's extracted text may be garbled. */
export function garbledNote(kind: PassageKind): string | null {
	switch (kind) {
		case "equation":
			return "This equation is sent as the text the PDF gives for it, not as an image: symbols, subscripts and layout are often lost, so the explanation may misread it. Compare with the PDF.";
		case "table_caption":
			return "Only text is sent: the caption and the table's cells as extracted from the PDF, where a table's layout is usually lost. Compare with the PDF.";
		case "figure_caption":
			return "Only text is sent: the figure itself is not seen, only its caption and the text around it.";
		case "fragment":
			return "This is text drawn inside a figure or a table, as extracted from the PDF: it may be garbled.";
		default:
			return null;
	}
}

/** What was left out of an explanation, said in numbers; null when nothing was. */
export function leftOutNote(leftOut: Explanation["left_out"]): string | null {
	const said = [];
	if (leftOut.not_found > 0) said.push(`${count(leftOut.not_found, "statement")} left out: their quotes were not found in the paper`);
	if (leftOut.past_bound > 0) said.push(`${count(leftOut.past_bound, "statement")} left out: past the most read from one answer`);
	if (leftOut.malformed > 0) said.push(`${count(leftOut.malformed, "statement")} left out: not of the form asked for`);
	return said.length ? `${said.join("; ")}.` : null;
}

export const PART_HEADING: Record<ExplanationPart, string> = {
	explanation: "In plain words",
	symbol: "The symbols",
	reading: "How to read it",
	prerequisite: "Worth knowing first",
};

const PART_ORDER: ExplanationPart[] = ["explanation", "reading", "symbol", "prerequisite"];

/** The statements by part, in a fixed order of parts, each part in the model's order; empty parts left out. */
export function byPart(statements: ExplanationStatement[]): { part: ExplanationPart; statements: ExplanationStatement[] }[] {
	return PART_ORDER.map((part) => ({ part, statements: statements.filter((statement) => statement.part === part) })).filter(
		(group) => group.statements.length > 0,
	);
}

/** A symbol as MathText typesets it: the model writes it in LaTeX, with or without dollar signs. */
export function symbolSource(symbol: string): string {
	const bare = symbol.trim().replace(/^\$+|\$+$/g, "");
	return bare ? `$${bare}$` : "";
}

/** What was searched for related passages, in words. */
export function searchedLine(searched: {
	nearby: number;
	bound: number;
	by_text: number;
	by_abstract: number;
	not_in_library: number;
}): string {
	if (searched.nearby === 0) return "This paper has no neighbours in this run's graph and no community to search.";
	const parts = [`${count(searched.by_text, "paper")} by their text`];
	if (searched.by_abstract) parts.push(`${count(searched.by_abstract, "paper")} by their abstract only (no PDF read)`);
	if (searched.not_in_library) parts.push(`${count(searched.not_in_library, "paper")} not in the library`);
	return `Searched the ${count(searched.nearby, "nearest paper")} of this run (at most ${count(searched.bound, "paper")}: its neighbours in the graph, then its community): ${parts.join(", ")}.`;
}
