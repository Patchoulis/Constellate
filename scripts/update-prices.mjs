/**
 * Bring the exchange rates the app converts costs with up to date, as part of the build:
 *
 *   node scripts/update-prices.mjs
 *
 * The rates and the model prices live in apps/engine/data/prices.json, which is compiled into the engine: the app
 * never asks the network for them. This asks frankfurter.dev for the European Central Bank's reference rates of
 * the day, for the currencies the interface knows (apps/web/lib/units.ts), and rewrites the file only when one of
 * them moved by more than the threshold since the stored one. When it does, it also raises the patch version
 * (package.json, package-lock.json, Cargo.toml, Cargo.lock) and adds that version to CHANGELOG.md, so that the
 * change is a release by itself. The weekly workflow .github/workflows/prices.yml runs it and opens a pull request.
 *
 * Model prices are not fetched: the provider publishes no machine-readable list of them. They change by hand,
 * with --set-model. A release made here leaves [Unreleased] in CHANGELOG.md as it is: when it has entries, they
 * ship with this version too, and whoever tags it moves them under its heading first.
 *
 *   --check                           write nothing; exit 1 when an update would be made, 0 when not
 *   --threshold=<percent>              how far a rate must move to be worth a release (default 1)
 *   --force                            accept a rate that moved by more than 25% (a real devaluation, say)
 *   --set-model <id> <input> <output>  a model's US dollars per million tokens; released like a rate
 *   --validate <file | ->              only check a prices file (- reads standard input): the pre-commit hook
 *
 * Exit status: 0 done (or nothing to do), 1 an update would be made (--check), 2 refused or failed.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
export const PRICES = join(root, "apps", "engine", "data", "prices.json");
const UNITS = join(root, "apps", "web", "lib", "units.ts");
const SOURCE = "https://api.frankfurter.dev/v1/latest";
const SOURCE_NAME = "European Central Bank via frankfurter.dev";
/** A rate that moved further than this in a week is more likely a wrong answer than a market. */
export const MOST_MOVED = 0.25;
/** The bank publishes on working days; an answer older than this is a source that stopped being updated. */
const OLDEST_DAYS = 14;

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const CODE = /^[A-Z]{3}$/;
const MODEL = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** A day written YYYY-MM-DD that is on the calendar (not 2026-02-30). */
function isDay(text) {
	if (typeof text !== "string" || !DATE.test(text)) return false;
	const day = new Date(`${text}T00:00:00Z`);
	return !Number.isNaN(day.getTime()) && day.toISOString().slice(0, 10) === text;
}

function daysBetween(earlier, later) {
	return (Date.parse(`${later}T00:00:00Z`) - Date.parse(`${earlier}T00:00:00Z`)) / 86_400_000;
}

/**
 * Dollars per million tokens as the engine holds them: 0 to a million, to the micro-dollar (it counts
 * picodollars per token, a whole number only for a price written to the micro-dollar).
 */
const isPrice = (value) =>
	typeof value === "number" &&
	Number.isFinite(value) &&
	value >= 0 &&
	value <= 1e6 &&
	Math.abs(value * 1e6 - Math.round(value * 1e6)) <= 1e-3;
const isRate = (value) => typeof value === "number" && Number.isFinite(value) && value > 0;
const isObject = (value) => typeof value === "object" && value !== null && !Array.isArray(value);

/** The currencies the interface may show, from units.ts: every value of CURRENCY_OF, the euro, not the dollar. */
export function currenciesOf(units) {
	const table = /const CURRENCY_OF[^{]*\{([^}]*)\}/.exec(units);
	if (!table) throw new Error("units.ts has no CURRENCY_OF table: where do the interface's currencies come from now?");
	const codes = new Set([...table[1].matchAll(/"([A-Z]{3})"/g)].map((found) => found[1]));
	if (/const EURO\b/.test(units)) codes.add("EUR");
	codes.delete("USD");
	if (codes.size === 0) throw new Error("units.ts names no currencies");
	return [...codes].sort();
}

