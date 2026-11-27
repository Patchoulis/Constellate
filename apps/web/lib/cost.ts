/**
 * What asking a language model would cost, as every feature shows it before the click and sends it with the click
 * (engine model/chosen.rs). One place, so that the seven features say the same things whichever provider is chosen:
 *
 * - a dollar figure only when the model has a price; when it has none, no dollar figure at all (not "cost not
 *   known", not 0), only the tokens it can take, which the job is then held to;
 * - a model on this computer says so: it costs no API fee;
 * - the request names the provider and model the estimate was shown for, and accepts its most, in dollars or in
 *   tokens.
 */
import type { AcceptedCost, AskingCost, BudgetOf, ShownModel } from "@rp/contracts";
import { type Currency, knownMoney, tokens } from "@/lib/units";

export const DOLLAR: Currency = { code: "USD", rate: 1, date: null };

/** Said wherever a model on this computer would be asked. */
export const THIS_COMPUTER = "runs on this computer: no API cost";

/** The model an estimate was shown for, as a request names it. */
export function shownModel(cost: Pick<AskingCost, "provider" | "model">): ShownModel {
	return { provider: cost.provider, model: cost.model };
}

/** What a request accepts of an estimate shown: its most in US dollars, or in tokens when the model has no price. */
export function accepting(cost: Pick<AskingCost, "most_usd" | "most_tokens">): AcceptedCost {
	return cost.most_usd === null ? { accepted_tokens: cost.most_tokens } : { accepted_usd: cost.most_usd };
}

/** A budget of the user's own for a job of this cost: `usd` (a whole cent above the most, say) for a model with a
 *  price, else the most it can take in tokens. */
export function budgetOf(cost: Pick<AskingCost, "most_usd" | "most_tokens">, usd?: number): BudgetOf {
	return cost.most_usd === null ? { budget_tokens: cost.most_tokens } : { budget_usd: usd ?? cost.most_usd };
}

/** A budget of the user's own rounded up to the cent (at least one) and to at most `most` dollars; in tokens for a
 *  model with no price. */
export function centBudget(cost: Pick<AskingCost, "most_usd" | "most_tokens">, most = Infinity): BudgetOf {
	if (cost.most_usd === null) return { budget_tokens: cost.most_tokens };
	return { budget_usd: Math.min(most, Math.max(0.01, Math.ceil(cost.most_usd * 100) / 100)) };
}

/** A budget in a few words: "at most USD 0.13", "at most 38k tokens". */
export function budgetWords(budget: BudgetOf, currency: Currency = DOLLAR, locale?: string): string {
	return budget.budget_usd !== undefined
		? `at most ${knownMoney(budget.budget_usd, currency, locale)}`
		: `at most ${tokens(budget.budget_tokens)}`;
}

/** Whether a dollar figure can be shown for it. */
export function priced(cost: Pick<AskingCost, "usd" | "most_usd">): boolean {
	return cost.usd !== null && cost.most_usd !== null;
}

/**
 * The cost, in a few words: "about USD 0.12, at most USD 0.25"; for a model with no price "at most 38k tokens";
 * on this computer "at most 38k tokens, runs on this computer: no API cost". Never a dollar figure that is not known.
 */
export function costWords(cost: AskingCost, currency: Currency = DOLLAR, locale?: string): string {
	const likely = knownMoney(cost.usd, currency, locale);
	const most = knownMoney(cost.most_usd, currency, locale);
	if (likely !== null && most !== null) return `about ${likely}, at most ${most}`;
	const tokenWords = `at most ${tokens(cost.most_tokens)}`;
	return cost.on_this_computer ? `${tokenWords}, ${THIS_COMPUTER}` : tokenWords;
}

/** The hover over a cost: the tokens, how they were reckoned and the prefix counted, and that it is an estimate. */
export function costTitle(cost: AskingCost): string {
	const likely = cost.tokens.input + cost.tokens.cache_read + cost.tokens.cache_write_5m + cost.tokens.cache_write_1h + cost.tokens.output;
	const caching =
		cost.caching === "not_reported"
			? "the provider reports no caching, so the shared text is counted in full for every question"
			: "the shared text is counted as cached after the first question";
	return (
		`An estimate for ${cost.model}: likely ${tokens(likely)}, at most ${tokens(cost.most_tokens)}. ` +
		`Prompt tokens are reckoned from characters (${cost.counting.chars_per_token} per token, ${cost.counting.kind}), so it errs high; ${caching}. ` +
		`No call is started that could pass the most shown.`
	);
}
