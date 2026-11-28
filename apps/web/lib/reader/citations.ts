/**
 * The citation network of one paper, as it is drawn and listed: where each work goes, how the lists are ordered
 * and filtered, and what the figures say. Pure functions, so the same network always gives the same picture.
 *
 * The picture reads left to right as time: the seed paper in the middle, the works it cites (prior work) on the
 * left by year, the papers that cite it (derivative work) on the right by year. A work's size says how many papers
 * Constellate has read cite it; within a year, the most cited sit nearest the middle line.
 */
import type {
	CitationLink,
	CitationNeighbour,
	CitationNetwork,
	Community,
	CountedCitations,
	CountedCommunity,
	ReferenceConfidence,
	RunCitations,
} from "@rp/contracts";
import { safeLink } from "@/lib/net/links";
import { count } from "@/lib/units";

export type Side = "seed" | "prior" | "derivative";

export interface PlacedWork {
	key: string;
	x: number;
	y: number;
	r: number;
	side: Side;
	/** null for the seed */
	work: CitationNeighbour | null;
}

export interface NetworkPicture {
	works: PlacedWork[];
	/** indices into `works`; `seed` lines join the seed to a neighbour, the others two neighbours */
	lines: { from: number; to: number; seed: boolean }[];
	/** where the years are along the bottom; `year` null is the column of works whose year is unknown */
	ticks: { year: number | null; x: number }[];
}

const MARGIN = 26; // px kept free at the frame's edges
const GAP = 52; // px between the seed and the nearest column of works
const UNKNOWN = 34; // px given to the column of works with no year, at the far edge
const SEED_R = 11;
const MIN_R = 3;
const MAX_R = 13;
const PAD = 2; // px between two works of a column

/** A work's radius by how many read papers cite it: area grows with the count, up to a cap. */
export function workRadius(citedBy: number): number {
	return Math.min(MAX_R, MIN_R + 2.2 * Math.sqrt(Math.max(0, citedBy)));
}

/** Most cited first, then the newer, then by title: one order, whatever order the works came in. */
const byWeight = (a: CitationNeighbour, b: CitationNeighbour) =>
	b.cited_by - a.cited_by || (b.year ?? 0) - (a.year ?? 0) || a.title.localeCompare(b.title) || a.key.localeCompare(b.key);

/** Positions for one side's works: x by year between `inner` (the seed's year) and `outer`, y spread from the middle. */
function placeSide(
	works: CitationNeighbour[],
	side: "prior" | "derivative",
	seedYear: number,
	frame: { width: number; height: number },
): { placed: PlacedWork[]; ticks: NetworkPicture["ticks"] } {
	const centre = frame.width / 2;
	const toward = side === "prior" ? -1 : 1; // the direction away from the seed
	const inner = centre + toward * GAP;
	const known = works.filter((work) => work.year !== null);
	const unknown = works.length > known.length;
	const edge = side === "prior" ? MARGIN : frame.width - MARGIN;
	const outer = unknown ? edge - toward * UNKNOWN : edge;
	// prior work is not later than the seed, derivative work not earlier: a preprint's year can say otherwise
	const clamp = (year: number) => (side === "prior" ? Math.min(year, seedYear) : Math.max(year, seedYear));
	const years = known.map((work) => clamp(work.year!));
	const far = years.length ? (side === "prior" ? Math.min(...years) : Math.max(...years)) : seedYear;
	const xOf = (year: number | null) =>
		year === null
			? edge
			: far === seedYear
				? inner
				: inner + ((outer - inner) * Math.abs(clamp(year) - seedYear)) / Math.abs(far - seedYear);

	const columns = new Map<number, CitationNeighbour[]>();
	for (const work of works) {
		const x = xOf(work.year);
		columns.set(x, [...(columns.get(x) ?? []), work]);
	}
	const middle = frame.height / 2;
	const room = frame.height - 2 * MARGIN;
	const placed: PlacedWork[] = [];
	// the band this side's works stay in: inside the frame, and clear of the seed
	const [lowest, highest] = side === "prior" ? [MARGIN, centre - 2 * SEED_R] : [centre + 2 * SEED_R, frame.width - MARGIN];
	for (const [x, column] of columns) {
		column.sort(byWeight);
		const step = 2 * Math.max(...column.map((work) => workRadius(work.cited_by))) + PAD;
		const fit = Math.max(1, Math.floor(room / step));
		column.forEach((work, index) => {
			// a column taller than the frame wraps into others beside it, one on each side in turn
			const row = index % fit;
			const wrap = Math.floor(index / fit);
			const offset = Math.ceil(row / 2) * step * (row % 2 ? -1 : 1);
			const shift = Math.ceil(wrap / 2) * (wrap % 2 ? 1 : -1) * step * 0.6;
			const at = Math.min(highest, Math.max(lowest, x + toward * shift));
			placed.push({ key: work.key, x: at, y: middle + offset, r: workRadius(work.cited_by), side, work });
		});
	}
	const ticks: NetworkPicture["ticks"] = [];
	if (years.length) {
		const mid = Math.round((far + seedYear) / 2);
		for (const year of new Set([far, mid])) if (year !== seedYear) ticks.push({ year, x: xOf(year) });
	}
	if (unknown) ticks.push({ year: null, x: edge });
	return { placed, ticks };
}