/** Fields a part of the file may not have: the engine refuses them (a field nobody reads is usually misspelt). */
function unknownFields(object, known, where) {
	return Object.keys(object)
		.filter((key) => !known.includes(key))
		.map((key) => `${where}: unknown field ${JSON.stringify(key)}`);
}

/**
 * What is wrong with a prices file, one line per problem; none for a good one. At least as strict as the engine
 * (`Bundled::parse` in apps/engine/src/usage/mod.rs), so that a file this accepts is one the engine reads: the
 * engine does not fail on a bad file, it counts every call as unpriced, which nobody would notice. `text`, when
 * given, is the file as written, for what JSON.parse cannot tell apart (100000 from 1e5, which the engine refuses).
 */
export function problemsOf(file, text) {
	if (!isObject(file)) return ["the file is not a JSON object"];
	const problems = unknownFields(file, ["updated", "models", "rates"], "the file");
	if (text !== undefined && /"above"\s*:\s*-?\d*[.eE]/.test(text))
		problems.push(`"above" is a number of tokens, written without a point or exponent`);
	if (!isDay(file.updated)) problems.push(`"updated" is not a day (YYYY-MM-DD): ${JSON.stringify(file.updated)}`);
	if (!isObject(file.models) || Object.keys(file.models).length === 0) problems.push(`"models" lists no model`);
	for (const [id, model] of Object.entries(isObject(file.models) ? file.models : {})) {
		if (!MODEL.test(id)) problems.push(`model ${JSON.stringify(id)}: not a model id (lower case, digits, hyphens)`);
		if (!isObject(model)) {
			problems.push(`model ${id}: not an object`);
			continue;
		}
		for (const side of ["input", "output"]) {
			if (!isPrice(model[side])) {
				problems.push(`model ${id}: ${side} is not a price (dollars per million tokens, 0 to 1000000, to the millionth)`);
			}
		}
		const long = model.long_prompt;
		if (long !== undefined) {
			if (!isObject(long) || !Number.isSafeInteger(long.above) || long.above <= 0 || !isPrice(long.input) || !isPrice(long.output)) {
				problems.push(`model ${id}: long_prompt is not {"above": tokens, "input": price, "output": price}`);
			} else {
				problems.push(...unknownFields(long, ["above", "input", "output"], `model ${id}: long_prompt`));
			}
		}
		problems.push(...unknownFields(model, ["input", "output", "long_prompt"], `model ${id}`));
	}
	const rates = file.rates;
	if (!isObject(rates)) return [...problems, `"rates" is missing`];
	problems.push(...unknownFields(rates, ["base", "date", "source", "per_dollar"], "rates"));
	if (isDay(rates.date) && isDay(file.updated) && rates.date > file.updated) {
		problems.push(`rates: "date" ${rates.date} is after the day the file was updated, ${file.updated}`);
	}
	if (rates.base !== "USD") problems.push(`rates: the base is ${JSON.stringify(rates.base)}, not "USD"`);
	if (!isDay(rates.date)) problems.push(`rates: "date" is not a day (YYYY-MM-DD): ${JSON.stringify(rates.date)}`);
	if (typeof rates.source !== "string" || rates.source.trim() === "") problems.push(`rates: "source" does not say where they are from`);
	if (!isObject(rates.per_dollar)) problems.push(`rates: "per_dollar" is missing`);
	for (const [code, rate] of Object.entries(isObject(rates.per_dollar) ? rates.per_dollar : {})) {
		if (!CODE.test(code) || code === "USD") problems.push(`rates: ${JSON.stringify(code)} is not a currency other than the dollar`);
		if (!isRate(rate)) problems.push(`rates: ${code} is not a rate (a number above 0): ${JSON.stringify(rate)}`);
	}
	return problems;
}

