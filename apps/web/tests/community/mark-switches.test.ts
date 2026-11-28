/** Each marked paper's two switches: shown on the graph or not, its links drawn or not (lib/community/marks.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
	NO_MARKS,
	addMark,
	addMarks,
	keepKnownState,
	markedListLine,
	marksOnGraph,
	parseMarkState,
	removeMark,
	serialiseMarks,
	setMarkLinks,
	setMarkShown,
	toggleMark,
	withPapers,
	type MarkState,
} from "@/lib/community/marks";

const state = (papers: string[], hidden: string[] = [], noLinks: string[] = []): MarkState => ({
	papers,
	hidden: new Set(hidden),
	noLinks: new Set(noLinks),
});
const plain = (marks: MarkState) => ({ papers: [...marks.papers], hidden: [...marks.hidden].sort(), noLinks: [...marks.noLinks].sort() });

test("marks stored before the switches read as every mark shown with its links", () => {
	const older = JSON.stringify({ version: 1, papers: ["a", "b"] });
	assert.deepEqual(plain(parseMarkState(older)), { papers: ["a", "b"], hidden: [], noLinks: [] });
	// and marks with nothing switched are stored as they always were, so an older version reads them too
	assert.equal(serialiseMarks(state(["a", "b"])), older);
});

test("the switches come back from storage as they were written", () => {
	const marks = state(["a", "b", "c"], ["b"], ["a", "c"]);
	assert.deepEqual(plain(parseMarkState(serialiseMarks(marks))), plain(marks));
});

test("a switch that cannot be trusted reads as the mark shown with its links, and the marks themselves still read", () => {
	for (const [hidden, noLinks] of [
		["b", 3],
		[{ b: true }, null],
		[[1, 2], [true]],
		[["", "x".repeat(600)], ["nope"]],
	]) {
		const text = JSON.stringify({ version: 1, papers: ["a", "b"], hidden, noLinks });
		assert.deepEqual(plain(parseMarkState(text)), { papers: ["a", "b"], hidden: [], noLinks: [] }, text);
	}
	// a switch naming a paper that is not marked is not kept
	assert.deepEqual(plain(parseMarkState(JSON.stringify({ version: 1, papers: ["a"], hidden: ["z", "a"], noLinks: ["q"] }))), {
		papers: ["a"],
		hidden: ["a"],
		noLinks: [],
	});
	for (const text of [null, "", "{", "[]", '{"version":2,"papers":["a"],"hidden":["a"]}']) assert.deepEqual(parseMarkState(text), NO_MARKS);
});

test("a mark is hidden and shown again, its links switched off and on, and nothing else changes", () => {
	let marks = state(["a", "b", "c"]);
	marks = setMarkShown(marks, ["b"], false);
	assert.deepEqual([...marks.hidden], ["b"]);
	marks = setMarkLinks(marks, ["a"], false);
	assert.deepEqual([...marks.noLinks], ["a"]);
	assert.deepEqual(marks.papers, ["a", "b", "c"], "every paper stays marked");
	marks = setMarkShown(marks, ["b"], true);
	marks = setMarkLinks(marks, ["a"], true);
	assert.deepEqual(plain(marks), { papers: ["a", "b", "c"], hidden: [], noLinks: [] });
	// a paper that is not marked has no switches to change
	assert.equal(setMarkShown(marks, ["z"], false), marks);
	assert.equal(setMarkLinks(marks, ["z"], false), marks);
	// nothing to change: the same marks, so nothing is stored or drawn again
	assert.equal(setMarkShown(marks, ["a"], true), marks);
});

test("all marks are shown or hidden, their links switched on or off, at once", () => {
	const all = ["a", "b", "c"];
	let marks = setMarkShown(state(all), all, false);
	assert.deepEqual([...marks.hidden].sort(), all);
	marks = setMarkShown(marks, all, true);
	assert.equal(marks.hidden.size, 0);
	marks = setMarkLinks(marks, all, false);
	assert.deepEqual([...marks.noLinks].sort(), all);
	marks = setMarkLinks(marks, all, true);
	assert.equal(marks.noLinks.size, 0);
});

test("unmarking a paper drops its switches, and a paper marked again, alone or with a lasso, is shown with its links", () => {
	let marks = state(["a", "b"], ["a"], ["a", "b"]);
	marks = withPapers(marks, (papers) => removeMark(papers, "a"));
	assert.deepEqual(plain(marks), { papers: ["b"], hidden: [], noLinks: ["b"] });
	marks = withPapers(marks, (papers) => addMark(papers, "a"));
	assert.deepEqual(plain(marks), { papers: ["a", "b"], hidden: [], noLinks: ["b"] });
	marks = setMarkShown(marks, ["a"], false);
	marks = withPapers(marks, (papers) => toggleMark(papers, "a"));
	marks = withPapers(marks, (papers) => addMarks(papers, ["a", "c"]));
	assert.deepEqual(plain(marks), { papers: ["a", "c", "b"], hidden: [], noLinks: ["b"] });
	// a paper marked already keeps its switches when a lasso goes round it again
	marks = setMarkShown(marks, ["b"], false);
	assert.deepEqual([...withPapers(marks, (papers) => addMarks(papers, ["b", "d"])).hidden], ["b"]);
});

test("marks whose paper left the graph go, with their switches", () => {
	const marks = state(["a", "gone", "b"], ["gone", "a"], ["gone"]);
	assert.deepEqual(plain(keepKnownState(marks, new Set(["a", "b"]))), { papers: ["a", "b"], hidden: ["a"], noLinks: [] });
	const kept = state(["a"], ["a"]);
	assert.equal(keepKnownState(kept, new Set(["a", "b"])), kept, "nothing gone: the same marks");
});

test("the graph is given the marks shown, newest first, and which of them have no links drawn", () => {
	const onGraph = marksOnGraph(state(["a", "b", "c", "d"], ["b"], ["b", "c"]));
	assert.deepEqual(onGraph.shown, ["a", "c", "d"]);
	assert.deepEqual([...onGraph.noLinks], ["c"], "a hidden mark's links are not the graph's concern");
	const none = marksOnGraph(state(["a"]));
	assert.deepEqual([...none.shown], ["a"]);
	assert.equal(none.noLinks.size, 0);
});

test("the Marked list's header says how many are marked, hidden and without links, and only what there is", () => {
	assert.equal(markedListLine(1, 0, 0), "1 paper marked in this run, newest first");
	assert.equal(markedListLine(500, 120, 0), "500 papers marked in this run, newest first · 120 hidden on the graph");
	assert.equal(markedListLine(12, 3, 2), "12 papers marked in this run, newest first · 3 hidden on the graph · 2 without links");
	assert.equal(markedListLine(1200, 0, 1), "1,200 papers marked in this run, newest first · 1 without links");
});
