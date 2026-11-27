/** Saving settings (lib/settings-saves.ts): the latest answer wins, and a refusal is kept with its reason. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { NO_SAVES, saveStep, type Saves } from "@/lib/settings-saves";

interface Settings {
	max_communities: number;
	describe: boolean;
}

const start: Saves<Settings> = NO_SAVES;
const before: Settings = { max_communities: 40, describe: true };

test("a saved change makes the backend's answer the settings shown", () => {
	const asked = saveStep(start, { kind: "asked" });
	const saved = saveStep(asked, { kind: "saved", id: 1, changes: { describe: false }, settings: { ...before, describe: false } });
	assert.deepEqual(saved.saved, { ...before, describe: false });
	assert.equal(saved.refused, null);
});

test("a refused change is kept with the backend's reason, not quietly dropped", () => {
	const asked = saveStep({ ...start, saved: before }, { kind: "asked" });
	const error = "max_communities must be a number between 2 and 500";
	const refused = saveStep(asked, { kind: "refused", id: 1, changes: { max_communities: 1000 }, error });
	assert.deepEqual(refused.refused, { changes: { max_communities: 1000 }, error });
	assert.deepEqual(refused.saved, before, "what is shown stays until the settings are read again");
	const read = saveStep(refused, { kind: "read", id: 1, settings: before });
	assert.deepEqual(read.saved, before);
	assert.deepEqual(read.refused?.error, error, "reading the settings again does not hide the reason");
});

test("an older save's answer arriving after a newer one's does not undo it", () => {
	let state = saveStep(saveStep(start, { kind: "asked" }), { kind: "asked" });
	state = saveStep(state, { kind: "saved", id: 2, changes: { describe: true }, settings: { max_communities: 60, describe: true } });
	state = saveStep(state, { kind: "saved", id: 1, changes: { max_communities: 60 }, settings: { max_communities: 60, describe: false } });
	assert.deepEqual(state.saved, { max_communities: 60, describe: true });
});

test("settings read after a refusal are not shown once a newer save was asked for", () => {
	let state = saveStep(start, { kind: "asked" });
	state = saveStep(state, { kind: "refused", id: 1, changes: { max_communities: 1 }, error: "too few" });
	state = saveStep(state, { kind: "asked" });
	state = saveStep(state, { kind: "read", id: 1, settings: before });
	assert.equal(state.saved, undefined);
});

test("a refusal stays until that setting is saved, or the message is put away", () => {
	let state = saveStep(start, { kind: "asked" });
	state = saveStep(state, { kind: "refused", id: 1, changes: { max_communities: 1 }, error: "too few" });
	state = saveStep(saveStep(state, { kind: "asked" }), { kind: "saved", id: 2, changes: { describe: false }, settings: before });
	assert.equal(state.refused?.error, "too few", "saving another setting does not say this one was saved");
	state = saveStep(saveStep(state, { kind: "asked" }), { kind: "saved", id: 3, changes: { max_communities: 8 }, settings: before });
	assert.equal(state.refused, null);
	state = saveStep(state, { kind: "refused", id: 3, changes: { describe: true }, error: "no key" });
	assert.equal(saveStep(state, { kind: "dismissed" }).refused, null);
});