/**
 * The picture of `network` in a frame of `frame` pixels. `seedYear` is the seed paper's year (when it was
 * published); without it, the middle of its neighbours' years stands in.
 */
export function layoutNetwork(network: CitationNetwork, seedYear: number | null, frame: { width: number; height: number }): NetworkPicture {
	const years = [...network.references, ...network.citations].map((work) => work.year).filter((year): year is number => year !== null);
	const year = seedYear ?? (years.length ? years.sort((a, b) => a - b)[Math.floor(years.length / 2)] : 2000);
	const seed: PlacedWork = { key: `paper:${network.seed}`, x: frame.width / 2, y: frame.height / 2, r: SEED_R, side: "seed", work: null };
	const prior = placeSide(network.references, "prior", year, frame);
	const derivative = placeSide(network.citations, "derivative", year, frame);
	// a work both cited by the seed and citing it (a preprint and its later version cite each other) is drawn
	// once, as prior work
	const works = [seed];
	const index = new Map<string, number>([[seed.key, 0]]);
	for (const work of [...prior.placed, ...derivative.placed]) {
		if (index.has(work.key)) continue;
		index.set(work.key, works.length);
		works.push(work);
	}
	const lines: NetworkPicture["lines"] = works.slice(1).map((_, at) => ({ from: 0, to: at + 1, seed: true }));
	for (const edge of network.edges) {
		const from = index.get(edge.from);
		const to = index.get(edge.to);
		if (from !== undefined && to !== undefined && from !== to) lines.push({ from, to, seed: false });
	}
	const ticks = [...prior.ticks, { year, x: frame.width / 2 }, ...derivative.ticks].sort((a, b) => a.x - b.x);
	return { works, lines, ticks };
}

export type WorkOrder = "cited" | "year";

/**
 * The works whose title, authors or venue have every word of `filter`, ordered by `order`, highest (or newest)
 * first; works with no year go last when ordered by year.
 */
export function shownWorks(works: CitationNeighbour[], order: WorkOrder, filter: string): CitationNeighbour[] {
	const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
	const text = (work: CitationNeighbour) =>
		[work.title, ...work.authors, work.venue ?? "", String(work.year ?? "")].join(" ").toLowerCase();
	const matching = words.length ? works.filter((work) => words.every((word) => text(work).includes(word))) : [...works];
	const byYear = (a: CitationNeighbour, b: CitationNeighbour) => (b.year ?? -Infinity) - (a.year ?? -Infinity) || byWeight(a, b);
	return matching.sort(order === "year" ? byYear : byWeight);
}

/** "Showing 200 of 1,234 citing papers". */
export function showing(shown: number, total: number, one: string, many = `${one}s`): string {
	return `Showing ${shown.toLocaleString()} of ${count(total, one, many)}`;
}

/** "Ada Lovelace, Alan Turing, Grace Hopper and 4 more". */
export function authorLine(work: CitationNeighbour): string {
	const more = work.authors_total - work.authors.length;
	return more > 0 ? `${work.authors.join(", ")} and ${more} more` : work.authors.join(", ");
}

