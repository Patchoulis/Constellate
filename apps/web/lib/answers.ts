/**
 * Asking a run or a community a question (engine api/answers.rs): what the Ask box shows, as plain functions and
 * one explicit state machine, so that they are tested (tests/answers.test.ts); components/ask.tsx draws them.
 */
import type { AnswerCounts, AnswerEstimate, AnswerSummary, AnswerView, PassageFound, PassagesFound, SearchCoverage } from "@rp/contracts";
import { costWords } from "@/lib/cost";
import { count, money, tokens } from "@/lib/units";

/** Said wherever a statement of an answer is shown. */
export const MODEL_NOTE =
	"Written by the model from the passages it was given: its reading, not the papers' words. The citations after it are the papers' own words, found in them.";

/** Said beside every cost before asking. */
export const ESTIMATE_NOTE =
	"An estimate, reckoned from the length of the prompt before anything is sent. The question is not asked if it could cost more than the most shown.";

const DOLLAR = { code: "USD", rate: 1, date: null };

/** "USD 0.0421": the bills are in US dollars. */
export const usd = (amount: number) => money(amount, DOLLAR);

/** What was searched: "Searched the full text of 212 of 739 papers; 527 by title and abstract only (no PDF kept)". */
export function coverageLine(coverage: SearchCoverage): string {
	const parts = [`Searched the full text of ${coverage.full_text} of ${count(coverage.papers, "paper")}`];
	const unread = coverage.no_pdf + coverage.unreadable;
	if (unread > 0) {
		const why = [
			coverage.no_pdf > 0 ? `${count(coverage.no_pdf, "paper")} with no PDF kept` : null,
			coverage.unreadable > 0 ? `${count(coverage.unreadable, "paper")} whose PDF could not be read` : null,
		].filter(Boolean);
		parts.push(`${count(unread, "paper")} by title and abstract only (${why.join(", ")})`);
	}
	if (coverage.capped > 0) parts.push(`${count(coverage.capped, "paper")} by their first passages only: the index was full`);
	if (coverage.missing > 0) parts.push(`${count(coverage.missing, "paper")} not in the library, not searched`);
	return parts.join("; ");
}

/** While the papers are being indexed: "Indexing the papers: 200 of 739 read". */
export function indexingLine(done: number, of: number): string {
	return of > 0 ? `Indexing the papers: ${done.toLocaleString()} of ${count(of, "paper")} read` : "Indexing the papers";
}

type ReadyEstimate = Extract<AnswerEstimate, { state: "ready" }>;

/** What the button that asks says: "Ask the model: about USD 0.12, at most USD 0.25"; for a model with no price,
 *  only the tokens it can take ("Ask the model: at most 9k tokens"), never a dollar figure that is not known. */
export function costLine(estimate: ReadyEstimate): string {
	if (estimate.kept) return "Open the kept answer: no cost";
	if (estimate.cost === null) return `Ask the model: at most ${tokens(estimate.most_tokens)}`;
	return `Ask the model: ${costWords(estimate.cost)}`;
}

/** What the hover over that button says: the tokens, the cap, and that it is an estimate. */
export function costTitle(estimate: ReadyEstimate): string {
	const likely = estimate.cost ? estimate.cost.tokens.input + estimate.cost.tokens.cache_write_5m + estimate.cost.tokens.output : 0;
	return (
		`One call to ${estimate.model} with ${count(estimate.passages, "passage")} from ${count(estimate.papers, "paper")} ` +
		`(passages capped at ${tokens(estimate.prompt_cap_tokens)}, the answer at ${tokens(estimate.max_answer_tokens)}): ` +
		`likely ${tokens(likely)}, at most ${tokens(estimate.most_tokens)}. ${ESTIMATE_NOTE}`
	);
}

/** Why the model cannot be asked, if it cannot. */
export function cannotAsk(estimate: ReadyEstimate): string | null {
	if (estimate.kept) return null;
	if (estimate.passages === 0) return "No passage of these papers matches the question: there is nothing to answer it from.";
	if (!estimate.api_key_set)
		return "The chosen provider's key is not set (see the settings), so the model cannot be asked. The passages found are above.";
	return null;
}

/** "3 statements left out: their quotes were not found in the papers", and those not of the form asked for. */
export function leftOutLine(counts: AnswerCounts): string | null {
	const parts = [];
	if (counts.not_found > 0) parts.push(`${count(counts.not_found, "statement")} left out: their quotes were not found in the papers`);
	if (counts.malformed > 0) parts.push(`${count(counts.malformed, "statement")} left out: not in the form asked for`);
	return parts.length > 0 ? parts.join("; ") : null;
}

