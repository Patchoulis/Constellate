/** The pure parts of update-prices.mjs: no network, no file written.   node --test scripts/update-prices.test.mjs */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
	PRICES,
	changesOf,
	currenciesOf,
	isModelKey,
	nextPatch,
	problemsOf,
	ratesFrom,
	serialize,
	withCargoVersion,
	withChangelogVersion,
	withLockedCrateVersion,
	withModel,
} from "./update-prices.mjs";

const bundled = () => JSON.parse(readFileSync(PRICES, "utf8"));
const stored = { base: "USD", date: "2026-11-20", source: "test", per_dollar: { CAD: 1.4, EUR: 0.9 } };
const answer = (rates, date = "2026-11-26") => ({ amount: 1.0, base: "USD", date, rates });
const asked = { codes: ["CAD", "EUR"], stored, today: "2026-11-26" };

test("the bundled prices file is valid and laid out as the script writes it", () => {
	assert.deepEqual(problemsOf(bundled()), []);
	assert.equal(serialize(bundled()), readFileSync(PRICES, "utf8").replace(/\r\n/g, "\n"));
});

test("a prices file with a wrong value says what is wrong", () => {
	const file = bundled();
	file.models["claude-opus-5-5"].input = -4;
	file.models["Claude Opus"] = { input: 1, output: 1 };
	file.rates.per_dollar.EUR = 0;
	file.rates.per_dollar.usd = 1;
	file.rates.date = "2026-02-30";
	const problems = problemsOf(file).join("\n");
	for (const expected of ["claude-opus-5-5: input", '"Claude Opus"', "EUR is not a rate", '"usd"', '"date" is not a day']) {
		assert.ok(problems.includes(expected), `${expected} in:\n${problems}`);
	}
	assert.deepEqual(problemsOf("not an object"), ["the file is not a JSON object"]);
	file.models["claude-haiku-5-5"].long_prompt.above = 0;
	assert.match(problemsOf(file).join("\n"), /claude-haiku-5-5: long_prompt/);
});

test("a misspelt field in a model is a problem, not ignored", () => {
	const file = bundled();
	file.models["claude-opus-5-5"].ouptut = 20;
	assert.match(problemsOf(file).join("\n"), /unknown field "ouptut"/);
});

test("what the engine would refuse is refused here too", () => {
	const problems = (change, text) => {
		const file = bundled();
		change(file);
		return problemsOf(file, text).join("\n");
	};
	assert.match(
		problems((file) => (file.note = "x")),
		/the file: unknown field "note"/,
	);
	assert.match(
		problems((file) => (file.rates.sorce = "x")),
		/rates: unknown field "sorce"/,
	);
	assert.match(
		problems((file) => (file.models["claude-haiku-5-5"].long_prompt.abvoe = 1)),
		/long_prompt: unknown field "abvoe"/,
	);
	assert.match(
		problems((file) => (file.models["claude-opus-5-5"].cache.raed = 1)),
		/claude-opus-5-5: cache: unknown field "raed"/,
	);
	assert.match(
		problems((file) => (file.models["claude-opus-5-5"].cache.read = -1)),
		/claude-opus-5-5: cache is not/,
	);
	assert.match(
		problems((file) => delete file.models["claude-haiku-5-5"].long_prompt.cache.write_1h),
		/claude-haiku-5-5: long_prompt: cache is not/,
	);
	assert.match(
		problems((file) => (file.models["claude-opus-5-5"].input = 0.0000001)),
		/claude-opus-5-5: input is not a price/,
	);
	assert.match(
		problems((file) => (file.models["claude-opus-5-5"].input = 2e6)),
		/claude-opus-5-5: input is not a price/,
	);
	assert.match(
		problems((file) => (file.rates.date = "2099-01-01")),
		/after the day the file was updated/,
	);
	assert.match(
		problems(() => {}, '{"long_prompt": {"above": 1e5}}'),
		/"above" is a number of tokens/,
	);
	assert.match(
		problems(() => {}, '{"long_prompt": {"above": 100000.0}}'),
		/"above" is a number of tokens/,
	);
	assert.equal(
		problems(() => {}, readFileSync(PRICES, "utf8")),
		"",
	);
});

test("the currencies are those the interface knows, with the euro and without the dollar", () => {
	const units = `const CURRENCY_OF: Record<string, string> = {\n\tUS: "USD",\n\tCA: "CAD",\n\tJP: "JPY",\n};\nconst EURO = ["DE"];`;
	assert.deepEqual(currenciesOf(units), ["CAD", "EUR", "JPY"]);
	assert.throws(() => currenciesOf("export const nothing = 1;"), /no CURRENCY_OF/);
	assert.ok(currenciesOf(readFileSync(new URL("../apps/web/lib/units.ts", import.meta.url), "utf8")).length > 10);
});

test("a good answer gives its rates and its day", () => {
	assert.deepEqual(ratesFrom(answer({ EUR: 0.91, CAD: 1.42 }), asked), { date: "2026-11-26", rates: { CAD: 1.42, EUR: 0.91 } });
});

