/** What asking a model would cost, as every feature shows it and sends it (lib/cost.ts, lib/units.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { accepting, budgetOf, budgetWords, centBudget, costTitle, costWords, shownModel } from "@/lib/cost";
import { knownMoney } from "@/lib/units";
import { askingCost } from "./costs";

const USD = { code: "USD", rate: 1, date: null };

test("an amount that is not known is shown as nothing at all, never as 0 or a placeholder", () => {
	assert.equal(knownMoney(0.42, USD, "en-US"), "USD 0.4200");
	assert.equal(knownMoney(0, USD, "en-US"), "USD 0.00", "a known nothing is still an amount");
	for (const unknown of [null, undefined, Number.NaN]) assert.equal(knownMoney(unknown, USD), null);
});

test("a cost with a price is said in dollars; one without, only in the tokens it can take", () => {
	assert.equal(costWords(askingCost(0.12, 0.25), USD, "en-US"), "about USD 0.1200, at most USD 0.2500");
	const unpriced = askingCost(null, null, { provider: "compatible", model: "qwen3:32b" });
	assert.equal(costWords(unpriced), "at most 28k tokens");
	assert.doesNotMatch(costWords(unpriced), /USD|not known|price/);
	assert.equal(costWords({ ...unpriced, on_this_computer: true }), "at most 28k tokens, runs on this computer: no API cost");
	assert.match(costTitle(unpriced), /at most 28k tokens/);
	assert.match(costTitle({ ...unpriced, caching: "not_reported" }), /in full for every question/);
});

test("a request names the model it was shown for and accepts its most, in dollars or in tokens", () => {
	const priced = askingCost(0.12, 0.25, { provider: "openai", model: "gpt-6.2-sol" });
	assert.deepEqual(shownModel(priced), { provider: "openai", model: "gpt-6.2-sol" });
	assert.deepEqual(accepting(priced), { accepted_usd: 0.25 });
	assert.deepEqual(accepting(askingCost(null, null)), { accepted_tokens: 28_000 });
	assert.deepEqual(budgetOf(priced, 0.3), { budget_usd: 0.3 });
	assert.deepEqual(budgetOf(askingCost(null, null), 0.3), { budget_tokens: 28_000 }, "no dollars for a model with no price");
	assert.deepEqual(centBudget(askingCost(0.1, 0.1234)), { budget_usd: 0.13 });
	assert.deepEqual(centBudget(askingCost(0.1, 30), 10), { budget_usd: 10 });
	assert.equal(budgetWords({ budget_usd: 0.13 }, USD, "en-US"), "at most USD 0.1300");
	assert.equal(budgetWords({ budget_tokens: 28_000 }), "at most 28k tokens");
});