/** Where a work that is not in the library is read: its own address when it is https (it was read from a PDF), else
 *  a search for its title. */
export function outsideLink(work: CitationNeighbour): string {
	return safeLink(work.url)?.href ?? `https://scholar.google.com/scholar?q=${encodeURIComponent(work.title)}`;
}

/** What a work was found to be, as a tag and what the tag means. */
export function linkTag(link: CitationLink): { label: string; note: string } {
	switch (link.kind) {
		case "in_library":
			return { label: "In your library", note: "This paper is in your library: click it to open it in the reader." };
		case "on_arxiv":
			return {
				label: "On arXiv",
				note: `Not in your library, but on arXiv as ${link.arxiv}: click it to open its arXiv page in the browser.`,
			};
		case "unresolved":
			return {
				label: "Not found",
				note: "Not in your library and no arXiv id is known for it: what was read of the reference is shown, and clicking it searches for its title in the browser.",
			};
	}
}

/** How far the reference was understood, when that is worth saying. */
export function confidenceNote(confidence: ReferenceConfidence): string | null {
	if (confidence === "raw_only")
		return "Only the entry as written could be read: its title could not be told apart from its authors and venue.";
	if (confidence === "title_parsed")
		return "Its title was read from the paper's reference list by a program and may be cut short or run on.";
	return null;
}

/** "cited by 3 papers Constellate has read". */
export function citedByLine(citedBy: number): string {
	return `cited by ${count(citedBy, "paper")} Constellate has read`;
}

/**
 * When the lookups of the works the paper cites stopped before every one was asked about: what stopped them, how
 * many are left, and how to go on. Null when nothing was left.
 */
export function lookupsStoppedLine(stopped: CitationNetwork["lookups_stopped"]): string | null {
	if (!stopped || stopped.left <= 0) return null;
	const left = `${count(stopped.left, "cited work")} ${stopped.left === 1 ? "was" : "were"} not looked up on arXiv yet`;
	const why =
		stopped.kind === "at_most"
			? "the lookups of the works this paper cites stopped early: one reading looks up only so many"
			: `the lookups of the works this paper cites stopped early: ${stopped.why}`;
	return `${why.charAt(0).toUpperCase()}${why.slice(1)}. ${left}; reading the references again continues where they stopped.`;
}

/**
 * A community's citation figures with their splits, or null while they are not counted: figures saved with a run
 * before citations from outside the run were counted have no splits, and are not shown as if they had.
 */
function countedCitations(community: Community): CountedCitations | null {
	const figures = community.citations;
	if (!figures) return null;
	const splits = [
		figures.cited_by_rest_of_run,
		figures.cited_by_outside_run,
		figures.cites_in_run,
		figures.cites_outside_run,
		figures.cites_on_arxiv,
		figures.cites_unresolved,
	];
	return splits.every((figure) => typeof figure === "number") ? (figures as CountedCitations) : null;
}

/** The communities of a run with the figures `counted` has for them (by tree path), in place of those saved with it. */
export function withCounted(communities: Community[], counted: RunCitations | null | undefined): Community[] {
	if (!counted) return communities;
	const byPath = new Map<string, CountedCommunity>();
	const gather = (counts: CountedCommunity[]) =>
		counts.forEach((one) => {
			byPath.set(one.tree_path, one);
			gather(one.children);
		});
	gather(counted.communities);
	const figures = (one: CountedCommunity): CountedCitations => ({
		papers_read: one.papers_read,
		cited_by: one.cited_by,
		cited_by_inside: one.cited_by_inside,
		cited_by_rest_of_run: one.cited_by_rest_of_run,
		cited_by_outside_run: one.cited_by_outside_run,
		cites: one.cites,
		cites_in_library: one.cites_in_library,
		cites_in_run: one.cites_in_run,
		cites_outside_run: one.cites_outside_run,
		cites_on_arxiv: one.cites_on_arxiv,
		cites_unresolved: one.cites_unresolved,
	});
	const replace = (community: Community): Community => {
		const found = byPath.get(community.tree_path);
		return { ...community, citations: found ? figures(found) : community.citations, children: community.children.map(replace) };
	};
	return communities.map(replace);
}

