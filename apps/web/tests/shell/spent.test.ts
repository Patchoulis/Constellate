/** What the sidebar says was spent, in all and on the open run (lib/shell/spent.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Usage, UsageEntry } from "@rp/contracts";
import { runDetail, spentLine, spentParts } from "@/lib/shell/spent";
import { money } from "@/lib/units";

const DOLLAR = { code: "USD", rate: 1, date: null };
const dollars = (usd: number) => money(usd, DOLLAR, "en-US");
const canadian = (usd: number) => money(usd, { code: "CAD", rate: 1.42, date: "2026-11-20" }, "en-CA");

function entry(calls: number, usd: number | null, unpriced = 0): UsageEntry {
	return { calls, input_tokens: 1_000 * calls, output_tokens: 500 * calls, usd, unpriced_calls: unpriced };
}

/** An answer of `GET /api/usage`, with what the test says changed. */
function usage(changes: Partial<Usage> = {}): Usage {
	return {
		provider: { name: "Anthropic", ready: true },
		model: { name: "claude-opus-5-5", label: "Claude Opus 5.5" },
		since: "2026-11-01",
		currency: DOLLAR,
		usd: 3.77,
		calls: 12,
		unpriced_calls: 0,
		run_usd: 1.2,
		run_calls: 4,
		run_unpriced_calls: 0,
		run_purposes: { "naming communities": entry(4, 1.2) },
		run_counted_since: null,
		models: { "claude-opus-5-5": entry(12, 3.77) },
		purposes: { "naming communities": entry(12, 3.77) },
		...changes,
	};
}

test("the total spent, and beside it what the open run cost", () => {
	assert.equal(spentLine(usage(), "r1", dollars), "USD 3.77 spent (USD 1.20 this run)");
	assert.deepEqual(spentParts(usage(), "r1", dollars), { total: "USD 3.77 spent", ofRun: "(USD 1.20 this run)" });
	// the owner's own example: nothing spent at all is a known nothing, said as such
	const nothing = usage({ usd: 0, calls: 0, run_usd: 0, run_calls: 0, run_purposes: {}, models: {}, purposes: {} });
	assert.equal(spentLine(nothing, "r1", dollars), "USD 0.00 spent (USD 0.00 this run)");
});

test("with no run open, the total alone; nor is a run's figure made up when the engine was not asked about one", () => {
	assert.equal(spentLine(usage(), null, dollars), "USD 3.77 spent");
	const notAsked = usage({ run_usd: null, run_calls: 0, run_purposes: {} });
	assert.equal(spentLine(notAsked, "r1", dollars), "USD 3.77 spent");
	assert.deepEqual(runDetail(notAsked, "r1", "My run", dollars), []);
});

test("calls with no known price are said in calls and tokens, never as an amount", () => {
	const unpriced = usage({
		usd: null,
		calls: 2,
		unpriced_calls: 2,
		models: { mystery: entry(2, null, 2) },
		run_usd: null,
		run_calls: 1,
		run_unpriced_calls: 1,
		run_purposes: { "community overviews": entry(1, null, 1) },
	});
	assert.equal(spentLine(unpriced, "r1", dollars), "2 calls, 3k tokens (1 call, 1.5k tokens this run)");
	assert.doesNotMatch(spentLine(unpriced, "r1", dollars), /USD/);
	const detail = runDetail(unpriced, "r1", "My run", dollars);
	assert.equal(detail[0], "This run, My run: 1 call, 1.5k tokens.");
	assert.equal(detail[2], "  community overviews: 1 call, 1k tokens sent, 500 tokens received");
});

test("every amount carries its currency's code, in the user's currency", () => {
	assert.equal(spentLine(usage(), "r1", canadian), "CAD 5.35 spent (CAD 1.70 this run)");
});

test("the tooltip adds the run's breakdown, what is not in its figure, and from which day it counts", () => {
	const short = usage({ run_unpriced_calls: 1, run_counted_since: "2026-11-28" });
	const detail = runDetail(short, "r1", "My run", dollars);
	assert.equal(detail[0], "This run, My run: USD 1.20, 4 calls.");
	assert.deepEqual(detail.slice(1, 3), [
		"This run by purpose:",
		"  naming communities: USD 1.20 (4 calls, 4k tokens sent, 2k tokens received)",
	]);
	assert.ok(detail.includes("1 call of this run to a model with no price is not in its figure."), detail.join("\n"));
	assert.ok(
		detail.some((line) => line.startsWith("This run's figure is counted since 2026-11-28")),
		detail.join("\n"),
	);
	assert.ok(detail.some((line) => line.startsWith("Deleting a run does not take what it cost off the total")));
	assert.ok(!runDetail(usage(), "r1", "My run", dollars).some((line) => line.includes("counted since")));
});
