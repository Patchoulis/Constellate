/** The interface's own choices: the key that selects papers, whether marks are shown, and what storage gives back. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	INITIAL_PREFS,
	onMac,
	parsePrefs,
	PREFS_KEY,
	SELECT_KEYS,
	selectKeyHeld,
	selectKeyName,
	serialisePrefs,
	setPrefs,
	type Prefs,
} from "@/lib/ui/prefs";
import { INTRO_WHENS } from "@/lib/graph/intro";
import { INTRO_CHOICES, STAR_CHOICES } from "@/components/shell/settings-dialog";
import { STAR_FIELDS } from "@/lib/graph/sky";

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
	const chosen: Prefs = { selectKey: "alt", showMarksOnMark: false, intro: "never", stars: "many", regionFrom: 60 };
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
	const stored = JSON.stringify({
		version: 1,
		prefs: { selectKey: "hyper", marksShown: false, showMarksOnMark: "no", stars: "lots", other: 1 },
	});
	assert.deepEqual(parsePrefs(stored), INITIAL_PREFS);
});

test("the initial choices are a key every keyboard has, marking shows hidden marks again", () => {
	assert.ok(SELECT_KEYS.includes(INITIAL_PREFS.selectKey));
	assert.equal(INITIAL_PREFS.showMarksOnMark, true);
});

test("the constellation intro plays whenever a run is opened unless chosen otherwise, and a choice that is not one is not kept", () => {
	assert.equal(INITIAL_PREFS.intro, "opened");
	// a choice made explicitly is kept, whichever it is: nothing falls through to "never"
	for (const intro of INTRO_WHENS) assert.equal(parsePrefs(serialisePrefs({ ...INITIAL_PREFS, intro })).intro, intro);
	for (const intro of ["always", "", "Never", 1, null, true, { made: true }, ["never"], undefined]) {
		const stored = JSON.stringify({ version: 1, prefs: { selectKey: "shift", intro } });
		assert.deepEqual(parsePrefs(stored), { ...INITIAL_PREFS, selectKey: "shift" }, JSON.stringify(intro));
	}
	// choices stored before the intro existed keep what they were, and the intro gets its initial choice
	const older = JSON.stringify({ version: 1, prefs: { selectKey: "alt", marksShown: false, showMarksOnMark: true } });
	assert.deepEqual(parsePrefs(older), {
		selectKey: "alt",
		showMarksOnMark: true,
		intro: "opened",
		stars: "few",
		regionFrom: 25,
	});
	assert.equal(parsePrefs(null).intro, "opened", "nothing stored");
});

test("Settings offers every choice of the intro once, in words, the initial one first", () => {
	assert.deepEqual(
		INTRO_CHOICES.map((choice) => choice.value),
		[...INTRO_WHENS],
	);
	assert.deepEqual(
		INTRO_CHOICES.map((choice) => choice.label),
		["Whenever a run is opened", "When a run is made", "Never"],
	);
	assert.equal(INTRO_CHOICES[0].value, INITIAL_PREFS.intro);
});

test("marked papers become a region from 25 unless chosen otherwise, within 5 to 500, and anything else reads as 25", () => {
	assert.equal(INITIAL_PREFS.regionFrom, 25);
	for (const regionFrom of [5, 40, 500]) assert.equal(parsePrefs(serialisePrefs({ ...INITIAL_PREFS, regionFrom })).regionFrom, regionFrom);
	const stored = (regionFrom: unknown) => parsePrefs(JSON.stringify({ version: 1, prefs: { intro: "never", regionFrom } }));
	assert.equal(stored(2).regionFrom, 5, "below the bounds: the least");
	assert.equal(stored(10_000).regionFrom, 500, "above them: the most");
	assert.equal(stored(30.4).regionFrom, 30);
	for (const regionFrom of ["40", null, true, [], {}, "many"])
		assert.deepEqual(stored(regionFrom), { ...INITIAL_PREFS, intro: "never" }, JSON.stringify(regionFrom));
});

test("background stars start at a few, every choice is kept, and anything else reads as a few", () => {
	assert.equal(INITIAL_PREFS.stars, "few");
	for (const stars of STAR_FIELDS) assert.equal(parsePrefs(serialisePrefs({ ...INITIAL_PREFS, stars })).stars, stars);
	for (const stars of ["lots", "", "Many", 2, null, false, ["many"]]) {
		const stored = JSON.stringify({ version: 1, prefs: { intro: "never", stars } });
		assert.deepEqual(parsePrefs(stored), { ...INITIAL_PREFS, intro: "never" }, JSON.stringify(stars));
	}
	assert.deepEqual(
		STAR_CHOICES.map((choice) => [choice.value, choice.label]),
		[
			["off", "Off"],
			["few", "A few"],
			["many", "Many"],
		],
	);
});

test("a change of the choices is laid over the ones remembered and remembered with them, even after storage refused one", () => {
	const kept = new Map<string, string>([[PREFS_KEY, serialisePrefs({ ...INITIAL_PREFS, selectKey: "shift" })]]);
	const working = {
		getItem: (key: string) => kept.get(key) ?? null,
		setItem: (key: string, value: string) => void kept.set(key, value),
	};
	const refusing = {
		getItem: () => null,
		setItem: () => {
			throw new Error("the quota is exceeded");
		},
	};
	const global = globalThis as { window?: unknown };
	const before = global.window;
	try {
		global.window = { localStorage: working };
		setPrefs({ regionFrom: 40 });
		assert.deepEqual(parsePrefs(kept.get(PREFS_KEY)), { ...INITIAL_PREFS, selectKey: "shift", regionFrom: 40 });
		// storage switched off: the change still holds in the window, and is written with the next one
		global.window = { localStorage: refusing };
		setPrefs({ showMarksOnMark: false });
		global.window = { localStorage: working };
		setPrefs({ selectKey: "alt" });
		assert.deepEqual(parsePrefs(kept.get(PREFS_KEY)), { ...INITIAL_PREFS, selectKey: "alt", regionFrom: 40, showMarksOnMark: false });
	} finally {
		global.window = before;
	}
});
