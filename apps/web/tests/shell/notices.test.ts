/** What the engine reports that is not about one run (lib/shell/notices.ts): its warnings, and the import problems dismissed. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { engineWarnings, keepDismissed, notDismissed, readDismissed } from "@/lib/shell/notices";

const KEY = "constellate.dismissed-import-problems";

/** Run `body` with `storage` as the window's session storage, and the one there was put back after. */
function withSessionStorage(storage: unknown, body: () => void): void {
	const before = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
	Object.defineProperty(globalThis, "sessionStorage", { configurable: true, get: () => storage });
	try {
		body();
	} finally {
		if (before) Object.defineProperty(globalThis, "sessionStorage", before);
		else delete (globalThis as { sessionStorage?: unknown }).sessionStorage;
	}
}

function memory(start: Record<string, string> = {}) {
	const kept = new Map(Object.entries(start));
	return {
		kept,
		getItem: (key: string) => kept.get(key) ?? null,
		setItem: (key: string, value: string) => void kept.set(key, String(value)),
	};
}

test("an import problem the engine also lists among its warnings is shown once, as an import problem", () => {
	const warnings = ["HTTPS is off", "library: runs.json could not be moved in"];
	assert.deepEqual(engineWarnings(warnings, ["library: runs.json could not be moved in"]), ["HTTPS is off"]);
	assert.deepEqual(engineWarnings(warnings, undefined), warnings);
	assert.deepEqual(engineWarnings(undefined, ["anything"]), []);
});

test("the import problems dismissed in this window are not shown again, the others are, in their order", () => {
	const problems = ["a could not be moved in", "b could not be moved in", "c could not be moved in"];
	assert.deepEqual(notDismissed(problems, new Set(["b could not be moved in"])), [problems[0], problems[2]]);
	assert.deepEqual(notDismissed(problems, new Set()), problems);
	assert.deepEqual(notDismissed(undefined, new Set(["a could not be moved in"])), []);
});

test("the problems dismissed are kept in the window's session as a JSON list and read back the same", () => {
	const storage = memory();
	withSessionStorage(storage, () => {
		assert.deepEqual([...readDismissed()], [], "nothing kept yet");
		keepDismissed(new Set(["a could not be moved in", 'b "quoted" could not be moved in']));
		assert.equal(storage.kept.get(KEY), '["a could not be moved in","b \\"quoted\\" could not be moved in"]');
		assert.deepEqual([...readDismissed()], ["a could not be moved in", 'b "quoted" could not be moved in']);
	});
});

test("what is kept that is not a list of texts is read as nothing dismissed, or as its texts alone", () => {
	for (const [kept, expected] of [
		["{ not json", []],
		['{"a": 1}', []],
		['"a text"', []],
		['["a", 2, null, "b"]', ["a", "b"]],
	] as const) {
		withSessionStorage(memory({ [KEY]: kept }), () => assert.deepEqual([...readDismissed()], expected, kept));
	}
});

test("session storage that is switched off dismisses only until the page is reloaded, and is no error", () => {
	const refusing = {
		getItem: () => {
			throw new Error("storage is disabled");
		},
		setItem: () => {
			throw new Error("the quota is exceeded");
		},
	};
	withSessionStorage(refusing, () => {
		assert.deepEqual([...readDismissed()], []);
		keepDismissed(new Set(["a could not be moved in"])); // refused quietly: nothing to assert but that it returns
	});
	withSessionStorage(undefined, () => assert.deepEqual([...readDismissed()], []));
});
