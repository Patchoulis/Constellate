/** Numbers are shown with what they are numbers of. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { count, currencyOf, money, percent, similarity, tokens } from "@/lib/units";

test("a count names what is counted, in the singular for one", () => {
	assert.equal(count(1, "paper"), "1 paper");
	assert.equal(count(12487, "paper"), "12,487 papers");
	assert.equal(count(24, "community", "communities"), "24 communities");
	assert.equal(count(0, "review"), "0 reviews");
});

test("a similarity says it is a cosine similarity", () => {
	assert.equal(similarity(0.4512), "0.45 cosine similarity");
	assert.equal(similarity(0.2134, 3), "0.213 cosine similarity");
});

test("shares and tokens carry their unit", () => {
	assert.equal(percent(0.118), "12%");
	assert.equal(tokens(38000), "38k tokens");
	assert.equal(tokens(1234567), "1.2M tokens");
	assert.equal(tokens(512), "512 tokens");
});

test("the currency follows the user's region, and is the dollar where that is unknown", () => {
	assert.equal(currencyOf("en-US"), "USD");
	assert.equal(currencyOf("en-CA"), "CAD");
	assert.equal(currencyOf("fr-CA"), "CAD");
	assert.equal(currencyOf("de-DE"), "EUR");
	assert.equal(currencyOf("en-GB"), "GBP");
	assert.equal(currencyOf("ja"), "JPY"); // a language alone: its most likely region
	assert.equal(currencyOf("xx-ZZ"), "USD");
	assert.equal(currencyOf(undefined), "USD");
	assert.equal(currencyOf("not a locale!"), "USD");
});

test("money is converted from dollars and always shown with its currency's code", () => {
	const dollar = { code: "USD", rate: 1, date: null };
	const canadian = { code: "CAD", rate: 1.423, date: "2026-11-26" };
	assert.equal(money(1.5, dollar, "en-US"), "USD 1.50");
	assert.equal(money(1.5, canadian, "en-CA"), "CAD 2.13");
	assert.equal(money(0, dollar, "en-US"), "USD 0.00");
	assert.equal(money(0.0042, dollar, "en-US"), "USD 0.0042"); // a small call is not shown as nothing
	assert.equal(money(10, { code: "JPY", rate: 152.3, date: "2026-11-26" }, "en-US"), "JPY 1,523"); // no cents in yen
});
