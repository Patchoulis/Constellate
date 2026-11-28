/** The citation network as it is drawn and listed, and the communities' citation figures. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { CitationNeighbour, CitationNetwork, Community } from "@rp/contracts";
import {
	authorLine,
	citedByFigure,
	citedByOf,
	citationsNote,
	citedByLine,
	citesFigure,
	citesOf,
	confidenceNote,
	layoutNetwork,
	linkTag,
	outsideLink,
	showing,
	shownWorks,
	workRadius,
} from "@/lib/reader/citations";

function work(key: string, year: number | null, citedBy: number, rest: Partial<CitationNeighbour> = {}): CitationNeighbour {
	return {
		key,
		title: `Work ${key}`,
		authors: ["Ada Lovelace", "Alan Turing"],
		authors_total: 2,
		year,
		venue: "NeurIPS",
		link: { kind: "in_library", paper: key },
		url: null,
		confidence: "identified",
		cited_by: citedBy,
		entry: null,
		...rest,
	};
}

function network(references: CitationNeighbour[], citations: CitationNeighbour[], edges: CitationNetwork["edges"] = []): CitationNetwork {
	return {
		seed: "seed",
		seed_state: "read",
		references,
		citations,
		edges,
		similar: [],
		totals: {
			references: references.length,
			references_in_library: references.length,
			references_on_arxiv: 0,
			references_unresolved: 0,
			citations: citations.length,
			citations_shown: citations.length,
			citations_in_run: null,
		},
		papers_read: 3,
		papers_in_library: 10,
		built: "2026-11-26",
	};
}

const FRAME = { width: 600, height: 300 };

test("prior work is left of the seed and derivative work right of it, older further out", () => {
	const picture = layoutNetwork(network([work("old", 2010, 1), work("recent", 2020, 0)], [work("later", 2024, 0)]), 2022, FRAME);
	const at = (key: string) => picture.works.find((placed) => placed.key === key)!;
	const seed = picture.works[0];
	assert.equal(seed.side, "seed");
	assert.deepEqual([seed.x, seed.y], [300, 150]);
	assert.ok(at("old").x < at("recent").x && at("recent").x < seed.x && seed.x < at("later").x);
	assert.equal(at("later").side, "derivative");
	assert.deepEqual(
		picture.ticks.map((tick) => tick.year),
		[2010, 2016, 2022, 2023, 2024],
	);
});

test("the same network always gives the same picture, whatever order its works came in", () => {
	const works = [work("a", 2019, 4), work("b", 2019, 9), work("c", 2019, 1), work("d", null, 0)];
	const one = layoutNetwork(network(works, []), 2021, FRAME);
	const other = layoutNetwork(network([...works].reverse(), []), 2021, FRAME);
	const positions = (picture: typeof one) =>
		picture.works.map((placed) => [placed.key, placed.x, placed.y]).sort((x, y) => String(x[0]).localeCompare(String(y[0])));
	assert.deepEqual(positions(one), positions(other));
	// within a year, the most cited is on the middle line; a work with no year goes to the far edge
	const at = (key: string) => one.works.find((placed) => placed.key === key)!;
	assert.equal(at("b").y, 150);
	assert.equal(at("d").x, 26);
	assert.equal(one.ticks[0].year, null);
});

test("a crowded year stays inside the frame and clear of the seed", () => {
	const crowd = Array.from({ length: 60 }, (_, at) => work(`w${at}`, 2022, at === 0 ? 40 : 0));
	const picture = layoutNetwork(network([], crowd), 2022, FRAME);
	for (const placed of picture.works.slice(1)) {
		assert.ok(placed.x >= 300 + 22 && placed.x <= 600 - 26, `x ${placed.x}`);
		assert.ok(Number.isFinite(placed.y));
	}
});

test("a work's size grows with how often it is cited, up to a cap", () => {
	assert.ok(workRadius(0) < workRadius(4) && workRadius(4) < workRadius(25));
	assert.equal(workRadius(10_000), workRadius(100_000));
});

test("lines join the seed to every neighbour and neighbours to the works they cite", () => {
	const picture = layoutNetwork(
		network(
			[work("a", 2019, 1)],
			[work("b", 2024, 0)],
			[
				{ from: "b", to: "a" },
				{ from: "b", to: "elsewhere" },
			],
		),
		2021,
		FRAME,
	);
	assert.equal(picture.lines.filter((line) => line.seed).length, 2);
	const between = picture.lines.filter((line) => !line.seed);
	assert.deepEqual(
		between.map((line) => [picture.works[line.from].key, picture.works[line.to].key]),
		[["b", "a"]],
	);
});

test("lists are filtered by every word and ordered by citations or by year", () => {
	const works = [
		work("a", 2019, 4, { title: "Sparse attention" }),
		work("b", 2023, 1, { title: "Dense attention" }),
		work("c", null, 9, { title: "Sparse codes" }),
	];
	assert.deepEqual(
		shownWorks(works, "cited", "").map((shown) => shown.key),
		["c", "a", "b"],
	);
	assert.deepEqual(
		shownWorks(works, "year", "").map((shown) => shown.key),
		["b", "a", "c"],
	);
	assert.deepEqual(
		shownWorks(works, "cited", "SPARSE attention").map((shown) => shown.key),
		["a"],
	);
	assert.deepEqual(
		shownWorks(works, "year", "2023").map((shown) => shown.key),
		["b"],
	);
});

test("counts say what they count", () => {
	assert.equal(showing(200, 1234, "citing paper"), "Showing 200 of 1,234 citing papers");
	assert.equal(authorLine(work("a", 2020, 0, { authors_total: 7 })), "Ada Lovelace, Alan Turing and 5 more");
	assert.equal(outsideLink(work("a", 2020, 0, { url: "https://arxiv.org/abs/2401.00001" })), "https://arxiv.org/abs/2401.00001");
	assert.equal(
		outsideLink(work("a", 2020, 0, { title: "Deep learning & more", url: null })),
		"https://scholar.google.com/scholar?q=Deep%20learning%20%26%20more",
	);
});

function community(citations: Community["citations"]): Community {
	return {
		id: 0,
		tree_path: "c0",
		size: 739,
		name: null,
		description: null,
		keywords: [],
		mix: {},
		mean_inner_similarity: 0.3,
		similarity_to_mean: 0.5,
		closest_communities: [],
		nearest_to_centroid: [],
		acceptance: {},
		ai_flagged: 0,
		members: [],
		children: [],
		citations,
	};
}

test("a community's citation figures say how much of it was read, and nothing read is not zero", () => {
	const read = community({ papers_read: 212, cited_by: 1204, cited_by_inside: 300, cites: 3410, cites_in_library: 120 });
	assert.equal(citedByFigure(read), "1,204 citations · 212 of 739 papers read");
	assert.equal(citesFigure(read), "3,410 works cited · 212 of 739 papers read");
	assert.equal(citedByOf(read), 1204);
	for (const unread of [
		community(undefined),
		community(null),
		community({ papers_read: 0, cited_by: 4, cited_by_inside: 0, cites: 0, cites_in_library: 0 }),
	]) {
		assert.equal(citedByFigure(unread), "not read yet");
		assert.equal(citesOf(unread), null);
	}
});

test("a work's link says where clicking it goes: the reader, its arXiv page by its id, or a search for its title", () => {
	assert.equal(linkTag({ kind: "in_library", paper: "2401.00001" }).label, "In your library");
	assert.match(linkTag({ kind: "in_library", paper: "2401.00001" }).note, /open it in the reader/);
	const arxiv = linkTag({ kind: "on_arxiv", arxiv: "2312.04567" });
	assert.equal(arxiv.label, "On arXiv");
	assert.equal(arxiv.note, "Not in your library, but on arXiv as 2312.04567: click it to open its arXiv page in the browser.");
	const unresolved = linkTag({ kind: "unresolved" });
	assert.equal(unresolved.label, "Not found");
	assert.match(unresolved.note, /searches for its title in the browser/);
});

test("a reference read only partly says so, and one identified says nothing", () => {
	assert.equal(confidenceNote("identified"), null);
	assert.match(confidenceNote("title_parsed") ?? "", /read from the paper's reference list by a program/);
	assert.match(confidenceNote("raw_only") ?? "", /^Only the entry as written could be read/);
});

test("how often a work is cited says of which papers, one in the singular", () => {
	assert.equal(citedByLine(1), "cited by 1 paper Constellate has read");
	assert.equal(citedByLine(1204), "cited by 1,204 papers Constellate has read");
	assert.equal(citedByLine(0), "cited by 0 papers Constellate has read");
});

test("a community's citation note says how many citations come from inside it and how many cited works are kept", () => {
	const read = community({ papers_read: 212, cited_by: 1204, cited_by_inside: 300, cites: 3410, cites_in_library: 1 });
	assert.equal(
		citationsNote(read),
		"300 citations of the 1,204 citations come from its own papers. 1 work of the 3,410 works it cites are in your library.",
	);
	const unread = community({ papers_read: 0, cited_by: 4, cited_by_inside: 0, cites: 0, cites_in_library: 0 });
	assert.equal(citationsNote(unread), "None of its papers' reference lists has been read yet.");
	assert.equal(citationsNote(community(null)), "None of its papers' reference lists has been read yet.");
});
