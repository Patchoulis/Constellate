/** The extraction table's logic: cells in words, sorting, filtering, coverage, CSV, groups for the graph. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Evidence, ExtractionCell, ExtractionRow, ExtractionTable } from "@rp/contracts";
import {
	cellText,
	columnId,
	coverage,
	filterOptions,
	filterRows,
	LEFT_OUT,
	leftOutNote,
	moved,
	NOT_STATED,
	paperGroups,
	sortRows,
	toCsv,
} from "@/lib/extraction";

const evidence = (paper: string, quote: string, page = 3): Evidence => ({
	paper,
	quote,
	established: {
		kind: "verified",
		address: { page, section: "4. Results", paragraph: 2, start: 10, end_page: page, end: 60 },
		matched: { kind: "exact" },
	},
});

const number = (paper: string, value: number): ExtractionCell => ({
	kind: "stated",
	value: { kind: "number", written: String(value), value, unit: "% accuracy", context: "ImageNet" },
	evidence: [evidence(paper, `we reach ${value}% accuracy on ImageNet`)],
});
const list = (paper: string, items: string[]): ExtractionCell => ({
	kind: "stated",
	value: { kind: "list", items },
	evidence: [evidence(paper, `we evaluate on ${items.join(" and ")}`)],
});
const notStated: ExtractionCell = { kind: "not_stated" };
const notAsked: ExtractionCell = { kind: "not_asked" };
const leftOut: ExtractionCell = { kind: "not_verified", why: "quotes_not_found" };

const row = (paper: string, cells: ExtractionCell[]): ExtractionRow => ({
	paper,
	title: `Title of ${paper}`,
	year: 2026,
	acceptance: "poster",
	acceptance_term: null,
	text: "pdf",
	sample: false,
	cells,
});

const rows = [
	row("a", [number("a", 84.3), list("a", ["ImageNet", "CIFAR-10"])]),
	row("b", [notStated, list("b", ["imagenet"])]),
	row("c", [number("c", 9.5), leftOut]),
	row("d", [notAsked, notStated]),
	row("e", [number("e", 120), list("e", ["COCO"])]),
];

const table: ExtractionTable = {
	columns: [
		{ id: "headline", label: "Headline result", question: "What is the headline result?", kind: "number" },
		{ id: "datasets", label: "Datasets", question: "Which datasets?", kind: "list" },
	],
	provider: "anthropic",
	model: "claude-opus-5-5",
	rows,
	sample: 5,
	left_out: { quotes_not_found: 1, value_not_in_quote: 2, malformed: 0 },
};

test("a cell says what it is: a value, not stated, or left out, never a value without its evidence", () => {
	assert.equal(cellText(rows[0].cells[0]), "84.3 % accuracy (ImageNet)");
	assert.equal(cellText(rows[0].cells[1]), "ImageNet, CIFAR-10");
	assert.equal(cellText(notStated), NOT_STATED);
	assert.equal(cellText(leftOut), LEFT_OUT);
	assert.match(cellText({ kind: "not_verified", why: "number_not_in_quote" }), /^not verified: left out/);
	assert.equal(cellText({ kind: "stated", value: { kind: "yes_no", answer: "no" }, evidence: [] }), "no");
});

test("numbers sort by value, text alphabetically, and not stated comes last either way", () => {
	const ids = (sorted: ExtractionRow[]) => sorted.map((one) => one.paper);
	assert.deepEqual(ids(sortRows(rows, 0)), ["c", "a", "e", "b", "d"], "9.5 < 84.3 < 120, then not stated, then not asked");
	assert.deepEqual(ids(sortRows(rows, 0, true)), ["e", "a", "c", "b", "d"]);
	assert.deepEqual(ids(sortRows(rows, 1)), ["e", "b", "a", "d", "c"], "COCO, imagenet, ImageNet and CIFAR-10, then not stated, left out");
	assert.deepEqual(ids(rows), ["a", "b", "c", "d", "e"], "the rows themselves are left as they were");
});

test("a column filters by stating it, by not stating it, or by a value, whatever its case", () => {
	const options = filterOptions(rows, 1);
	assert.deepEqual(
		options.map((option) => [option.key, option.papers]),
		[
			["stated", 3],
			["not_stated", 1],
			["value:imagenet", 2],
			["value:cifar-10", 1],
			["value:coco", 1],
		],
	);
	assert.equal(options[2].label, "ImageNet");
	assert.deepEqual(
		filterRows(rows, 1, "value:imagenet").map((one) => one.paper),
		["a", "b"],
	);
	assert.deepEqual(
		filterRows(rows, 0, "not_stated").map((one) => one.paper),
		["b"],
	);
	assert.deepEqual(
		filterOptions(rows, 0).map((option) => option.key),
		["stated", "not_stated"],
		"numbers filter only by being stated",
	);
});

test("coverage counts the papers that state a column, of those asked, with its unit", () => {
	assert.equal(coverage(rows, 0, "Headline result"), "3 of 4 papers state headline result (1 paper not asked yet)");
	assert.equal(coverage(rows, 1, "Datasets"), "3 of 5 papers state datasets");
	// nothing asked yet: not "0 of 0 papers state datasets"
	const unasked = rows.map((row) => ({ ...row, cells: row.cells.map(() => ({ kind: "not_asked" as const })) }));
	assert.equal(coverage(unasked, 1, "Datasets"), `No paper asked about datasets yet (${unasked.length} papers not asked)`);
});

test("the CSV has every cell's value, quotes and pages, and nothing a spreadsheet would run", () => {
	const tricky = row("f", [
		{ kind: "stated", value: { kind: "text", text: '=HYPERLINK("x")' }, evidence: [evidence("f", 'a "quoted", line', 7)] },
		notStated,
	]);
	const csv = toCsv(table, [rows[0], tricky]);
	const lines = csv.trimEnd().split("\r\n");
	assert.equal(
		lines[0],
		"paper,title,year,acceptance,read from,values read by the model," +
			"Headline result (the model's reading),Headline result: quotes (the paper's words),Headline result: pages," +
			"Datasets (the model's reading),Datasets: quotes (the paper's words),Datasets: pages",
	);
	assert.equal(
		lines[1],
		'a,Title of a,2026,poster,PDF,claude-opus-5-5,84.3 % accuracy (ImageNet),we reach 84.3% accuracy on ImageNet,p. 3,"ImageNet, CIFAR-10",we evaluate on ImageNet and CIFAR-10,p. 3',
	);
	assert.ok(lines[2].includes(`"'=HYPERLINK(""x"")"`), lines[2]);
	assert.ok(lines[2].includes(`"a ""quoted"", line",p. 7`), lines[2]);
	assert.ok(lines[2].endsWith(`${NOT_STATED},,`));
	const formula = row("g", [{ kind: "stated", value: { kind: "text", text: "-1+SUM(A1)" }, evidence: [] }, number("g", -3)]);
	const guarded = toCsv(table, [formula]).split("\r\n")[1];
	assert.ok(guarded.includes(",'-1+SUM(A1),"), guarded);
});

test("the graph can be coloured by a column's values: the papers of each", () => {
	const groups = paperGroups(rows, 1);
	assert.deepEqual(groups.get("ImageNet"), ["a"]);
	assert.deepEqual(groups.get("imagenet"), ["b"]);
	assert.deepEqual(groups.get(NOT_STATED), ["d"]);
	assert.equal([...groups.values()].flat().includes("c"), false, "a cell left out is in no group");
});

test("what was left out is said with its count and why", () => {
	assert.equal(
		leftOutNote(table),
		"1 cell left out: their quotes were not found in the paper; 2 values left out: not in the quotes given for them.",
	);
	assert.equal(leftOutNote({ ...table, left_out: { quotes_not_found: 0, value_not_in_quote: 0, malformed: 0 } }), null);
});

test("a new column gets an id of its own, and columns move within the list", () => {
	assert.equal(columnId("Out-of-distribution data?", []), "out-of-distribution-data");
	assert.equal(columnId("Task", ["task", "task-2"]), "task-3");
	assert.equal(columnId("???", []), "column");
	assert.deepEqual(moved(["a", "b", "c"], 0, 1), ["b", "a", "c"]);
	assert.deepEqual(moved(["a", "b", "c"], 0, -1), ["a", "b", "c"]);
	assert.deepEqual(moved(["a", "b", "c"], 2, 5), ["a", "b", "c"]);
});
