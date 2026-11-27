/** Making a run again in place, and naming its communities: nothing is replaced or paid for without the click. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { DescribeBeforeRun, RunReplaced, RunRequest, Settings } from "@rp/contracts";
import { answered, RequestError } from "@/lib/request";
import { acceptedWords, type Asking, needsNaming, replacedOf, replaceWords, startRun, valueWords } from "@/lib/run-replace";

const replaced: RunReplaced = {
	run: "NeurIPS-2025",
	title: "NeurIPS 2025",
	communities: 24,
	named: 24,
	communities_will: "change",
	changed: [{ setting: "clustering", was: "fiedler", now: "spectral" }],
};

const conflict = () =>
	answered({
		status: 409,
		ok: false,
		headers: new Headers(),
		body: { kind: "json", value: { error: "NeurIPS 2025 is made already.", replaces: replaced } },
	});

const plan = (api_key_set = true): DescribeBeforeRun => ({
	provider: "anthropic",
	model: "a-model",
	communities_at_most: 30,
	calls_at_most: 30,
	api_key_set,
	describe: true,
	estimate: {
		provider: "anthropic",
		model: "a-model",
		calls: 30,
		tokens: { input: 1000, output: 100, cache_read: 0, cache_write_5m: 0, cache_write_1h: 0 },
		usd: 0.1,
		most_usd: 0.25,
		most_tokens: 5000,
		counting: { kind: "assumed", chars_per_token: 4 },
		caching: "not_reported",
		on_this_computer: false,
	},
});

/** A fake engine: refuses the first request that does not say `replace`, as the engine does for a run that exists. */
function engine(exists: boolean) {
	const sent: RunRequest[] = [];
	return {
		sent,
		send: async (request: RunRequest) => {
			sent.push(request);
			if (exists && !request.replace) conflict();
		},
	};
}

const LABELS: Partial<Record<keyof Settings, string>> = { clustering: "Clustering method" };
const label = (key: keyof Settings) => LABELS[key] ?? key;

test("a 409 that says what it replaces is read from the error, and any other refusal is not", () => {
	assert.throws(conflict, (error: RequestError) => replacedOf(error)?.run === "NeurIPS-2025");
	const busy = new RequestError({ kind: "refused", status: 409, error: "a job is running" });
	assert.equal(replacedOf(busy), null);
	assert.equal(replacedOf(new Error("x")), null);
});

test("the dialog says the title, the communities and how many are named, what changed, and what no longer applies", () => {
	const words = replaceWords(replaced, label);
	assert.equal(words.title, 'Make "NeurIPS 2025" again?');
	assert.equal(words.counted, "24 communities, 24 of them named.");
	assert.deepEqual(words.changes, ["Clustering method: fiedler → spectral"]);
	assert.match(words.will, /will change/);
	assert.match(words.consequence, /names, their overviews and whatever was asked about them \(tables, stances, answers\) no longer apply/);
	const same = replaceWords({ ...replaced, named: 0, communities: 1, communities_will: "may_change", changed: [] }, label);
	assert.equal(same.counted, "1 community, none of them named.");
	assert.match(same.will, /may change/);
	assert.match(same.consequence, /^If they change/);
});

test("a setting's values are said in words: on and off, lists, and not recorded", () => {
	assert.equal(valueWords(true), "on");
	assert.equal(valueWords(null), "not recorded");
	assert.equal(valueWords(["a", "b"]), "a, b");
	assert.equal(valueWords([]), "none");
	assert.equal(valueWords(12000), "12,000");
	assert.equal(
		valueWords("spectral", () => "Spectral"),
		"Spectral",
	);
});

test("a run that would replace one is not made when the person cancels, and nothing is sent again", async () => {
	const fake = engine(true);
	const asked: Asking[] = [];
	const started = await startRun(
		{ venues: ["NeurIPS 2025"], include_workshops: false },
		{
			send: fake.send,
			beforeRun: async () => plan(),
			ask: async (asking) => (asked.push(asking), null),
		},
	);
	assert.deepEqual(started, { kind: "cancelled" });
	assert.equal(fake.sent.length, 1);
	assert.deepEqual(asked, [{ replaced, naming: null, accepted: null }]);
});