test("an answer missing a currency, or with one not asked for, is refused", () => {
	assert.throws(() => ratesFrom(answer({ CAD: 1.42 }), asked), /no rate for EUR/);
	assert.throws(() => ratesFrom(answer({ CAD: 1.42, EUR: 0.91, GBP: 0.8 }), asked), /not asked for: GBP/);
});

test("a rate that is not a finite number above zero is refused", () => {
	for (const rate of [0, -1, "1.4", null, Infinity, Number.NaN]) {
		assert.throws(() => ratesFrom(answer({ CAD: rate, EUR: 0.91 }), asked), /CAD is not a number above 0/, String(rate));
	}
});

test("an answer from an implausible day is refused", () => {
	const rates = { CAD: 1.42, EUR: 0.91 };
	assert.throws(() => ratesFrom(answer(rates, "2026-11-31"), asked), /not a day/);
	assert.throws(() => ratesFrom(answer(rates, "2026-12-01"), asked), /in the future/);
	assert.throws(() => ratesFrom(answer(rates, "2026-10-01"), asked), /days old/);
	assert.throws(() => ratesFrom(answer(rates, "2026-11-19"), asked), /older than the stored rates/);
	assert.doesNotThrow(() => ratesFrom(answer(rates, "2026-11-27"), asked)); // the bank's day is a time zone ahead
});

test("an answer in another base or for another amount is refused", () => {
	assert.throws(() => ratesFrom({ ...answer({ CAD: 1.42, EUR: 0.91 }), base: "EUR" }, asked), /not in US dollars/);
	assert.throws(() => ratesFrom({ ...answer({ CAD: 1.42, EUR: 0.91 }), amount: 100 }, asked), /100 dollars/);
	assert.throws(() => ratesFrom("<html>", asked), /not a JSON object/);
});

test("a rate that moved by more than a quarter is refused unless forced", () => {
	const moved = answer({ CAD: 1.4 * 1.3, EUR: 0.91 });
	assert.throws(() => ratesFrom(moved, asked), /CAD moved from 1.4 to .*--force/);
	assert.equal(ratesFrom(moved, { ...asked, force: true }).rates.CAD, 1.4 * 1.3);
	assert.doesNotThrow(() => ratesFrom(answer({ CAD: 1.4 * 1.2, EUR: 0.91 }), asked));
});

test("only a move past the threshold, a new currency or a dropped one is a change", () => {
	assert.deepEqual(changesOf({ CAD: 1.4, EUR: 0.9 }, { CAD: 1.41, EUR: 0.9 }, 0.01), []);
	assert.deepEqual(changesOf({ CAD: 1.4, EUR: 0.9 }, { CAD: 1.42, EUR: 0.9 }, 0.01), ["CAD: 1.4 -> 1.42 (+1.4%)"]);
	assert.deepEqual(changesOf({ CAD: 1.4 }, { CAD: 1.4, EUR: 0.9 }, 0.01), ["EUR: 0.9 (new)"]);
	assert.deepEqual(changesOf({ CAD: 1.4, EUR: 0.9 }, { CAD: 1.4 }, 0.01), ["EUR: no longer shown"]);
	assert.deepEqual(changesOf({ CAD: 1.4 }, { CAD: 1.3 }, 0.1), []);
});

test("the patch version is raised", () => {
	assert.equal(nextPatch("0.1.0"), "0.1.1");
	assert.equal(nextPatch("1.2.9"), "1.2.10");
	assert.throws(() => nextPatch("1.2"), /not a version/);
	assert.throws(() => nextPatch("1.2.3-beta"), /not a version/);
});

test("the workspace's version is set in Cargo.toml and in Cargo.lock, and nothing else", () => {
	const toml = `[workspace]\nmembers = ["apps/engine"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\nversion = "9"\n`;
	assert.equal(withCargoVersion(toml, "0.1.1"), toml.replace('version = "0.1.0"', 'version = "0.1.1"'));
	assert.throws(() => withCargoVersion("[package]\nversion = '1'\n", "0.1.1"), /no version/);
	const lock = `[[package]]\nname = "chrono"\nversion = "0.4.0"\n\n[[package]]\nname = "constellate"\nversion = "0.1.0"\n`;
	assert.equal(withLockedCrateVersion(lock, "0.1.1"), lock.replace('version = "0.1.0"', 'version = "0.1.1"'));
	assert.throws(() => withLockedCrateVersion("", "0.1.1"), /no package constellate/);
});

