/** The citation network as it is drawn and listed, and the communities' citation figures. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { CitationNeighbour, CitationNetwork, Community, CountedCitations, RunCitations } from "@rp/contracts";
import {
	authorLine,
	CITATIONS_NOTE,
	CITED_BY_TITLE,
	citationsLine,
	citedByFigure,
	citedByOf,
	citedBySplit,
	citationsNote,
	citedByLine,
	citesFigure,
	citesOf,
	citesSplit,
	confidenceNote,
	layoutNetwork,
	linkTag,
	networkSourceLine,
	NOT_COUNTABLE,
	outsideLink,
	referencesLine,
	runCitationsLine,
	runTag,
	showing,
	shownWorks,
	withCounted,
	workPlace,
	workRadius,
} from "@/lib/reader/citations";
import { communityTree, nodeAt } from "@/lib/community/community";

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

/** Figures counted with their splits: 1,204 citations (300 + 500 + 404), 3,410 works cited (120 + 57 + 1,000 + 2,233). */
const COUNTED: CountedCitations = {
	papers_read: 212,
	cited_by: 1204,
	cited_by_inside: 300,
	cited_by_rest_of_run: 500,
	cited_by_outside_run: 404,
	cites: 3410,
	cites_in_library: 177,
	cites_in_run: 120,
	cites_outside_run: 57,
	cites_on_arxiv: 1000,
	cites_unresolved: 2233,
};

/** As runs saved them before the splits were counted. */
const SAVED_BEFORE = { papers_read: 212, cited_by: 1204, cited_by_inside: 300, cites: 3410, cites_in_library: 120 };

test("a community's citation figures say what they count, and what was not counted is not zero", () => {
	const read = community(COUNTED);
	assert.equal(citedByFigure(read), "1,204 citations");
	assert.equal(citesFigure(read), "3,410 works cited · 212 of 739 papers read");
	assert.equal(citedByOf(read), 1204);
	assert.equal(citesOf(read), 3410);
	for (const unread of [community(undefined), community(null), community(SAVED_BEFORE)]) {
		assert.equal(citedByFigure(unread), "not counted yet");
		assert.equal(citesFigure(unread), "not counted yet");
		assert.equal(citedByOf(unread), null, "sorted last");
		assert.equal(citesOf(unread), null);
	}
	// cited from the library though none of its own papers is read: what it cites is not known, who cites it is
	const unreadOwn = community({
		...COUNTED,
		papers_read: 0,
		cites: 0,
		cites_in_library: 0,
		cites_in_run: 0,
		cites_outside_run: 0,
		cites_on_arxiv: 0,
		cites_unresolved: 0,
	});
	assert.equal(citedByOf(unreadOwn), 1204);
	assert.equal(citesOf(unreadOwn), null);
	assert.equal(citesFigure(unreadOwn), "none of its papers read yet");
	assert.equal(citesSplit(unreadOwn), null);
});

test("the splits of a community's figures add up to their totals, each said with what it is", () => {
	const read = community(COUNTED);
	const citedBy = citedBySplit(read)!;
	assert.deepEqual(
		citedBy.map((part) => part.label),
		["from its own papers", "from the rest of the run", "from your library outside the run"],
	);
	assert.equal(
		citedBy.reduce((sum, part) => sum + part.value, 0),
		COUNTED.cited_by,
	);
	const cites = citesSplit(read)!;
	assert.deepEqual(
		cites.map((part) => [part.label, part.value]),
		[
			["in the run", 120],
			["in your library outside the run", 57],
			["not in your library", 3233],
		],
	);
	assert.equal(cites[2].detail, "1,000 on arXiv, 2,233 not found");
	assert.equal(
		cites.reduce((sum, part) => sum + part.value, 0),
		COUNTED.cites,
	);
	assert.equal(citedBySplit(community(SAVED_BEFORE)), null);
});

test("figures counted now take the place of those saved with the run, by tree path, sub-communities too", () => {
	const child = { ...community(SAVED_BEFORE), tree_path: "c0.1" };
	const saved = [{ ...community(SAVED_BEFORE), children: [child] }];
	const counted: RunCitations = {
		...COUNTED,
		papers: 739,
		library_papers: 5000,
		library_papers_read: 900,
		communities: [
			{ ...COUNTED, tree_path: "c0", papers: 739, children: [{ ...COUNTED, cited_by: 7, tree_path: "c0.1", papers: 2, children: [] }] },
		],
	};
	const [root] = withCounted(saved, counted);
	assert.equal(citedByOf(root), 1204);
	assert.equal(citedByOf(root.children[0]), 7);
	assert.equal(root.citations && "tree_path" in root.citations, false, "only the figures");
	assert.equal(withCounted(saved, null), saved, "until counted, the saved ones");
	// a community selected before the figures came is found again in the tree made with them
	const before = nodeAt(communityTree(saved), "c0.1")!;
	const after = nodeAt(communityTree(withCounted(saved, counted)), before.community.tree_path)!;
	assert.equal(citedByOf(after.community), 7);
	assert.equal(nodeAt(communityTree(saved), "c9"), null);
	assert.equal(citedByFigure(withCounted(saved, null)[0]), "not counted yet");
});

