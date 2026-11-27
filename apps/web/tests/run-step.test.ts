/** The run's step in the job view: what it is doing, how far it has got, and of how many steps. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunStep, RunStepName, StepCount } from "@rp/contracts";
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

test("the embedding model's first download is said in megabytes, as the engine's log says it", () => {
	const download = step({
		step: "model",
		title: "the embedding model",
		number: 2,
		count: { done: 312 * 1_048_576, of: 1180 * 1_048_576, unit: "bytes" },
	});
	assert.equal(stepLine(download), "Step 2 of 10: downloading the embedding model: 312 of 1,180 MB");
	assert.equal(countWords({ done: 0, of: 1_237_000_000, unit: "bytes" }), "0 of 1,180 MB");
	assert.equal(stepLine(step({ step: "model", title: "the embedding model", number: 2 })), "Step 2 of 10: loading the embedding model");
});

test("embedding says how many papers of how many, with thousands separated, and the device after it", () => {
	assert.equal(stepLine(step({ count: { done: 3120, of: 12487, unit: "papers" } })), "Step 3 of 10: embedding 3,120 of 12,487 papers");
	assert.equal(
		stepLine(step({ count: { done: 3120, of: 12487, unit: "papers" }, detail: "on the graphics card" })),
		"Step 3 of 10: embedding 3,120 of 12,487 papers · on the graphics card",
	);
});

test("slices are said as days or conferences, communities as communities, one of one in the singular", () => {
	assert.equal(
		stepLine(step({ step: "gather", title: "gather the papers", number: 1, count: { done: 1, of: 4, unit: "slices" } })),
		"Step 1 of 10: gathering the papers: 1 of 4 days or conferences",
	);
	assert.equal(
		stepLine(step({ step: "describe", title: "describe communities", number: 9, count: { done: 3, of: 24, unit: "communities" } })),
		"Step 9 of 10: naming 3 of 24 communities",
	);
	assert.equal(countWords({ done: 1, of: 1, unit: "papers" }), "1 of 1 paper");
});

test("every step and every unit is said in words, never as a bare name or number", () => {
	const names: RunStepName[] = [
		"gather",
		"model",
		"embed",
		"detect",
		"filter",
		"similarity",
		"communities",
		"summarise",
		"describe",
		"write",
	];
	const units: StepCount["unit"][] = ["papers", "slices", "bytes", "communities"];
	for (const [index, name] of names.entries()) {
		const bare = stepLine(step({ step: name, title: name, number: index + 1 }));
		assert.match(bare, new RegExp(`^Step ${index + 1} of 10: [a-z]`), bare);
		assert.ok(!bare.endsWith(`: ${name}`) || name === "embed", bare);
		for (const unit of units) {
			const line = stepLine(step({ step: name, title: name, number: index + 1, count: { done: 2_097_152, of: 4_194_304, unit } }));
			assert.match(line, unit === "bytes" ? /2 of 4 MB/ : /2,097,152 of 4,194,304 (papers|days or conferences|communities)/, line);
		}
	}
});
