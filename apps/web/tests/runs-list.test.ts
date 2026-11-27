/** The sidebar's runs, a page at a time and searched (lib/runs-list.ts): each change of the list, and those refused. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunPage, RunSummary } from "@rp/contracts";
import {
	FIRST,
	RUNS_PAGE,
	matchesSearch,
	offsetAsked,
	pageOf,
	runsListStep,
	shownLine,
	withOpenRun,
	type ListedRun,
	type RunsList,
} from "@/lib/runs-list";

const summary = (index: number, title = `Run ${index}`): RunSummary => ({
	name: `run-${index}`,
	title,
	papers: index,
	communities: 2,
	modularity: 0.4,
});
const listed = (index: number): ListedRun => ({
	name: `run-${index}`,
	title: `Run ${index}`,
	papers: index,
	details: `${index} papers · 2 communities`,
	problem: null,
});
const many = Array.from({ length: 120 }, (_, index) => summary(index + 2));

test("the first page is asked from the start, then a further page from where the rows end", () => {
	assert.equal(offsetAsked(FIRST), 0);
	const shown = runsListStep(FIRST, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2), listed(3)], total: 120 });
	assert.equal(shown.state, "shown");
	assert.equal(offsetAsked(shown), null, "nothing asked while it is shown");
	const more = runsListStep(shown, { kind: "more" });
	assert.equal(more.state, "more");
	assert.equal(offsetAsked(more), 2);
	const longer = runsListStep(more, { kind: "page", asked: more.asked, offset: 2, rows: [listed(4)], total: 120 });
	assert.deepEqual(
		longer.rows.map((row) => row.name),
		["run-2", "run-3", "run-4"],
	);
	assert.equal(shownLine(longer), "Showing 3 of 120 runs");
});

test("a page for another search, or for an offset that is not the next one, changes nothing", () => {
	const searching = runsListStep(FIRST, { kind: "search", search: "gnn" });
	assert.equal(runsListStep(searching, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2)], total: 1 }), searching);
	const shown = runsListStep(searching, { kind: "page", asked: searching.asked, offset: 0, rows: [listed(2)], total: 5 });
	const more = runsListStep(shown, { kind: "more" });
	assert.equal(runsListStep(more, { kind: "page", asked: shown.asked, offset: 0, rows: [listed(9)], total: 5 }), more, "an old first page");
	assert.equal(runsListStep(shown, { kind: "page", asked: shown.asked, offset: 1, rows: [listed(9)], total: 5 }), shown, "nothing asked");
	assert.equal(runsListStep(more, { kind: "more" }), more, "a second click while one page is asked");
});

test("the same search asked again while its first page is on its way takes only the page asked after", () => {
	// the runs changed (one deleted) while the first page was asked: it is asked again, the earlier answer dropped
	const again = runsListStep(FIRST, { kind: "search", search: "" });
	assert.equal(again.search, FIRST.search);
	assert.equal(
		runsListStep(again, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2)], total: 2 }),
		again,
		"from before the change",
	);
	const shown = runsListStep(again, { kind: "page", asked: again.asked, offset: 0, rows: [listed(3)], total: 1 });
	assert.deepEqual(
		shown.rows.map((row) => row.name),
		["run-3"],
	);
});

test("asking for more when every run is shown does nothing", () => {
	const all = runsListStep(FIRST, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2)], total: 1 });
	assert.equal(runsListStep(all, { kind: "more" }), all);
	assert.equal(shownLine(all), null);
});

test("a new search keeps the last rows in view until its first page comes", () => {
	const shown = runsListStep(FIRST, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2)], total: 1 });
	const searching = runsListStep(shown, { kind: "search", search: "vision" });
	assert.equal(searching.state, "loading");
	assert.deepEqual(searching.rows, shown.rows);
	assert.equal(offsetAsked(searching), 0);
});

test("a refusal is kept with the rows shown before, and only for the request waited for", () => {
	const shown = runsListStep(FIRST, { kind: "page", asked: FIRST.asked, offset: 0, rows: [listed(2)], total: 9 });
	const more = runsListStep(shown, { kind: "more" });
	const failed = runsListStep(more, { kind: "refused", asked: more.asked, error: "The engine did not answer within 20 seconds." });
	assert.deepEqual(failed, {
		state: "failed",
		search: "",
		asked: more.asked,
		rows: shown.rows,
		total: 9,
		error: "The engine did not answer within 20 seconds.",
	});
	assert.equal(runsListStep(shown, { kind: "refused", asked: shown.asked, error: "late" }), shown, "nothing was waited for");
	assert.equal(runsListStep(more, { kind: "refused", asked: shown.asked, error: "x" }), more, "an earlier request's");
	// trying again asks the first page anew
	assert.equal(offsetAsked(runsListStep(failed, { kind: "search", search: "" })), 0);
	const refused: RunsList = runsListStep(failed, { kind: "more" });
	assert.equal(refused, failed, "no further page after a failure: try again first");
});

test("the Python backend's whole list is searched and paged here as the engine would", () => {
	const first = pageOf(many, "", 0);
	assert.equal(first.rows.length, RUNS_PAGE);
	assert.equal(first.total, 120);
	assert.equal(pageOf(many, "", RUNS_PAGE).rows[0].name, `run-${RUNS_PAGE + 2}`);
	const titled = [summary(1, "NeurIPS 2025 graph neural networks"), summary(2, "ICML 2025"), summary(3, "Graph theory")];
	assert.deepEqual(
		pageOf(titled, "graph NETWORKS", 0).rows.map((row) => row.name),
		["run-1"],
		"every word, any case",
	);
	assert.ok(matchesSearch({ name: "neurips-2025", title: "Mine" }, "neurips"), "the name counts too");
});

test("the engine's page is taken as it is, its unreadable runs said and its unread ones named", () => {
	const page: RunPage = {
		total: 300,
		offset: 0,
		rows: [
			{
				kind: "run",
				name: "a",
				title: "A",
				papers: 10,
				communities: 3,
				modularity: 0.5,
				created: 1,
				last_opened: null,
				times_opened: 0,
				bytes: 9,
			},
			{ kind: "unreadable", name: "b", reason: "communities.json is not JSON" },
			{ kind: "unread", name: "c" },
		],
	};
	const told = pageOf(page, "", 0);
	assert.equal(told.total, 300);
	assert.deepEqual(told.rows[0], { name: "a", title: "A", papers: 10, details: "10 papers · 3 communities", problem: null });
	assert.equal(told.rows[1].problem, "Its files cannot be read: communities.json is not JSON");
	assert.deepEqual(told.rows[2], { name: "c", title: "c", papers: null, details: null, problem: null });
});

test("the open run stays in sight, first, when it is not among the rows shown", () => {
	const rows = [listed(2), listed(3)];
	assert.deepEqual(withOpenRun(rows, "run-3", undefined), { rows, pinned: null }, "already shown: in its place");
	const pinned = withOpenRun(rows, "run-99", listed(99));
	assert.equal(pinned.pinned, "run-99");
	assert.deepEqual(
		pinned.rows.map((row) => row.name),
		["run-99", "run-2", "run-3"],
	);
	assert.equal(withOpenRun(rows, "run-77", undefined).rows[0].title, "run-77", "known by its name alone");
	assert.deepEqual(withOpenRun(rows, null, undefined), { rows, pinned: null });
});
