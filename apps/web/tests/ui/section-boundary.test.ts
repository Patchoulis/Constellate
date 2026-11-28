/** A section of the window that stopped while being drawn: what it says, and when it is tried again. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { changedKeys, SECTION_SHOWN, sectionLines, sectionStep, type Section } from "@/lib/ui/section-boundary";

const failed = (section: Section) => {
	assert.equal(section.state, "failed");
	return section as Extract<Section, { state: "failed" }>;
};

test("a section that stops says so in its own words, with the reason in a line", () => {
	const section = failed(
		sectionStep(SECTION_SHOWN, { kind: "failed", error: new TypeError("Cannot read properties of undefined (reading 'x')\n    at draw") }),
	);
	assert.deepEqual(sectionLines(section, "the graph"), {
		title: "The graph could not be shown.",
		reason: "Cannot read properties of undefined (reading 'x')",
		again: null,
	});
	// whatever was thrown: not every throw is an Error
	assert.equal(failed(sectionStep(SECTION_SHOWN, { kind: "failed", error: "a string" })).reason, "No reason was given.");
	assert.equal(failed(sectionStep(SECTION_SHOWN, { kind: "failed", error: null })).reason, "No reason was given.");
});

test("trying again draws it again, and a retry that did not help says how often it was tried", () => {
	let section = sectionStep(SECTION_SHOWN, { kind: "failed", error: new Error("odd") });
	section = sectionStep(section, { kind: "retried" });
	assert.deepEqual(section, { state: "shown", tries: 1 });
	section = sectionStep(section, { kind: "failed", error: new Error("odd") });
	assert.equal(sectionLines(failed(section), "the reader").again, "Tried again once: it stopped the same way.");
	section = sectionStep(sectionStep(section, { kind: "retried" }), { kind: "failed", error: new Error("odd") });
	assert.equal(sectionLines(failed(section), "the reader").again, "Tried again 2 times: it stopped the same way.");
	// a retry of a section that is shown changes nothing
	assert.equal(sectionStep(SECTION_SHOWN, { kind: "retried" }), SECTION_SHOWN);
});

test("new data to draw from tries the section again from the start", () => {
	const section = sectionStep({ state: "shown", tries: 3 }, { kind: "failed", error: new Error("odd") });
	assert.deepEqual(sectionStep(section, { kind: "changed" }), SECTION_SHOWN);
	const data = { nodes: [] };
	assert.equal(changedKeys([data, "run"], [data, "run"]), false);
	assert.equal(changedKeys([data, "run"], [{ nodes: [] }, "run"]), true);
	assert.equal(changedKeys(undefined, undefined), false);
	assert.equal(changedKeys([1], [1, 2]), true);
});
