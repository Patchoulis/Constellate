/** The interface's own choices: the key that selects papers, whether marks are shown, and what storage gives back. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { INITIAL_PREFS, onMac, parsePrefs, SELECT_KEYS, selectKeyHeld, selectKeyName, serialisePrefs, type Prefs } from "@/lib/ui/prefs";

const keys = (held: Partial<Record<"ctrlKey" | "metaKey" | "shiftKey" | "altKey", boolean>>) => ({
	ctrlKey: false,
	metaKey: false,
	shiftKey: false,
	altKey: false,
	...held,
});

test("the select key is held when its own key is down, and no other key counts for it", () => {
	assert.ok(selectKeyHeld("primary", keys({ ctrlKey: true })), "Ctrl on Windows and Linux");
	assert.ok(selectKeyHeld("primary", keys({ metaKey: true })), "Command on a Mac");
	assert.ok(!selectKeyHeld("primary", keys({ shiftKey: true, altKey: true })));
	assert.ok(selectKeyHeld("shift", keys({ shiftKey: true })));
	assert.ok(!selectKeyHeld("shift", keys({ ctrlKey: true, metaKey: true, altKey: true })));
	assert.ok(selectKeyHeld("alt", keys({ altKey: true })));
	assert.ok(!selectKeyHeld("alt", keys({ ctrlKey: true, metaKey: true, shiftKey: true })));
	for (const key of SELECT_KEYS) assert.ok(!selectKeyHeld(key, keys({})), `${key} with nothing held`);
});

test("a key is named as it is written on the keyboard in front of the person", () => {
	assert.equal(selectKeyName("primary", false), "Ctrl");
	assert.equal(selectKeyName("primary", true), "⌘");
	assert.equal(selectKeyName("alt", false), "Alt");
	assert.equal(selectKeyName("alt", true), "⌥");
	assert.equal(selectKeyName("shift", false), "Shift");
	assert.equal(selectKeyName("shift", true), "Shift");
	assert.ok(onMac("MacIntel"));
	assert.ok(onMac("iPad"));
	for (const platform of ["Win32", "Linux x86_64", ""]) assert.ok(!onMac(platform), platform);
});

test("the choices come back from storage as they were written", () => {
	const chosen: Prefs = { selectKey: "alt", marksShown: false, showMarksOnMark: false };
	assert.deepEqual(parsePrefs(serialisePrefs(chosen)), chosen);
	assert.deepEqual(parsePrefs(serialisePrefs(INITIAL_PREFS)), INITIAL_PREFS);
});

test("whatever else is in storage reads as the initial choices, one choice at a time, never as an error", () => {
	for (const text of [
		null,
		undefined,
		"",
		"{",
		"[]",
		"3",
		"null",
		'{"version":2,"prefs":{"selectKey":"alt"}}',
		'{"version":1}',
		'{"version":1,"prefs":null}',
	])
		assert.deepEqual(parsePrefs(text), INITIAL_PREFS, String(text));
	// one choice that cannot be trusted does not take the others with it
	const stored = JSON.stringify({ version: 1, prefs: { selectKey: "hyper", marksShown: false, showMarksOnMark: "no", other: 1 } });
	assert.deepEqual(parsePrefs(stored), { ...INITIAL_PREFS, marksShown: false });
});

test("the initial choices are a key every keyboard has, with marks shown", () => {
	assert.ok(SELECT_KEYS.includes(INITIAL_PREFS.selectKey));
	assert.equal(INITIAL_PREFS.marksShown, true);
	assert.equal(INITIAL_PREFS.showMarksOnMark, true);
});