/** Why an answer is not shown as statements. */
export function stateNote(view: AnswerView): string | null {
	switch (view.state) {
		case "answered":
			return null;
		case "not_in_papers":
			return "The papers in scope do not say. The model found no answer to this question in the passages it was given.";
		case "unverified":
			return `The answer could not be verified: the quotes of only ${view.counts.shown} of ${count(view.counts.written, "statement")} were found in the papers, too few to show it. The passages found are below.`;
		case "asking":
			return "The model is being asked…";
		case "unanswered":
			return "Not answered: the call failed or was stopped, or the answer was cleared. Asking again costs again.";
		case "empty":
			return "The model answered without writing a single statement: there is nothing to show. The passages found are below.";
	}
}

/** The passages of an answer: those it cites, then the others (shown collapsed). */
export function splitPassages(passages: PassageFound[]): { cited: PassageFound[]; unused: PassageFound[] } {
	return { cited: passages.filter((found) => found.cited), unused: passages.filter((found) => !found.cited) };
}

/** The questions asked of one scope: the whole run's (community null), or one community's. */
export function historyOf(questions: AnswerSummary[], community: string | null): AnswerSummary[] {
	return questions.filter((asked) => (asked.community ?? null) === community);
}

/** A passage's words, cut for a list: at most `most` characters, at a word's end. */
export function excerpt(text: string, most = 320): string {
	const letters = Array.from(text);
	if (letters.length <= most) return text;
	const cut = letters.slice(0, most).join("");
	const space = cut.lastIndexOf(" ");
	return `${space > most / 2 ? cut.slice(0, space) : cut}…`;
}

/**
 * The Ask box, as one explicit state machine:
 *
 *     idle --asked--> searching --found--> found --started--> asking --viewed--> answer
 *                     searching --indexing--> indexing --asked--> searching
 *     (any) --failed--> failed;  (any) --opened--> opening --viewed--> answer;  (any) --reset--> idle
 *
 * `found` holds what the search found and what asking would cost; nothing is sent to the model until `started`,
 * which follows the user's click on the button that shows the cost.
 */
export type AskState =
	| { kind: "idle" }
	| { kind: "searching"; question: string }
	| { kind: "indexing"; question: string; done: number; of: number }
	| { kind: "found"; question: string; found: Extract<PassagesFound, { state: "ready" }>; estimate: ReadyEstimate }
	| { kind: "asking"; question: string; id: string; job: number }
	| { kind: "opening"; id: string }
	| { kind: "answer"; view: AnswerView }
	| { kind: "failed"; question: string | null; error: string };

export type AskEvent =
	| { type: "asked"; question: string }
	| { type: "searched"; found: PassagesFound; estimate: AnswerEstimate }
	| { type: "started"; id: string; job: number }
	| { type: "opened"; id: string }
	| { type: "viewed"; view: AnswerView }
	| { type: "failed"; error: string }
	| { type: "reset" };

function questionOf(state: AskState): string | null {
	return "question" in state ? state.question : state.kind === "answer" ? state.view.question : null;
}

export function reduceAsk(state: AskState, event: AskEvent): AskState {
	switch (event.type) {
		case "asked":
			return { kind: "searching", question: event.question };
		case "searched": {
			if (state.kind !== "searching") return state; // an answer to a search no longer wanted
			const { found, estimate } = event;
			if (found.state === "indexing") return { kind: "indexing", question: state.question, done: found.done, of: found.of };
			if (estimate.state === "indexing") return { kind: "indexing", question: state.question, done: estimate.done, of: estimate.of };
			return { kind: "found", question: state.question, found, estimate };
		}
		case "started":
			return state.kind === "found" ? { kind: "asking", question: state.question, id: event.id, job: event.job } : state;
		case "opened":
			return { kind: "opening", id: event.id };
		case "viewed":
			// while asking, an answer still being asked keeps the box waiting
			if (state.kind === "asking" && (event.view.id !== state.id || event.view.state === "asking")) return state;
			// an answer opened, or the one on screen looked at again; any other is no longer wanted
			if (state.kind === "opening" && event.view.id !== state.id) return state;
			if (state.kind === "answer" && event.view.id !== state.view.id) return state;
			if (state.kind !== "asking" && state.kind !== "opening" && state.kind !== "answer") return state;
			return { kind: "answer", view: event.view };
		case "failed":
			return { kind: "failed", question: questionOf(state), error: event.error };
		case "reset":
			return { kind: "idle" };
	}
}
