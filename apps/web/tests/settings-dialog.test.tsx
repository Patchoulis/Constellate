/** Where a refused setting is said in the settings dialog (components/settings-dialog.tsx). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { refusalOf } from "@/components/settings-dialog";

const refused = (changes: object) => ({ changes, error: "max_communities must be a number between 2 and 500" });

test("a refused change is said beside the field it was about, and beside no other", () => {
	const field = { kind: "number" as const, key: "max_communities" as const, label: "Communities" };
	const other = { kind: "number" as const, key: "max_links" as const, label: "Most links per paper" };
	assert.equal(refusalOf(field, refused({ max_communities: 1000 })), "max_communities must be a number between 2 and 500");
	assert.equal(refusalOf(other, refused({ max_communities: 1000 })), null);
	assert.equal(refusalOf(field, null), null);
});

test("a refused provider or effort is said beside the language model, which picks them", () => {
	const model = { kind: "model" as const, key: "describe_model" as const, label: "Language model" };
	assert.ok(refusalOf(model, refused({ describe_provider: "openai", describe_model: "gpt" })));
	assert.ok(refusalOf(model, refused({ describe_effort: "max" })));
	assert.equal(refusalOf(model, refused({ describe: true })), null);
});