/** How often the community's papers are cited by the read papers of the library; null (sorted last) while not counted. */
export function citedByOf(community: Community): number | null {
	return countedCitations(community)?.cited_by ?? null;
}

/** How many distinct works its read papers cite; null (sorted last) while not counted or nothing of it is read. */
export function citesOf(community: Community): number | null {
	const counted = countedCitations(community);
	return counted && counted.papers_read > 0 ? counted.cites : null;
}

/** "212 of 739 papers read". */
function readShare(community: Community, papersRead: number): string {
	return `${papersRead.toLocaleString()} of ${count(community.size, "paper")} read`;
}

export const NOT_COUNTED = "not counted yet";

/** "1,204 citations", or "not counted yet". */
export function citedByFigure(community: Community): string {
	const counted = countedCitations(community);
	return counted ? count(counted.cited_by, "citation") : NOT_COUNTED;
}

/** "3,410 works cited · 212 of 739 papers read", "none of its papers read yet", or "not counted yet". */
export function citesFigure(community: Community): string {
	const counted = countedCitations(community);
	if (!counted) return NOT_COUNTED;
	if (counted.papers_read === 0) return "none of its papers read yet";
	return `${count(counted.cites, "work cited", "works cited")} · ${readShare(community, counted.papers_read)}`;
}

/** One part of a split figure: what it is, and how many. */
export interface SplitPart {
	label: string;
	value: number;
	/** what the part is made of, when it is two figures in one */
	detail?: string;
}

/** Where a community's citations come from, in three parts that add up to its "cited by"; null while not counted. */
export function citedBySplit(community: Community): SplitPart[] | null {
	const counted = countedCitations(community);
	if (!counted) return null;
	return [
		{ label: "from its own papers", value: counted.cited_by_inside },
		{ label: "from the rest of the run", value: counted.cited_by_rest_of_run },
		{ label: "from your library outside the run", value: counted.cited_by_outside_run },
	];
}

/** What a community's works cited are, in three parts that add up to its "cites"; null while not counted or not read. */
export function citesSplit(community: Community): SplitPart[] | null {
	const counted = countedCitations(community);
	if (!counted || counted.papers_read === 0) return null;
	return [
		{ label: "in the run", value: counted.cites_in_run },
		{ label: "in your library outside the run", value: counted.cites_outside_run },
		{
			label: "not in your library",
			value: counted.cites_on_arxiv + counted.cites_unresolved,
			detail: `${counted.cites_on_arxiv.toLocaleString()} on arXiv, ${counted.cites_unresolved.toLocaleString()} not found`,
		},
	];
}

/** "1,000 from its own papers, 500 from the rest of the run and 404 from your library outside the run". */
function splitWords(parts: SplitPart[]): string {
	const said = parts.map((part) => `${part.value.toLocaleString()} ${part.label}${part.detail ? ` (${part.detail})` : ""}`);
	return `${said.slice(0, -1).join(", ")} and ${said[said.length - 1]}`;
}

/** What the community's figures are made of, for its hover text. */
export function citationsNote(community: Community): string {
	const counted = countedCitations(community);
	if (!counted)
		return "Not counted yet: the figures saved with this run were counted before citations from outside the run were. They are counted again whenever the run is opened.";
	const citedBy = `Of its ${count(counted.cited_by, "citation")}, ${splitWords(citedBySplit(community)!)}.`;
	const split = citesSplit(community);
	const cites = split
		? ` Of the ${count(counted.cites, "work")} it cites, ${splitWords(split)}.`
		: " None of its papers' reference lists has been read yet, so what it cites is not known.";
	return citedBy + cites;
}

/** The run's own figures in a line: how often its papers are cited, from inside it and from outside, and what they cite. */
export function runCitationsLine(run: RunCitations): string {
	const citedBy =
		`The run's papers are cited ${count(run.cited_by, "time")}: ` +
		`${run.cited_by_inside.toLocaleString()} by papers of the run and ${run.cited_by_outside_run.toLocaleString()} by papers of your library outside it.`;
	if (run.papers_read === 0) return `${citedBy} None of its papers' reference lists has been read yet.`;
	const outside = run.cites_on_arxiv + run.cites_unresolved;
	return (
		`${citedBy} Works cited by its ${count(run.papers_read, "paper")} read: ${run.cites.toLocaleString()} ` +
		`(${run.cites_in_run.toLocaleString()} in the run, ${run.cites_outside_run.toLocaleString()} in your library outside it, ` +
		`${outside.toLocaleString()} not in your library).`
	);
}