test("on confirm the same request is sent again with replace, and its naming as the form accepted it", async () => {
	const fake = engine(true);
	const describe = { provider: "anthropic" as const, model: "a-model", accepted_usd: 0.25 };
	const request: RunRequest = { venues: ["NeurIPS 2025"], include_workshops: false, describe };
	const started = await startRun(request, { send: fake.send, beforeRun: async () => plan(), ask: async () => "unnamed" });
	assert.deepEqual(started, { kind: "started", namesKept: false });
	assert.deepEqual(fake.sent[1], { ...request, replace: true });
});

test("the naming the run form accepted is said again in the dialog before the run is replaced with it", async () => {
	const fake = engine(true);
	const asked: Asking[] = [];
	const describe = { provider: "anthropic" as const, model: "a-model", accepted_usd: 0.25 };
	await startRun(
		{ venues: ["NeurIPS 2025"], include_workshops: false, describe },
		{
			send: fake.send,
			beforeRun: async () => plan(),
			ask: async (asking) => (asked.push(asking), null),
		},
	);
	assert.deepEqual(asked[0].accepted, describe);
	assert.equal(
		acceptedWords(describe),
		"Its communities are then named with a-model, at most USD 0.2500, as accepted in the run form (an estimate).",
	);
	assert.match(acceptedWords({ provider: "anthropic", model: "m", accepted_tokens: 40_000 }), /at most 40k tokens/);
});

test("a redo that names again asks in the same dialog, and sends the most shown only when naming was chosen", async () => {
	const fake = engine(true);
	const asked: Asking[] = [];
	const started = await startRun(
		{ rerun: "NeurIPS-2025", redo: ["download", "embeddings", "descriptions"] },
		{ send: fake.send, beforeRun: async () => plan(), ask: async (asking) => (asked.push(asking), "named") },
	);
	assert.deepEqual(started, { kind: "started", namesKept: false });
	assert.equal(asked[0].naming?.kind, "offer");
	assert.deepEqual(fake.sent[1], {
		rerun: "NeurIPS-2025",
		redo: ["download", "embeddings", "descriptions"],
		describe: { provider: "anthropic", model: "a-model", accepted_usd: 0.25 },
		replace: true,
	});
});

test("a redo whose naming is declined is sent without the descriptions step, and says the names are kept", async () => {
	const fake = engine(true);
	const started = await startRun(
		{ rerun: "NeurIPS-2025", redo: ["download", "embeddings", "descriptions"] },
		{ send: fake.send, beforeRun: async () => plan(), ask: async () => "unnamed" },
	);
	assert.deepEqual(started, { kind: "started", namesKept: true });
	assert.deepEqual(fake.sent[1], { rerun: "NeurIPS-2025", redo: ["download", "embeddings"], replace: true });
	assert.ok(!("describe" in fake.sent[1]));
});

test("with no key for the provider nothing can be named: the redo goes without describing, said so", async () => {
	const fake = engine(true);
	const asked: Asking[] = [];
	const started = await startRun(
		{ rerun: "NeurIPS-2025", redo: ["download", "embeddings", "descriptions"] },
		{ send: fake.send, beforeRun: async () => plan(false), ask: async (asking) => (asked.push(asking), "unnamed") },
	);
	assert.deepEqual(asked[0].naming, { kind: "unavailable" });
	assert.deepEqual(started, { kind: "started", namesKept: true });
	assert.deepEqual(fake.sent[1].redo, ["download", "embeddings"]);
});

test("a new selection that redoes descriptions is asked about naming before anything is sent", async () => {
	const fake = engine(false);
	const order: string[] = [];
	await startRun(
		{ start: "2026-01-01", end: "2026-01-07", categories: [], redo: ["descriptions"] },
		{
			send: async (request) => (order.push("send"), fake.send(request)),
			beforeRun: async () => plan(),
			ask: async () => (order.push("ask"), null),
		},
	);
	assert.deepEqual(order, ["ask"]);
	assert.ok(needsNaming({ rerun: "x", redo: ["descriptions"] }));
	assert.ok(!needsNaming({ rerun: "x", redo: ["descriptions"], describe: { provider: "anthropic", model: "m", accepted_usd: 1 } }));
});
