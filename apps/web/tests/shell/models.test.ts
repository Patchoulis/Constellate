/** The choice of language model: grouping, availability, efforts and prices in words. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ModelCatalog, ModelProvider } from "@rp/contracts";
import {
	capitalised,
	EFFORT_LABEL,
	EFFORTS,
	READING_TITLE,
	ROLE_FIELDS,
	SAME_AS_NAMING,
	changeFor,
	effortChange,
	modelsApart,
	pickedOf,
	readingPicks,
	readingDiffers,
	readingEntries,
	readingStateOf,
	readingStateWords,
	DEFAULT_READING,
	choosableOf,
	chosenLabel,
	chosenOf,
	effortSent,
	effortsOf,
	groupsOf,
	isPlainId,
	priceLabel,
	priceOf,
	endpointIds,
	unavailableBecause,
} from "@/lib/shell/models";

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
		reading_default: null,
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
				{
					id: "claude-haiku-5-5",
					label: "Claude Haiku 5.5",
					note: "cheapest, least thorough",
					efforts: ["low", "medium", "high"],
					price: { kind: "known", input: 1, output: 5, cache_read: 0.1 },
				},
			],
			reading_default: { model: "claude-haiku-5-5", effort: "medium" },
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
			["anthropic", "Set ANTHROPIC_API_KEY"],
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

test("until the engine has told its models, the settings' model alone is shown, and nothing is offered", () => {
	const models = [{ name: "claude-opus-5-5", label: "Claude Opus 5.5", note: "the default" }];
	const waiting = choosableOf(undefined, models, undefined);
	assert.equal(waiting.source, "waiting");
	assert.deepEqual(
		waiting.catalog.providers.map((one) => one.id),
		["anthropic"],
	);
	assert.equal(waiting.catalog.chosen.model, "claude-opus-5-5");
	assert.deepEqual(effortsOf(chosenOf(waiting.catalog, undefined)), []);
	const named = choosableOf(undefined, models, { describe_provider: "openai", describe_model: "gpt-6.2-sol" });
	assert.equal(named.catalog.chosen.provider, "openai");
	assert.equal(named.catalog.chosen.model, "gpt-6.2-sol");
	// asking failed: said, as why the model cannot be used now
	const failed = choosableOf(undefined, models, undefined, "Constellate answered with error 500");
	assert.equal(failed.source === "waiting" && failed.why, "Constellate answered with error 500");
	assert.deepEqual(failed.catalog.providers[0].available, { state: "unavailable", why: "Constellate answered with error 500" });
	assert.equal(choosableOf(catalog, [], undefined).source, "engine");
});

test("a server's list of models keeps only the ids that can be used, and says how many it left out", () => {
	assert.deepEqual(endpointIds({ models: ["qwen3:32b", "llama4:8b", "../../etc", 42, "qwen3:32b"] }), {
		ids: ["qwen3:32b", "llama4:8b"],
		dropped: 3,
	});
	assert.deepEqual(endpointIds({ models: [] }), { ids: [], dropped: 0 });
	assert.equal(endpointIds({ data: [{ id: "x" }] }), null, "an answer that is not a list of models");
	assert.equal(endpointIds("<html>"), null);
	assert.equal(endpointIds(null), null);
});

const claude = { describe_provider: "anthropic" as const, describe_model: "claude-opus-5-5", describe_effort: "high" as const };
const openai = { describe_provider: "openai" as const, describe_model: "gpt-6.2-sol", describe_effort: "high" as const };

test("on the Claude API, papers are read by its Haiku by default; on another provider by the naming model", () => {
	assert.equal(readingStateOf(claude), "default", "an older engine's or file's settings have no reading keys");
	assert.deepEqual(pickedOf(claude, "reading", catalog), { provider: "anthropic", model: "claude-haiku-5-5", effort: "medium" });
	assert.equal(chosenLabel(chosenOf(catalog, claude, "reading")), "Claude Haiku 5.5 · Anthropic");
	assert.equal(chosenLabel(chosenOf(catalog, claude, "naming")), "Claude Opus 5.5 · Anthropic", "the naming model as it was");
	assert.equal(readingDiffers(catalog, claude), true);
	assert.equal(modelsApart(catalog, claude), true, "the default differs: both lines are shown");
	assert.equal(readingStateWords(catalog, claude), "Default: Claude Haiku 5.5 · Anthropic.");
	// another provider: the default is the naming model, never a provider not set up
	assert.equal(readingStateOf(openai), "default");
	assert.deepEqual(pickedOf(openai, "reading", catalog), { provider: "openai", model: "gpt-6.2-sol", effort: "high" });
	assert.equal(modelsApart(catalog, openai), false);
	assert.equal(readingStateWords(catalog, openai), "The same as the naming model.");
	// no catalogue yet: the default is not known, rather than guessed
	assert.deepEqual(pickedOf(claude, "reading"), { provider: undefined, model: undefined, effort: undefined });
	assert.equal(chosenOf(catalog, undefined, "reading").id, "claude-haiku-5-5", "the catalogue's naming provider's default");
});

test("the naming model can be chosen to read explicitly, and a model of its own is its own", () => {
	const same = { ...claude, reading_same_as_naming: true };
	assert.equal(readingStateOf(same), "same_as_naming");
	assert.equal(chosenLabel(chosenOf(catalog, same, "reading")), "Claude Opus 5.5 · Anthropic");
	assert.equal(modelsApart(catalog, same), false, "it follows the naming model: one line");
	assert.equal(readingStateWords(catalog, same), "The same as the naming model.");
	const own = { ...claude, reading_provider: "compatible" as const, reading_model: "qwen3:32b", reading_effort: "low" as const };
	assert.equal(readingStateOf(own), "chosen");
	assert.deepEqual(pickedOf(own, "reading", catalog), { provider: "compatible", model: "qwen3:32b", effort: "low" });
	assert.equal(readingStateWords(catalog, own), null);
	// chosen on its own as the very same model: shown apart, since changing the naming one would not change it
	const alike = { ...openai, reading_provider: "openai" as const, reading_model: "gpt-6.2-sol", reading_effort: "high" as const };
	assert.equal(readingDiffers(catalog, alike), false);
	assert.equal(modelsApart(catalog, alike), true);
});

test("the menu's first entry says what the default is, and the naming model stays a choice on the Claude API", () => {
	assert.deepEqual(readingEntries(catalog, claude), [
		{ label: "Default: Claude Haiku 5.5 · Anthropic", note: "", ticked: true, change: DEFAULT_READING },
		{ label: "The same as the naming model", note: "Claude Opus 5.5 · Anthropic", ticked: false, change: SAME_AS_NAMING },
	]);
	assert.deepEqual(
		readingEntries(catalog, { ...claude, reading_same_as_naming: true }).map((entry) => entry.ticked),
		[false, true],
	);
	assert.deepEqual(
		readingEntries(catalog, { ...claude, reading_provider: "openai", reading_model: "gpt-6.2-sol" }).map((entry) => entry.ticked),
		[false, false],
	);
	// elsewhere the default is the naming model: one entry that says so
	assert.deepEqual(readingEntries(catalog, openai), [
		{ label: "The same as the naming model", note: "GPT-6.2 Sol · OpenAI", ticked: true, change: DEFAULT_READING },
	]);
});

test("a server's window cannot choose the model that reads papers, nor move it to another provider", () => {
	assert.deepEqual(readingPicks(catalog, openai, false), { locked: false, canReset: true });
	assert.deepEqual(readingPicks(catalog, openai, true), { locked: true, canReset: true }, "the owner chooses it");
	assert.deepEqual(readingPicks(catalog, claude, true), { locked: true, canReset: true }, "the default: one provider, the server's");
	assert.deepEqual(readingPicks(catalog, { ...claude, reading_same_as_naming: true }, true), { locked: true, canReset: true });
	const elsewhere = { ...openai, reading_provider: "compatible" as const, reading_model: "qwen3:32b" };
	assert.deepEqual(readingPicks(catalog, elsewhere, true), { locked: false, canReset: false });
	const within = { ...openai, reading_provider: "openai" as const, reading_model: "gpt-6.2-sol" };
	assert.deepEqual(readingPicks(catalog, within, true), { locked: false, canReset: true });
});

test("a pick of the model that reads papers saves its own settings, and the entries save no model", () => {
	assert.deepEqual(changeFor("openai", "gpt-6.2-sol", false, "reading"), { reading_provider: "openai", reading_model: "gpt-6.2-sol" });
	assert.deepEqual(changeFor("openai", "gpt-6.2-sol", true, "reading"), { reading_model: "gpt-6.2-sol" }, "the server's provider");
	assert.deepEqual(effortChange("low", "reading"), { reading_effort: "low" });
	assert.deepEqual(effortChange("low"), { describe_effort: "low" });
	assert.deepEqual(DEFAULT_READING, { reading_provider: null, reading_same_as_naming: false });
	assert.deepEqual(SAME_AS_NAMING, { reading_same_as_naming: true });
	assert.deepEqual(ROLE_FIELDS.reading, { provider: "reading_provider", model: "reading_model", effort: "reading_effort" });
	assert.match(READING_TITLE, /tables, agree \/ disagree, questions, claims, weaknesses, explanations and citation kinds/);
});

test("a provider that cannot be asked says why, and one that can says nothing", () => {
	assert.equal(unavailableBecause(provider("anthropic", "Anthropic", true)), null);
	assert.equal(unavailableBecause(provider("openai", "OpenAI", false)), "Set OPENAI_API_KEY");
});

test("labels that stand alone begin with a capital: the efforts, and why a provider cannot be used", () => {
	assert.deepEqual(
		EFFORTS.map((effort) => EFFORT_LABEL[effort]),
		["Low effort", "Medium effort", "High effort", "Extra-high effort", "Most effort"],
	);
	assert.equal(capitalised("set ANTHROPIC_API_KEY"), "Set ANTHROPIC_API_KEY");
	assert.equal(capitalised("Already so"), "Already so");
	assert.equal(capitalised(""), "");
});