/**
 * The rates in frankfurter's answer, checked as strictly as they will be trusted: every currency asked for and no
 * other, a finite number above 0, a day that is on the calendar, not in the future, not stale, not older than the
 * stored rates, and (without `force`) no rate that moved by more than MOST_MOVED since the stored one.
 * Throws an Error that says what is wrong.
 */
export function ratesFrom(answer, { codes, stored, today, force = false }) {
	if (!isObject(answer)) throw new Error("the answer is not a JSON object");
	if (answer.base !== "USD") throw new Error(`the answer is in ${JSON.stringify(answer.base)}, not in US dollars`);
	if (answer.amount !== undefined && answer.amount !== 1) throw new Error(`the answer is for ${answer.amount} dollars, not 1`);
	if (!isDay(answer.date)) throw new Error(`the answer's date is not a day: ${JSON.stringify(answer.date)}`);
	if (daysBetween(today, answer.date) > 1) throw new Error(`the answer's date ${answer.date} is in the future`);
	if (daysBetween(answer.date, today) > OLDEST_DAYS)
		throw new Error(`the answer's date ${answer.date} is more than ${OLDEST_DAYS} days old`);
	if (isDay(stored?.date) && answer.date < stored.date)
		throw new Error(`the answer (${answer.date}) is older than the stored rates (${stored.date})`);
	if (!isObject(answer.rates)) throw new Error("the answer has no rates");
	const missing = codes.filter((code) => !(code in answer.rates));
	if (missing.length) throw new Error(`the answer has no rate for ${missing.join(", ")}`);
	const extra = Object.keys(answer.rates).filter((code) => !codes.includes(code));
	if (extra.length) throw new Error(`the answer has rates that were not asked for: ${extra.join(", ")}`);
	const rates = {};
	for (const code of [...codes].sort()) {
		const rate = answer.rates[code];
		if (!isRate(rate)) throw new Error(`the rate for ${code} is not a number above 0: ${JSON.stringify(rate)}`);
		const before = stored?.per_dollar?.[code];
		if (!force && isRate(before) && Math.abs(rate / before - 1) > MOST_MOVED) {
			throw new Error(
				`${code} moved from ${before} to ${rate} (${percent(rate / before - 1)}): more than ${percent(MOST_MOVED)} is refused without --force`,
			);
		}
		rates[code] = rate;
	}
	return { date: answer.date, rates };
}

function percent(fraction) {
	return `${fraction > 0 ? "+" : ""}${(fraction * 100).toFixed(1)}%`;
}

/**
 * What makes the fresh rates worth a release, one line each: a rate that moved by more than `threshold` (a
 * fraction: 0.01 is 1%), a currency that is new, one that is no longer shown. Nothing when none of these.
 */
export function changesOf(stored, fresh, threshold) {
	const changes = [];
	for (const [code, rate] of Object.entries(fresh)) {
		const before = stored[code];
		if (!isRate(before)) changes.push(`${code}: ${rate} (new)`);
		else if (Math.abs(rate / before - 1) > threshold) changes.push(`${code}: ${before} -> ${rate} (${percent(rate / before - 1)})`);
	}
	for (const code of Object.keys(stored)) if (!(code in fresh)) changes.push(`${code}: no longer shown`);
	return changes;
}

/** 0.1.9 -> 0.1.10. */
export function nextPatch(version) {
	const parts = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
	if (!parts) throw new Error(`${JSON.stringify(version)} is not a version X.Y.Z`);
	return `${parts[1]}.${parts[2]}.${Number(parts[3]) + 1}`;
}

