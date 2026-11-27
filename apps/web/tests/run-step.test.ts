/** The run's step in the job view: what it is doing, how far it has got, and of how many steps. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunStep } from "@rp/contracts";
import { countWords, stepLine } from "@/lib/run-step";

const step = (more: Partial<RunStep>): RunStep => ({
	step: "embed",
	title: "embed",
	number: 3,
	of: 10,
	count: null,
	detail: null,
	...more,
});

test("the embedding model's download is said in megabytes, as the engine's log says it", () => {
	const download = step({
		step: "model",
		title: "the embedding model",
		number: 2,
		count: { done: 312 * 1_048_576, of: 1180 * 1_048_576, unit: "bytes" },
	});
	assert.equal(stepLine(download), "Downloading the embedding model: 312 of 1,180 MB (step 2 of 10)");
	assert.equal(countWords({ done: 0, of: 1_237_000_000, unit: "bytes" }), "0 of 1,180 MB");
});

test("a step that counts papers says how many of how many, with thousands separated", () => {
	assert.equal(stepLine(step({ count: { done: 3120, of: 12487, unit: "papers" } })), "Embed: 3,120 of 12,487 papers (step 3 of 10)");
});

test("a step's detail follows its count, and a step that counts nothing says only what it is", () => {
	assert.equal(
		stepLine(step({ count: { done: 1, of: 4, unit: "slices" }, detail: "embedding on the processor" })),
		"Embed: 1 of 4 slices (step 3 of 10) · embedding on the processor",
	);
	assert.equal(
		stepLine(step({ step: "similarity", title: "similarity -> adjacency", number: 6 })),
		"Similarity -> adjacency (step 6 of 10)",
	);
});
