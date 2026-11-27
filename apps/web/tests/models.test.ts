/** The choice of language model: grouping, availability, efforts and prices in words. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ModelCatalog, ModelProvider } from "@rp/contracts";
import {
	changeFor,
	choosableOf,
	chosenLabel,
	chosenOf,
	effortSent,
	effortsOf,
	engineCatalog,
	groupsOf,
	isPlainId,
	olderBackend,
	priceLabel,
	priceOf,
} from "@/lib/models";
import { isMissingRoute } from "@/lib/missing-route";

const USD = { code: "USD", rate: 1, date: null };

function provider(id: ModelProvider["id"], label: string, ready: boolean, extra: Partial<ModelProvider> = {}): ModelProvider {
	return {
		id,
		label,
		key: { variable: `${id.toUpperCase()}_API_KEY`, needed: true, set: ready },
		address: { variable: null, host: `api.${id}.example`, place: "provider" },
		available: ready ? { state: "ready" } : { state: "unavailable", why: `set ${id.toUpperCase()}_API_KEY` },
		ids: "listed_or_typed",
		capabilities: { answers: "enforced", effort: "by_model", caching: "automatic", counting: "endpoint" },
		limits: [],
		models: [],
		price_of_typed: { kind: "not_known" },
		docs: "",
		...extra,
	};
}

const catalog: ModelCatalog = {
	providers: [
		provider("anthropic", "Anthropic", false, {
			ids: "listed",
			models: [
				{
					id: "claude-opus-5-5",
					label: "Claude Opus 5.5",
					note: "the default",
					efforts: ["low", "medium", "high", "xhigh", "max"],
					price: { kind: "known", input: 4, output: 20, cache_read: 0.2 },
				},
			],
		}),
		provider("openai", "OpenAI", true, {
			models: [
				{
					id: "gpt-6.2-sol",
					label: "GPT-6.2 Sol",
					note: "",
					efforts: ["low", "medium", "high"],
					price: { kind: "known", input: 2, output: 10, cache_read: 0.1 },
				},
			],
		}),
		provider("compatible", "OpenAI-compatible endpoint", true, {
			ids: "typed",
			capabilities: { answers: "requested", effort: "not_sent", caching: "not_reported", counting: "characters" },
			price_of_typed: { kind: "this_computer" },
			limits: ["Cannot count tokens in advance: they are estimated from the number of characters."],
		}),
	],
	chosen: { provider: "anthropic", model: "claude-opus-5-5", effort: "medium" },
};

test("providers that can be used come first, those that cannot after, saying what to set", () => {
	const groups = groupsOf(catalog);
	assert.deepEqual(
		groups.map((group) => [group.provider.id, group.why]),
		[
			["openai", null],
			["compatible", null],
			["anthropic", "set ANTHROPIC_API_KEY"],
		],
	);
	assert.deepEqual(
		groups.map((group) => group.typed),
		[true, true, false],
		"an id may be typed except for the Claude API",
	);
});

test("the chosen model is named with its provider, and a typed id by itself", () => {
	assert.equal(chosenLabel(chosenOf(catalog, undefined)), "Claude Opus 5.5 · Anthropic");
	const typed = chosenOf(catalog, { describe_provider: "compatible", describe_model: "qwen3:32b", describe_effort: "high" });
	assert.equal(chosenLabel(typed), "qwen3:32b · OpenAI-compatible endpoint");
	assert.equal(typed.model, undefined);
});

test("a price is said with its currency, and one not known is never 0", () => {
	assert.equal(
		priceLabel({ kind: "known", input: 2, output: 10, cache_read: 0.1 }, USD, "en-US"),
		"USD 2.00 in, USD 10.00 out per million tokens",
	);
	assert.equal(priceLabel({ kind: "not_known" }, USD), null, "no price: nothing is shown");
	assert.equal(priceLabel({ kind: "this_computer" }, USD), "runs on this computer: no API cost");
	const compatible = catalog.providers[2];
	assert.deepEqual(priceOf(compatible, "qwen3:32b"), { kind: "this_computer" });
	assert.deepEqual(priceOf(catalog.providers[1], "gpt-7"), { kind: "not_known" }, "a typed id has no known price");
	assert.deepEqual(priceOf(undefined, "x"), { kind: "not_known" });
});

test("efforts are those of the model, none for a typed id or a provider that sets none", () => {
	assert.deepEqual(effortsOf(chosenOf(catalog, { describe_provider: "openai", describe_model: "gpt-6.2-sol" })), ["low", "medium", "high"]);
	assert.deepEqual(effortsOf(chosenOf(catalog, { describe_provider: "openai", describe_model: "gpt-7" })), []);
	assert.deepEqual(effortsOf(chosenOf(catalog, { describe_provider: "compatible", describe_model: "qwen3:32b" })), []);
	assert.equal(effortSent("max", ["low", "medium", "high"]), "high");
	assert.equal(effortSent("xhigh", ["low", "high", "max"]), "high", "of two as near, the lower, as the engine");
	assert.equal(effortSent("medium", []), null);
});

test("a typed id must be plain, as the engine takes it", () => {
	for (const good of ["qwen3:32b", "accounts/fireworks/models/glm-5p3", "gpt-6.2-sol"]) assert.ok(isPlainId(good), good);
	for (const bad of ["", "../x", "/v1", "a b", "x?key=1"]) assert.ok(!isPlainId(bad), bad);
});

test("a pick saves the provider with the model, except where the provider is the server's", () => {
	assert.deepEqual(changeFor("openai", "gpt-6.2-sol", false), { describe_provider: "openai", describe_model: "gpt-6.2-sol" });
	assert.deepEqual(changeFor("openai", "gpt-6.2-sol", true), { describe_model: "gpt-6.2-sol" });
});

test("a backend without the models route falls back to today's list of Claude models", async () => {
	const missing = await engineCatalog(() => Promise.reject(new Error("The backend answered 404")));
	assert.equal(missing, null);
	const older = choosableOf(missing, [{ name: "claude-opus-5-5", label: "Claude Opus 5.5", note: "the default" }], undefined);
	assert.equal(older.source, "older_backend");
	assert.deepEqual(
		older.catalog.providers.map((one) => one.id),
		["anthropic"],
		"no provider the running backend cannot use",
	);
	assert.equal(older.catalog.chosen.model, "claude-opus-5-5");
	assert.deepEqual(effortsOf(chosenOf(older.catalog, undefined)), [], "no effort the older backend does not know");
	assert.equal(choosableOf(await engineCatalog(() => Promise.resolve(catalog)), [], undefined).source, "engine");
	await assert.rejects(engineCatalog(() => Promise.reject(new Error("The backend is not reachable"))));
	assert.ok(isMissingRoute(new Error("not found")) && isMissingRoute("not found") && !isMissingRoute(undefined));
	assert.equal(olderBackend([], "claude-sonnet-5-5").catalog.chosen.model, "claude-sonnet-5-5");
});