export const CITATIONS_NOTE =
	"Counted from the reference lists Constellate has read (the papers of your library whose PDFs it has read), not from the whole literature.";

/** What a community's "Cited by" and "Cites" are, for their hover text and the sorts. */
export const CITED_BY_TITLE = `How many times its papers are cited by the papers of your library whose reference lists have been read, in the run or not. ${CITATIONS_NOTE}`;
export const CITES_TITLE = `How many different works its papers cite: papers of the run, of your library outside it, or not in your library. ${CITATIONS_NOTE}`;

/** Said wherever "cited by" is: what cannot be counted. */
export const NOT_COUNTABLE = "Not counted: papers that are not in your library.";

/** Where a work of a network stands: a paper of the run (or of the library, when no run is asked about), a paper of the
 *  library outside the run, on arXiv, or not found. */
export type WorkPlace = "in_library" | "outside_run" | "on_arxiv" | "unresolved";

export function workPlace(work: CitationNeighbour): WorkPlace {
	if (work.link.kind !== "in_library") return work.link.kind;
	return work.in_run === false ? "outside_run" : "in_library";
}

/** The tag of a paper of the library that is not in the run the reader was opened from; null for any other work. */
export function runTag(work: CitationNeighbour): { label: string; note: string } | null {
	if (workPlace(work) !== "outside_run") return null;
	return {
		label: "Outside this run",
		note: "A paper of your library that is not in this run: click it to open it in the reader. It is not on this run's graph.",
	};
}

/** "Showing 12 of 12 references: 3 in this run, 2 in your library outside this run, 4 on arXiv, 3 not found". */
export function referencesLine(totals: CitationNetwork["totals"], shown: number): string {
	const known = `${totals.references_on_arxiv.toLocaleString()} on arXiv, ${totals.references_unresolved.toLocaleString()} not found`;
	const head = showing(shown, totals.references, "reference");
	if (typeof totals.references_in_run === "number" && typeof totals.references_outside_run === "number")
		return `${head}: ${totals.references_in_run.toLocaleString()} in this run, ${totals.references_outside_run.toLocaleString()} in your library outside this run, ${known}`;
	return `${head}: ${totals.references_in_library.toLocaleString()} in your library, ${known}`;
}

/**
 * Where a network comes from, and with a run, how many of the papers citing it are of the run and how many of the
 * library outside it: "From the reference lists Constellate has read of your library: 12 of 300 papers. Built
 * 2026-11-28. Cited by 3 papers of this run and 2 of your library outside it."
 */
export function networkSourceLine(network: CitationNetwork): string {
	const { totals } = network;
	const source = `From the reference lists Constellate has read of your library: ${network.papers_read.toLocaleString()} of ${count(network.papers_in_library, "paper")}. Built ${network.built}.`;
	if (totals.citations_in_run === null) return source;
	if (typeof totals.citations_outside_run !== "number") return `${source} Cited by ${count(totals.citations_in_run, "paper")} of this run.`;
	return `${source} Cited by ${count(totals.citations_in_run, "paper")} of this run and ${totals.citations_outside_run.toLocaleString()} of your library outside it.`;
}

/** "Showing 5 of 5 citing papers Constellate has read in your library: 2 in this run, 3 outside it". */
export function citationsLine(totals: CitationNetwork["totals"], shown: number): string {
	const limited = totals.citations_shown < totals.citations ? ", the most cited" : "";
	const head = `${showing(shown, totals.citations, "citing paper")} Constellate has read in your library${limited}`;
	if (typeof totals.citations_in_run === "number" && typeof totals.citations_outside_run === "number")
		return `${head}: ${totals.citations_in_run.toLocaleString()} in this run, ${totals.citations_outside_run.toLocaleString()} outside it`;
	return head;
}
