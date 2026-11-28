/** Numbers are shown with what they are numbers of. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	count,
	currencyOf,
	money,
	MOST_STARS,
	normalised,
	onScale,
	percent,
	scaleName,
	similarity,
	spreadPoints,
	starsOf,
	tokens,
} from "@/lib/units";

test("a count names what is counted, in the singular for one", () => {
	assert.equal(count(1, "paper"), "1 paper");
	assert.equal(count(12487, "paper"), "12,487 papers");
	assert.equal(count(24, "community", "communities"), "24 communities");
	assert.equal(count(0, "review"), "0 reviews");
});

test("a reviewers' figure names its scale, and a figure across scales says it is normalised", () => {
	const iclr = { minimum: 1, maximum: 10 };
	assert.equal(onScale(5.5, iclr, "mean rating"), "5.5 of 10 mean rating");
	assert.equal(onScale(4, { minimum: 1, maximum: 6 }, "mean rating"), "4 of 6 mean rating");
	assert.equal(onScale(4, { minimum: 0, maximum: 10 }, "mean rating"), "4 of 0 to 10 mean rating");
	assert.equal(scaleName(iclr), "the 1 to 10 scale");
	assert.equal(spreadPoints(1.234), "1.2 points spread");
	assert.equal(normalised(0.556), "56% of the scale, normalised");
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

test("a score is drawn as stars out of its scale's own points", () => {
	const five = { minimum: 1, maximum: 5 };
	assert.deepEqual(starsOf(4, five), { total: 5, filled: 4 });
	assert.deepEqual(starsOf(2.8, { minimum: 1, maximum: 4 }), { total: 4, filled: 2.8 });
	assert.deepEqual(starsOf(5.5, { minimum: 1, maximum: 10 }), { total: 10, filled: 5.5 });
	// a scale from 0 or below counts from its lowest point
	assert.deepEqual(starsOf(4, { minimum: 0, maximum: 10 }), { total: 10, filled: 4 });
	assert.deepEqual(starsOf(-1, { minimum: -2, maximum: 2 }), { total: 4, filled: 1 });
	// a score outside its scale (a venue that changed its form) fills no fewer than none and no more than all
	assert.deepEqual(starsOf(7, five), { total: 5, filled: 5 });
	assert.deepEqual(starsOf(-3, five), { total: 5, filled: 0 });
});

test("a scale too long for stars, or with no points, has none: its score is said in words only", () => {
	assert.equal(starsOf(40, { minimum: 1, maximum: 100 }), null);
	assert.equal(starsOf(1, { minimum: 1, maximum: 0 }), null);
	assert.equal(starsOf(1, { minimum: 0.5, maximum: 4 }), null);
	assert.equal(MOST_STARS, 10);
});
