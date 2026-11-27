/** Estimates as the engine sends them, for the tests: an `AskingCost`, or a batch's (`BatchCost`). */
import type { AskingCost, BatchCost } from "@rp/contracts";

/** What asking would cost, priced at `usd` likely and `most` at most (null for a model with no price). */
export function askingCost(usd: number | null, most: number | null, overrides: Partial<AskingCost> = {}): AskingCost {
	return {
		provider: "anthropic",
		model: "some-model",
		calls: 1,
		tokens: { input: 900, output: 3_500, cache_read: 0, cache_write_5m: 20_000, cache_write_1h: 0 },
		usd,
		most_usd: most,
		most_tokens: 28_000,
		counting: { kind: "assumed", chars_per_token: 3 },
		caching: "marked",
		on_this_computer: false,
		...overrides,
	};
}

/** A batch of `toAsk` items not kept yet (of `items`), its cost as `askingCost` says. */
export function batchCost(usd: number | null, most: number | null, toAsk = 1, overrides: Partial<BatchCost> = {}): BatchCost {
	return { ...askingCost(usd, most), items: Math.max(toAsk, 1), kept: 0, to_ask: toAsk, ...overrides };
}