/** Cargo.toml with `version` of `[workspace.package]` set, and nothing else changed. */
export function withCargoVersion(text, version) {
	const section = /(\[workspace\.package\][^[]*?\nversion = ")([^"]+)(")/;
	if (!section.test(text)) throw new Error("Cargo.toml has no version in [workspace.package]");
	return text.replace(section, `$1${version}$3`);
}

/** Cargo.lock with the engine's own entry at `version` (cargo would otherwise rewrite it at the next build). */
export function withLockedCrateVersion(text, version, crate = "constellate") {
	const entry = new RegExp(`(\\[\\[package\\]\\]\\r?\\nname = "${crate}"\\r?\\nversion = ")([^"]+)(")`);
	if (!entry.test(text)) throw new Error(`Cargo.lock has no package ${crate}`);
	return text.replace(entry, `$1${version}$3`);
}

/** CHANGELOG.md with a new version heading between [Unreleased] and the last release; Unreleased stays as it is. */
export function withChangelogVersion(text, version, date, lines) {
	if (text.includes(`## [${version}]`)) throw new Error(`CHANGELOG.md already has a section for ${version}`);
	const section = `## [${version}] - ${date}\n\n### Changed\n\n${lines.map((line) => `- ${line}`).join("\n\n")}\n\n`;
	const unreleased = text.indexOf("## [Unreleased]");
	const from = unreleased === -1 ? 0 : unreleased + "## [Unreleased]".length;
	const next = text.indexOf("\n## [", from);
	if (next === -1) return `${text.trimEnd()}\n\n${section.trimEnd()}\n`;
	return `${text.slice(0, next + 1)}${section}${text.slice(next + 1)}`;
}

/** The file with one model's prices set; a long-prompt rate it had is kept (and said so by the caller). */
export function withModel(file, id, input, output) {
	if (!MODEL.test(id)) throw new Error(`${JSON.stringify(id)} is not a model id (lower case, digits, hyphens), e.g. claude-opus-5-5`);
	const prices = [input, output].map(Number);
	// written as a decimal: Number() would also take "0x10" or "1e3"
	if (!prices.every(isPrice) || [input, output].some((text) => !/^\d+(\.\d+)?$/.test(String(text)))) {
		throw new Error(`the prices must be US dollars per million tokens, 0 or more: ${input} ${output}`);
	}
	const kept = file.models[id]?.long_prompt;
	return { ...file, models: { ...file.models, [id]: { input: prices[0], output: prices[1], ...(kept ? { long_prompt: kept } : {}) } } };
}

/** The prices file as the formatter would lay it out (tabs; Prettier accepts JSON.stringify's layout). */
export function serialize(file) {
	return `${JSON.stringify(file, null, "\t")}\n`;
}

function readJson(path) {
	return JSON.parse(readFileSync(path, "utf8"));
}

/**
 * Write the new prices file, raise the patch version everywhere it is written, and add it to the changelog.
 * Every file's new content is worked out before the first is written, so a refusal (a changelog that already has
 * this version, say) leaves nothing half done. Returns the new version.
 */
function release(prices, lines) {
	const problems = problemsOf(prices);
	if (problems.length) throw new Error(`the new prices file would not be valid:\n  ${problems.join("\n  ")}`);
	const text = (name) => readFileSync(join(root, name), "utf8");
	const manifest = readJson(join(root, "package.json"));
	const version = nextPatch(manifest.version);
	const lock = readJson(join(root, "package-lock.json"));
	lock.version = version;
	if (lock.packages?.[""]) lock.packages[""].version = version;
	const written = [
		[PRICES, serialize(prices)],
		[join(root, "package.json"), serialize({ ...manifest, version })],
		[join(root, "package-lock.json"), serialize(lock)],
		[join(root, "Cargo.toml"), withCargoVersion(text("Cargo.toml"), version)],
		[join(root, "Cargo.lock"), withLockedCrateVersion(text("Cargo.lock"), version)],
		[join(root, "CHANGELOG.md"), withChangelogVersion(text("CHANGELOG.md"), version, prices.updated, lines)],
	];
	for (const [path, content] of written) writeFileSync(path, content);
	return version;
}

/** The value after `--name=` or `--name`, or undefined. */
function option(args, name) {
	const index = args.findIndex((arg) => arg === name || arg.startsWith(`${name}=`));
	if (index === -1) return undefined;
	return args[index].includes("=") ? args[index].slice(name.length + 1) : args[index + 1];
}

function today() {
	return new Date().toISOString().slice(0, 10);
}

async function main(args) {
	const check = args.includes("--check");
	const validate = option(args, "--validate");
	if (validate !== undefined) {
		const text = validate === "-" ? readFileSync(0, "utf8") : readFileSync(resolve(validate), "utf8");
		let problems;
		try {
			problems = problemsOf(JSON.parse(text), text);
		} catch (error) {
			problems = [`not JSON: ${error.message}`];
		}
		for (const problem of problems) console.error(`  ${problem}`);
		return problems.length ? 2 : 0;
	}

	const storedText = readFileSync(PRICES, "utf8");
	const file = JSON.parse(storedText);
	const stored = problemsOf(file, storedText);
	if (stored.length) throw new Error(`${PRICES} is not valid as it is:\n  ${stored.join("\n  ")}`);
	const day = today();

	const setModel = args.indexOf("--set-model");
	if (setModel !== -1) {
		const [id, input, output] = args.slice(setModel + 1, setModel + 4);
		const changed = withModel(file, id, input, output);
		const model = changed.models[id];
		if (JSON.stringify(model) === JSON.stringify(file.models[id])) {
			console.log(`${id} already costs ${model.input} / ${model.output}: nothing to do.`);
			return 0;
		}
		console.log(`${id}: ${model.input} per million input tokens, ${model.output} per million output tokens (US dollars).`);
		if (model.long_prompt)
			console.log(`  Its long-prompt rate is kept as it was: ${JSON.stringify(model.long_prompt)}. Edit the file if it changed too.`);
		if (check) return 1;
		const version = release({ ...changed, updated: day }, [
			`The cost of ${id} is counted at USD ${model.input} per million input tokens and USD ${model.output} per million output tokens.`,
		]);
		console.log(`Wrote ${PRICES}; version ${version}.`);
		return 0;
	}

	const thresholdText = option(args, "--threshold") ?? "1";
	const threshold = Number(thresholdText) / 100;
	if (!Number.isFinite(threshold) || threshold < 0) throw new Error(`--threshold is a percentage, e.g. 1: ${thresholdText}`);
	const codes = currenciesOf(readFileSync(UNITS, "utf8"));
	console.log("Model prices are not fetched (there is no machine-readable official list): left as they are. --set-model changes one.");

	const url = `${SOURCE}?base=USD&symbols=${codes.join(",")}`;
	const response = await fetch(url, { signal: AbortSignal.timeout(20_000), headers: { accept: "application/json" } });
	if (!response.ok) throw new Error(`${SOURCE} answered ${response.status}`);
	const fresh = ratesFrom(await response.json(), { codes, stored: file.rates, today: day, force: args.includes("--force") });

	const changes = changesOf(file.rates.per_dollar, fresh.rates, threshold);
	if (changes.length === 0) {
		console.log(`No rate moved by more than ${thresholdText}% since ${file.rates.date}: nothing to do.`);
		return 0;
	}
	console.log(`Rates of ${fresh.date} against those of ${file.rates.date}:\n  ${changes.join("\n  ")}`);
	if (check) return 1;
	const rates = { base: "USD", date: fresh.date, source: SOURCE_NAME, per_dollar: fresh.rates };
	// the bank's day can be a time zone ahead of this one; the file is never older than its rates
	const updated = fresh.date > day ? fresh.date : day;
	const version = release({ ...file, updated, rates }, [
		`Costs are converted from US dollars at the European Central Bank's reference rates of ${fresh.date}.`,
	]);
	console.log(`Wrote ${PRICES}; version ${version}.`);
	return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
	main(process.argv.slice(2)).then(
		(status) => process.exit(status),
		(error) => {
			console.error(`update-prices: ${error.message}`);
			process.exit(2);
		},
	);
}