test("the new version goes between Unreleased and the last release, and Unreleased stays", () => {
	const changelog = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Something.\n\n## [0.1.0] - 2026-11-26\n\nThe first release.\n";
	const written = withChangelogVersion(changelog, "0.1.1", "2026-12-03", ["Rates of 2026-12-02."]);
	assert.equal(
		written,
		"# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Something.\n\n## [0.1.1] - 2026-12-03\n\n### Changed\n\n- Rates of 2026-12-02.\n\n## [0.1.0] - 2026-11-26\n\nThe first release.\n",
	);
	assert.throws(() => withChangelogVersion(written, "0.1.1", "2026-12-03", ["Again."]), /already has a section/);
	const first = withChangelogVersion("# Changelog\n\n## [Unreleased]\n", "0.0.1", "2026-12-03", ["One."]);
	assert.equal(first, "# Changelog\n\n## [Unreleased]\n\n## [0.0.1] - 2026-12-03\n\n### Changed\n\n- One.\n");
});

test("the real changelog takes a new version", () => {
	const changelog = readFileSync(new URL("../CHANGELOG.md", import.meta.url), "utf8");
	const written = withChangelogVersion(changelog, "9.9.9", "2026-12-03", ["Rates."]);
	assert.ok(written.indexOf("## [Unreleased]") < written.indexOf("## [9.9.9]"));
	assert.ok(written.indexOf("## [9.9.9]") < written.indexOf("## [0.1.0]"));
	assert.equal(written.replace(/## \[9\.9\.9\][^]*?(?=## \[0\.1\.0\])/, ""), changelog);
});

test("a model's prices are set by hand, checked, and its cache and long-prompt rates are kept", () => {
	const file = bundled();
	const changed = withModel(file, "claude-haiku-5-5", "0.2", "1");
	const { cache, long_prompt } = file.models["claude-haiku-5-5"];
	assert.deepEqual(changed.models["claude-haiku-5-5"], { provider: "anthropic", input: 0.2, output: 1, cache, long_prompt });
	assert.deepEqual(withModel(file, "claude-new-6", "3", "15").models["claude-new-6"], { provider: "anthropic", input: 3, output: 15 });
	assert.equal(file.models["claude-haiku-5-5"].input, 0.1); // the file read is not changed
	assert.throws(() => withModel(file, "Claude New", "3", "15"), /not a model id/);
	assert.throws(() => withModel(file, "claude-new-6", "-3", "15"), /dollars per million tokens/);
	assert.throws(() => withModel(file, "claude-new-6", "three", "15"), /dollars per million tokens/);
	assert.throws(() => withModel(file, "claude-new-6", "", "15"), /dollars per million tokens/);
	assert.throws(() => withModel(file, "claude-new-6", "0x10", "15"), /dollars per million tokens/);
	assert.throws(() => withModel(file, "claude-new-6", "0.0000001", "15"), /dollars per million tokens/);
	assert.throws(() => withModel(file, "claude-new-6", "3", undefined), /dollars per million tokens/);
	assert.deepEqual(problemsOf(withModel(file, "claude-new-6", "3", "15")), []);
});

test("every model names its provider, which has a source, and is kept as provider:id unless it is Claude", () => {
	assert.ok(isModelKey("claude-opus-5-5", "anthropic"));
	assert.ok(isModelKey("openai:gpt-6.2-sol", "openai"));
	assert.ok(isModelKey("together:deepseek-ai/DeepSeek-V4.1-Flash", "together"));
	for (const [id, provider] of [
		["gpt-6.2-sol", "openai"],
		["openai:gpt-6.2-sol", "anthropic"],
		["groq:openai/gpt-oss-120b", "openai"],
		["openai:../x", "openai"],
		["openai:", "openai"],
	]) {
		assert.ok(!isModelKey(id, provider), `${id} of ${provider}`);
	}
	const problems = (change) => {
		const file = bundled();
		change(file);
		return problemsOf(file).join("\n");
	};
	assert.match(
		problems((file) => delete file.models["openai:gpt-6.2-sol"].provider),
		/"provider" names no provider with a source/,
	);
	assert.match(
		problems((file) => (file.models["gpt-6.2-sol"] = { ...file.models["openai:gpt-6.2-sol"] })),
		/"gpt-6.2-sol": not how a openai model is kept/,
	);
	assert.match(
		problems((file) => (file.sources.openai.url = "http://example.org/pricing")),
		/source "openai": "url" is not an https address/,
	);
	assert.match(
		problems((file) => (file.sources.openai.fetched = "yesterday")),
		/source "openai": "fetched" is not a day/,
	);
	assert.match(
		problems((file) => delete file.models["claude-opus-5-5"].cache.write_1h),
		/claude-opus-5-5: cache is not/,
	);
	assert.equal(
		problems((file) => (file.models["gemini:gemini-4-carbon"].cache = { read: 0.2 })),
		"",
		"a cache with no write price is one whose writes are billed as input",
	);
	const file = bundled();
	assert.deepEqual(withModel(file, "openai:gpt-7", "3", "15").models["openai:gpt-7"], { provider: "openai", input: 3, output: 15 });
	assert.throws(() => withModel(file, "nobody:model-1", "3", "15"), /no source/);
	assert.throws(() => withModel(file, "openai:a b", "3", "15"), /not a model id/);
});
