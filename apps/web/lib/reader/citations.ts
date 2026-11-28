/**
 * The citation network of one paper, as it is drawn and listed: where each work goes, how the lists are ordered
 * and filtered, and what the figures say. Pure functions, so the same network always gives the same picture.
 *
 * The picture reads left to right as time: the seed paper in the middle, the works it cites (prior work) on the
 * left by year, the papers that cite it (derivative work) on the right by year. A work's size says how many papers
 * Constellate has read cite it; within a year, the most cited sit nearest the middle line.
 */
import type { CitationLink, CitationNeighbour, CitationNetwork, Community, ReferenceConfidence } from "@rp/contracts";
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

/** A community's citation figures, or null while none of its papers' reference lists has been read. */
function readCitations(community: Community) {
	return community.citations && community.citations.papers_read > 0 ? community.citations : null;
}

/** How often the community's papers are cited by the run's read papers; null (sorted last) while nothing of it is read. */
export function citedByOf(community: Community): number | null {
	return readCitations(community)?.cited_by ?? null;
}

/** How many distinct works its read papers cite; null (sorted last) while nothing of it is read. */
export function citesOf(community: Community): number | null {
	return readCitations(community)?.cites ?? null;
}

/** "212 of 739 papers read". */
function readShare(community: Community, papersRead: number): string {
	return `${papersRead.toLocaleString()} of ${count(community.size, "paper")} read`;
}

/** "1,204 citations · 212 of 739 papers read", or "not read yet". */
export function citedByFigure(community: Community): string {
	const read = readCitations(community);
	return read ? `${count(read.cited_by, "citation")} · ${readShare(community, read.papers_read)}` : "not read yet";
}

/** "3,410 works cited · 212 of 739 papers read", or "not read yet". */
export function citesFigure(community: Community): string {
	const read = readCitations(community);
	return read ? `${count(read.cites, "work cited", "works cited")} · ${readShare(community, read.papers_read)}` : "not read yet";
}

/** What the community's figures are made of, for its hover text. */
export function citationsNote(community: Community): string {
	const read = readCitations(community);
	if (!read) return "None of its papers' reference lists has been read yet.";
	return (
		`${count(read.cited_by_inside, "citation")} of the ${count(read.cited_by, "citation")} come from its own papers. ` +
		`${count(read.cites_in_library, "work")} of the ${count(read.cites, "work")} it cites ${read.cites_in_library === 1 ? "is" : "are"} in your library.`
	);
}

export const CITATIONS_NOTE =
	"Counted from the reference lists Constellate has read (the run's papers whose PDFs it has read), not from the whole literature.";
