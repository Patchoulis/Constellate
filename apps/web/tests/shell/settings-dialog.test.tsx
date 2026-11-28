/** Where a refused setting is said in the settings dialog (components/shell/settings-dialog.tsx). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { refusalOf } from "@/components/shell/settings-dialog";

const refused = (changes: object) => ({ changes, error: "max_communities must be a number between 2 and 500" });

test("a refused change is said beside the field it was about, and beside no other", () => {
	const field = { kind: "number" as const, key: "max_communities" as const, label: "Communities" };
	const other = { kind: "number" as const, key: "max_links" as const, label: "Most links per paper" };
	assert.equal(refusalOf(field, refused({ max_communities: 1000 })), "max_communities must be a number between 2 and 500");
	assert.equal(refusalOf(other, refused({ max_communities: 1000 })), null);
	assert.equal(refusalOf(field, null), null);
});

test("a refused provider or effort is said beside the language model, which picks them", () => {
	const model = { kind: "model" as const, role: "naming" as const, key: "describe_model" as const, label: "Language model" };
	assert.ok(refusalOf(model, refused({ describe_provider: "openai", describe_model: "gpt" })));
	assert.ok(refusalOf(model, refused({ describe_effort: "max" })));
	assert.equal(refusalOf(model, refused({ describe: true })), null);
	assert.equal(refusalOf(model, refused({ reading_provider: "openai" })), null, "the model that reads papers is another field");
});

test("a refused choice of the model that reads papers is said beside that model, not the naming one", () => {
	const reading = { kind: "model" as const, role: "reading" as const, key: "reading_model" as const, label: "Model that reads papers" };
	assert.ok(refusalOf(reading, refused({ reading_provider: "compatible" })));
	assert.ok(refusalOf(reading, refused({ reading_effort: "max" })));
	assert.ok(refusalOf(reading, refused({ reading_provider: null })));
	assert.equal(refusalOf(reading, refused({ describe_model: "gpt" })), null);
});
