/** Marked papers: what can be done to a run's list of marks, and what is read back from storage. */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	addMark,
	addMarks,
	forgetMarks,
	isMarked,
	keepKnown,
	marksKey,
	parseMarks,
	removeMark,
	serialiseMarks,
	toggleMark,
	type Marks,
} from "@/lib/community/marks";

test("a marked paper is listed newest first, and only once", () => {
	let marks: Marks = [];
	for (const id of ["a", "b", "c"]) marks = addMark(marks, id);
	assert.deepEqual(marks, ["c", "b", "a"]);
	assert.deepEqual(addMark(marks, "b"), ["c", "b", "a"], "marking it again changes nothing");
	assert.ok(isMarked(marks, "b"));
	assert.ok(!isMarked(marks, "d"));
});

test("unmarking takes out that paper and keeps the order of the rest", () => {
	assert.deepEqual(removeMark(["c", "b", "a"], "b"), ["c", "a"]);
	assert.deepEqual(removeMark(["c", "b", "a"], "x"), ["c", "b", "a"], "a paper that is not marked");
	assert.deepEqual(removeMark([], "x"), []);
});

test("toggling marks and unmarks; twice on an unmarked paper is no change", () => {
	const before: Marks = ["b", "a"];
	assert.deepEqual(toggleMark(before, "c"), ["c", "b", "a"]);
	assert.deepEqual(toggleMark(before, "b"), ["a"]);
	assert.deepEqual(toggleMark(toggleMark(before, "c"), "c"), before);
	assert.deepEqual(toggleMark(toggleMark([], "c"), "c"), []);
	// twice on a marked paper: unmarked, then marked again as the newest
	assert.deepEqual(toggleMark(toggleMark(before, "a"), "a"), ["a", "b"]);
});

test("the core never changes the list it is given", () => {
	const marks = Object.freeze(["b", "a"]) as Marks;
	addMark(marks, "c");
	removeMark(marks, "a");
	toggleMark(marks, "b");
	keepKnown(marks, new Set(["a"]));
	assert.deepEqual(marks, ["b", "a"]);
});

test("marks survive being written and read back, order included", () => {
	const marks: Marks = ["2401.00001", "neurips-2024:abc", 'with "quotes" and ünïcode'];
	assert.deepEqual(parseMarks(serialiseMarks(marks)), marks);
	assert.deepEqual(parseMarks(serialiseMarks([])), []);
});

test("whatever else is in storage reads as no marks, never as an error", () => {
	for (const stored of [
		null,
		undefined,
		"",
		"not json",
		"{",
		"null",
		"42",
		'"a string"',
		"[]",
		'["a", "b"]', // a bare list: not this format
		"{}",
		'{"papers": ["a"]}', // no version
		'{"version": 2, "papers": ["a"]}', // a version this code does not know
		'{"version": "1", "papers": ["a"]}',
		'{"version": 1}',
		'{"version": 1, "papers": "a"}',
		'{"version": 1, "papers": {"0": "a"}}',
	])
		assert.deepEqual(parseMarks(stored), [], String(stored));
});

test("only the ids that can be trusted are read from a damaged list", () => {
	const stored = JSON.stringify({ version: 1, papers: ["a", 3, null, "", { id: "b" }, "b", "a", ["c"], "x".repeat(600)] });
	assert.deepEqual(parseMarks(stored), ["a", "b"], "not strings, empty, too long and repeated ids are left out");
});

test("a list far longer than anyone marks is read only up to a limit", () => {
	const papers = Array.from({ length: 20000 }, (_, index) => `p${index}`);
	const read = parseMarks(JSON.stringify({ version: 1, papers }));
	assert.equal(read.length, 5000);
	assert.equal(read[0], "p0");
});

test("marks of papers that are not in the run's graph are left out", () => {
	const known = new Set(["a", "b", "c"]);
	assert.deepEqual(keepKnown(["d", "c", "x", "a"], known), ["c", "a"]);
	assert.deepEqual(keepKnown(["d"], known), []);
	const all: Marks = ["c", "a"];
	assert.equal(keepKnown(all, known), all, "nothing to leave out: the same list");
	assert.deepEqual(keepKnown(["a"], new Set()), []);
});

test("every run has its own storage key, and no name reaches another's", () => {
	assert.notEqual(marksKey("neurips-2024"), marksKey("icml-2024"));
	assert.equal(marksKey("neurips-2024"), "constellate.marks.neurips-2024");
	// a name that looks like another key, or like a path, stays inside its own key
	assert.notEqual(marksKey("a.b"), marksKey("a"));
	assert.notEqual(marksKey("a/b"), marksKey("a%2Fb"));
	assert.ok(marksKey("../other").startsWith("constellate.marks."));
	assert.equal(marksKey(""), "constellate.marks.");
});

test("what is stored under one run's key reads back as that run's marks only", () => {
	const storage = new Map<string, string>();
	storage.set(marksKey("run one"), serialiseMarks(["a", "b"]));
	storage.set(marksKey("run two"), serialiseMarks(toggleMark([], "z")));
	assert.deepEqual(parseMarks(storage.get(marksKey("run one"))), ["a", "b"]);
	assert.deepEqual(parseMarks(storage.get(marksKey("run two"))), ["z"]);
	assert.deepEqual(parseMarks(storage.get(marksKey("run three"))), []);
});

test("deleting a run forgets its marks and no other run's, and storage switched off is no error", () => {
	const storage = new Map<string, string>([
		[marksKey("gone"), serialiseMarks(["a"])],
		[marksKey("kept"), serialiseMarks(["b"])],
	]);
	const localStorage = {
		getItem: (key: string) => storage.get(key) ?? null,
		setItem: (key: string, value: string) => void storage.set(key, value),
		removeItem: (key: string) => void storage.delete(key),
	};
	const global = globalThis as { window?: unknown };
	const before = global.window;
	try {
		global.window = { localStorage };
		forgetMarks("gone");
		assert.equal(storage.get(marksKey("gone")), undefined);
		assert.equal(storage.get(marksKey("kept")), serialiseMarks(["b"]));
		global.window = {
			get localStorage(): never {
				throw new Error("storage is disabled");
			},
		};
		assert.doesNotThrow(() => forgetMarks("kept"));
	} finally {
		global.window = before;
	}
});

test("several papers marked at once (a lasso's) become the newest, each once, and those marked already stay put", () => {
	const marks: Marks = ["b", "a"];
	assert.deepEqual(addMarks(marks, ["c", "d"]), ["c", "d", "b", "a"]);
	assert.deepEqual(addMarks(marks, ["a", "c", "c"]), ["c", "b", "a"], "a is where it was, c is added once");
	assert.equal(addMarks(marks, ["a", "b"]), marks, "nothing new: the same list, so nothing is rendered or written again");
	assert.equal(addMarks(marks, []), marks);
	assert.deepEqual(addMarks([], ["x"]), ["x"]);
});
