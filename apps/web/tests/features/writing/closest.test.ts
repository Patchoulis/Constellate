/** A community's closest papers: which are listed, and what is said while they are found or when they cannot be. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ClosestPaper, ClosestPapers, CommunitySection } from "@rp/contracts";
import {
	CLOSEST_NOTE,
	citedIn,
	closeness,
	closestLine,
	noneCloseWords,
	quotesTitle,
	shownPapers,
	statementOf,
} from "@/lib/features/writing/closest";

const paper = (id: string, cosine = 0.5): ClosestPaper => ({
	id,
	title: `Title ${id}`,
	cosine,
	evidence: {
		paper: id,
		quote: "A sentence of the paper",
		established: { kind: "abstract", field: "abstract", start: 0, end: 23, matched: { kind: "exact" } },
	},
});

const status = (more: Partial<ClosestPapers> = {}): ClosestPapers => ({
	state: "done",
	statements: [
		{ statement: "Papers on routers.", section: "Problem", papers: [paper("p1")] },
		{ statement: "They share a method.", section: "Shared methods", papers: [] },
	],
	error: null,
	unavailable: null,
	model: "Qwen/Qwen3-Embedding-0.6B",
	floor: 0.4,
	...more,
});

test("three closest papers are listed until all are asked for, and none the model cited is listed again", () => {
	const papers = ["p1", "p2", "p3", "p4", "p5"].map((id) => paper(id));
	assert.deepEqual(
		shownPapers(papers, new Set(), false).shown.map((found) => found.id),
		["p1", "p2", "p3"],
	);
	assert.equal(shownPapers(papers, new Set(), false).more, 2);
	assert.equal(shownPapers(papers, new Set(), true).shown.length, 5);
	const section = { cites: [{ title: "T", evidence: paper("p2").evidence }] } as Pick<CommunitySection, "cites">;
	const cited = citedIn(section);
	assert.deepEqual(
		shownPapers(papers, cited, true).shown.map((found) => found.id),
		["p1", "p3", "p4", "p5"],
	);
	assert.equal(citedIn(null).size, 0);
});

test("closeness is said with its unit, and a statement nothing is close to says the floor", () => {
	assert.equal(closeness(paper("p1", 0.714)), "0.71 cosine similarity");
	assert.equal(noneCloseWords(0.4), "No paper of this community is close to this (none reaches 0.40 cosine similarity).");
	assert.match(CLOSEST_NOTE, /closeness of meaning, not a check that the paper says this/);
});

test("a section's statement is found by its heading", () => {
	assert.equal(statementOf(status(), "Problem")?.papers[0].id, "p1");
	assert.equal(statementOf(status(), "What sets it apart"), null);
	assert.equal(statementOf(null, "Problem"), null);
});

test("the line says what is being done, why nothing can be, and nothing once they are shown", () => {
	const idle = { state: "idle" };
	assert.equal(closestLine(status(), idle, false, null), null);
	assert.deepEqual(closestLine(null, idle, false, null), { text: "Finding the closest papers…", tone: "quiet" });
	const missing = status({
		state: "idle",
		statements: null,
		unavailable: "The embedding model is not on this computer: it is downloaded the first time a run is made here.",
	});
	const said = closestLine(missing, idle, false, null);
	assert.equal(said?.tone, "quiet");
	assert.match(said!.text, /the embedding model is not on this computer/i);
	assert.match(closestLine(status({ state: "idle", statements: null }), idle, true, null)!.text, /once what is being done now ends/);
	assert.deepEqual(closestLine(status({ state: "idle", statements: null }), { state: "failed", error: "no vectors" }, false, null), {
		text: "The closest papers could not be found: no vectors",
		tone: "problem",
	});
	assert.equal(closestLine(null, idle, false, "the engine did not answer")?.tone, "problem");
});

test("words that are the paper's title are not shown again under it", () => {
	assert.equal(quotesTitle(paper("p1")), false);
	const titled = paper("p1");
	titled.evidence = { ...titled.evidence, established: { kind: "abstract", field: "title", start: 0, end: 8, matched: { kind: "exact" } } };
	assert.equal(quotesTitle(titled), true);
});