test("the run's own line says where its citations come from and what its papers cite", () => {
	const run: RunCitations = {
		...COUNTED,
		cited_by: 4321,
		cited_by_inside: 1000,
		cited_by_rest_of_run: 0,
		cited_by_outside_run: 3321,
		papers: 739,
		library_papers: 5000,
		library_papers_read: 900,
		communities: [],
	};
	assert.equal(
		runCitationsLine(run),
		"The run's papers are cited 4,321 times: 1,000 by papers of the run and 3,321 by papers of your library outside it. " +
			"Works cited by its 212 papers read: 3,410 (120 in the run, 57 in your library outside it, 3,233 not in your library).",
	);
	assert.match(runCitationsLine({ ...run, papers_read: 0 }), /None of its papers' reference lists has been read yet\.$/);
});

test("the notes say the figures are of your library, and what cannot be counted", () => {
	assert.match(CITATIONS_NOTE, /reference lists Constellate has read \(the papers of your library whose PDFs it has read\)/);
	assert.doesNotMatch(CITATIONS_NOTE, /of the run/);
	assert.equal(NOT_COUNTABLE, "Not counted: papers that are not in your library.");
	assert.match(CITED_BY_TITLE, /in the run or not/);
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

test("a community's citation note says where its citations come from and what the works it cites are", () => {
	assert.equal(
		citationsNote(community(COUNTED)),
		"Of its 1,204 citations, 300 from its own papers, 500 from the rest of the run and 404 from your library outside the run. " +
			"Of the 3,410 works it cites, 120 in the run, 57 in your library outside the run and 3,233 not in your library (1,000 on arXiv, 2,233 not found).",
	);
	const unread = community({ ...COUNTED, papers_read: 0 });
	assert.match(citationsNote(unread), /None of its papers' reference lists has been read yet, so what it cites is not known\.$/);
	for (const before of [community(null), community(SAVED_BEFORE)]) assert.match(citationsNote(before), /^Not counted yet: /);
});

function outsider(rest: Partial<CitationNeighbour> = {}): CitationNeighbour {
	return work("paper:p9", 2024, 2, { link: { kind: "in_library", paper: "p9" }, in_run: false, ...rest });
}

test("a paper of the library outside the run is told apart, in the lists and in the picture", () => {
	assert.equal(workPlace(outsider()), "outside_run");
	assert.equal(workPlace(outsider({ in_run: true })), "in_library");
	assert.equal(workPlace(outsider({ in_run: null })), "in_library", "no run asked about");
	assert.equal(workPlace(outsider({ in_run: undefined })), "in_library", "an older engine");
	assert.equal(workPlace(work("arxiv:1", 2020, 0, { link: { kind: "on_arxiv", arxiv: "1" } })), "on_arxiv");
	assert.equal(runTag(outsider())?.label, "Outside this run");
	assert.match(runTag(outsider())!.note, /open it in the reader/);
	assert.equal(runTag(outsider({ in_run: true })), null);
});

test("a network's lines split what it cites and who cites it by the run", () => {
	const base = network([], []).totals;
	const totals = {
		...base,
		references: 12,
		references_in_library: 5,
		references_in_run: 3,
		references_outside_run: 2,
		references_on_arxiv: 4,
		references_unresolved: 3,
		citations: 5,
		citations_shown: 5,
		citations_in_run: 2,
		citations_outside_run: 3,
	};
	assert.equal(
		referencesLine(totals, 12),
		"Showing 12 of 12 references: 3 in this run, 2 in your library outside this run, 4 on arXiv, 3 not found",
	);
	assert.equal(citationsLine(totals, 5), "Showing 5 of 5 citing papers Constellate has read in your library: 2 in this run, 3 outside it");
	// no run asked about, or an older engine: as before
	const plain = { ...totals, references_in_run: null, references_outside_run: null, citations_in_run: null, citations_outside_run: null };
	assert.equal(referencesLine(plain, 12), "Showing 12 of 12 references: 5 in your library, 4 on arXiv, 3 not found");
	assert.equal(
		citationsLine({ ...plain, citations: 300, citations_shown: 200 }, 200),
		"Showing 200 of 300 citing papers Constellate has read in your library, the most cited",
	);
	const seen = { ...network([], []), totals };
	assert.equal(
		networkSourceLine(seen),
		"From the reference lists Constellate has read of your library: 3 of 10 papers. Built 2026-11-26. Cited by 2 papers of this run and 3 of your library outside it.",
	);
	assert.equal(
		networkSourceLine({ ...seen, totals: plain }),
		"From the reference lists Constellate has read of your library: 3 of 10 papers. Built 2026-11-26.",
	);
});
