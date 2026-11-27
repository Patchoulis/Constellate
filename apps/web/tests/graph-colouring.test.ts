/** Colouring the graph by a feature's groups of papers (lib/graph-colouring.ts), and the groups each feature makes. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtractionRow } from "@rp/contracts";
import { canvasColouring, colouringStep, legendNote, MOST_GROUPS, spreadHues, type PaperColouring } from "@/lib/graph-colouring";
import { columnColouring, NOT_STATED } from "@/lib/extraction";

const table: PaperColouring = {
	owner: "extraction:r:{}",
	variant: "dataset",
	title: "Dataset",
	groups: [{ name: "ImageNet", hue: 200, papers: ["a"] }],
};
const stances: PaperColouring = {
	owner: "stances:r:{}",
	variant: "q1",
	title: "Q",
	groups: [{ name: "Supports", hue: 145, papers: ["b"] }],
};

test("a colouring set replaces the one before, whoever set it", () => {
	assert.equal(colouringStep(null, { kind: "set", colouring: table }), table);
	assert.equal(colouringStep(table, { kind: "set", colouring: stances }), stances);
});

test("only its owner clears a colouring; clearing another's changes nothing", () => {
	assert.equal(colouringStep(stances, { kind: "clear", owner: table.owner }), stances, "the table going away keeps the stances'");
	assert.equal(colouringStep(stances, { kind: "clear", owner: stances.owner }), null);
	assert.equal(colouringStep(stances, { kind: "clear_all" }), null);
	assert.equal(colouringStep(null, { kind: "clear", owner: "anyone" }), null);
});

test("a colouring with no papers in it clears its owner's and leaves another's", () => {
	const empty = { ...stances, groups: [{ name: "cited", hue: 48, papers: [] }] };
	assert.equal(colouringStep(stances, { kind: "set", colouring: empty }), null, "an answer that cites nothing: nothing to colour");
	assert.equal(colouringStep(table, { kind: "set", colouring: empty }), table);
});

test("each paper of the graph is drawn in its first group, the others in none, and the legend counts what is drawn", () => {
	const colouring: PaperColouring = {
		owner: "o",
		title: "t",
		groups: [
			{ name: "first", hue: 10, papers: ["a", "b", "zz"] },
			{ name: "second", hue: null, papers: ["b", "c", "c"] },
		],
	};
	const indexOf = new Map([
		["a", 0],
		["b", 1],
		["c", 2],
		["d", 3],
	]);
	const drawn = canvasColouring(colouring, indexOf, 4);
	assert.deepEqual([...drawn.of], [0, 0, 1, -1], "b is in both: drawn in the first; d is in none");
	assert.deepEqual(drawn.hues, [10, null]);
	assert.deepEqual(drawn.legend, [
		{ name: "first", hue: 10, shown: 2, earlier: 0, elsewhere: 1 },
		{ name: "second", hue: null, shown: 1, earlier: 1, elsewhere: 0 },
	]);
	assert.equal(legendNote(drawn.legend[0]), "1 paper of this group is not in this run.");
	assert.equal(legendNote(drawn.legend[1]), "1 more paper of this group is drawn in the colour of a group above, which it is in too.");
	assert.equal(legendNote({ name: "x", hue: 1, shown: 3, earlier: 0, elsewhere: 0 }), undefined);
});

test("hues for groups without a meaning of their own are all different", () => {
	const hues = spreadHues(MOST_GROUPS);
	assert.equal(new Set(hues.map(Math.round)).size, MOST_GROUPS);
	assert.ok(hues.every((hue) => hue >= 0 && hue < 360));
});

const cell = (text: string): ExtractionRow["cells"][number] => ({ kind: "stated", value: { kind: "text", text }, evidence: [] });
const row = (paper: string, cells: ExtractionRow["cells"]): ExtractionRow =>
	({ paper, title: paper, year: null, acceptance: null, sample: false, text: "pdf", cells }) as unknown as ExtractionRow;

test("a column colours the graph by its most common values, the rest as other values and not stated in grey", () => {
	const values = ["A", "A", "A", "B", "B", "C", "D", "E", "F", "G", "H", "I"];
	const rows = [
		...values.map((value, index) => row(`p${index}`, [cell(value)])),
		row("n", [{ kind: "not_stated" } as ExtractionRow["cells"][number]]),
	];
	const colouring = columnColouring(rows, 0, "Dataset");
	assert.equal(colouring.title, "Dataset (extraction table)");
	assert.ok(colouring.groups.length <= MOST_GROUPS, `${colouring.groups.length} groups`);
	assert.deepEqual(colouring.groups[0], { name: "A", hue: colouring.groups[0].hue, papers: ["p0", "p1", "p2"] }, "the most common first");
	const other = colouring.groups.find((group) => group.name.startsWith("other values"));
	assert.ok(other, "the rarest values together");
	const last = colouring.groups.at(-1)!;
	assert.deepEqual([last.name, last.hue, last.papers], [NOT_STATED, null, ["n"]]);
	const shownPapers = colouring.groups.flatMap((group) => group.papers);
	assert.equal(new Set(shownPapers).size, rows.length, "every paper with a value or not stated is in a group");
	const few = columnColouring([row("x", [cell("A")]), row("y", [cell("B")])], 0, "Dataset");
	assert.deepEqual(
		few.groups.map((group) => group.name),
		["A", "B"],
		"no 'other' and no 'not stated' when there are none",
	);
});
