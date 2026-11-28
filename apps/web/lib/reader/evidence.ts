/**
 * What the interface needs to show evidence and land on it: a paper's short label, what hovering a citation
 * says, where in the reader a citation opens, and the passage and characters to highlight there. Plain functions,
 * so that they are tested (tests/reader/evidence.test.tsx); components/reader/evidence-link.tsx draws them.
 */
import { UNREADABLE, type Evidence, type PaperPassages, type PassageAddress, type QuoteMatch } from "@rp/contracts";
import { count } from "@/lib/units";

/**
 * The characters `start..end` of `text`, counted as the engine counts them: Unicode code points. `String.slice`
 * counts UTF-16 units, which differ for the mathematical letters PDFs are full of ("𝐖").
 */
export function codePoints(text: string, start: number, end?: number): string {
	return Array.from(text).slice(start, end).join("");
}

/**
 * `text` cut at each formula the PDF does not say the characters of (`UNREADABLE`): the paper's words, with `null`
 * where a note stands instead. Text without one is itself alone.
 */
export function unreadableParts(text: string): (string | null)[] {
	const parts: (string | null)[] = [];
	text.split(UNREADABLE).forEach((words, at) => {
		if (at > 0) parts.push(null);
		if (words) parts.push(words);
	});
	return parts;
}

/**
 * `text` cut at the superscripts and subscripts the engine marks as `^{…}` and `_{…}` ("CoT^{gt}_{d}"): each piece
 * with whether it is raised, lowered or on the line, the marks themselves left out. A mark cut in two (a highlight
 * that ends inside one) is shown as it is.
 */
export function scriptParts(text: string): { text: string; script: "sup" | "sub" | null }[] {
	const parts: { text: string; script: "sup" | "sub" | null }[] = [];
	let from = 0;
	for (const found of text.matchAll(/([\^_])\{([^{}]*)\}/g)) {
		const at = found.index ?? 0;
		if (at > from) parts.push({ text: text.slice(from, at), script: null });
		parts.push({ text: found[2], script: found[1] === "^" ? "sup" : "sub" });
		from = at + found[0].length;
	}
	if (from < text.length) parts.push({ text: text.slice(from), script: null });
	return parts;
}

/** Is the passage all formula that could not be read, with hardly a letter or digit of its own (an equation
 *  number at most)? There is nothing in it to explain. As the engine's `Passage::unreadable`. */
export function whollyUnreadable(passage: { text: string }): boolean {
	return passage.text.includes(UNREADABLE) && Array.from(passage.text).filter((c) => /[\p{L}\p{N}]/u.test(c)).length < 4;
}

/** A paper as a citation names it: "Vaswani et al., 2017", "Vaswani and Shazeer, 2017", "Vaswani, 2017". */
export function shortLabel(paper: { authors: string[]; published?: string | null }): string {
	const surname = (author: string) => {
		const name = author.includes(",") ? author.split(",")[0] : (author.trim().split(/\s+/).pop() ?? author);
		return name.trim();
	};
	const year = /\b(19|20)\d{2}\b/.exec(paper.published ?? "")?.[0];
	const [first, second] = paper.authors;
	const who = !first
		? "Unknown authors"
		: paper.authors.length === 1
			? surname(first)
			: paper.authors.length === 2
				? `${surname(first)} and ${surname(second)}`
				: `${surname(first)} et al.`;
	return year ? `${who}, ${year}` : who;
}

/** "p. 4", "pp. 4–5". */
export function pages(address: PassageAddress): string {
	return address.end_page > address.page ? `pp. ${address.page}–${address.end_page}` : `p. ${address.page}`;
}

/** Where a place is, in words: "p. 4, 3.2. Results". */
export function placeName(address: PassageAddress): string {
	return address.section ? `${pages(address)}, ${address.section}` : pages(address);
}

function matchNote(matched: QuoteMatch | undefined): string {
	if (matched?.kind === "exact") return "Found word for word in the paper.";
	if (matched?.kind === "close" && typeof matched.differences === "number")
		return `Found in the paper with ${count(matched.differences, "letter")} different from the model's quote; shown as the paper words it.`;
	return "Found in the paper; shown as the paper words it."; // a match this version does not know (a newer engine)
}

/**
 * What hovering a citation says: the paper's words, where they are, and how that was established. The statement
 * the citation supports is a model's reading of the paper; the quote is the paper's.
 */
export function evidenceTitle(evidence: Evidence, label: string): string {
	const quote = `“${evidence.quote}”`;
	const established = evidence.established;
	if (established.kind === "verified") {
		return `${quote}\n${label}, ${placeName(established.address)}. ${matchNote(established.matched)} Click to open the paper there.`;
	}
	if (established.kind !== "abstract") return `${quote}\n${label}. Click to open the paper.`; // a kind this version does not know
	return (
		`${quote}\n${label}, ${established.field}. ${matchNote(established.matched)} ` +
		"Only the paper's title and abstract were checked: its PDF had not been read, so no page is known. Click to open the paper."
	);
}

/** The browser's PDF viewer at a page: `#page=N` is the one open parameter every viewer honours. */
export function pdfAt(url: string, page: number | undefined): string {
	return page ? `${url.split("#")[0]}#page=${page}` : url;
}

/** The index of the passage an address starts in, or -1. */
export function passageAt(passages: PaperPassages, address: PassageAddress): number {
	return passages.passages.findIndex(
		(passage) => passage.address.page === address.page && passage.address.start <= address.start && address.start < passage.address.end,
	);
}

/** A passage's text in three parts: before the highlight, the highlight, after it. Characters are code points. */
export function highlighted(
	passage: { address: PassageAddress; text: string },
	address: PassageAddress | null | undefined,
): [string, string, string] | null {
	if (!address || address.page !== passage.address.page) return null;
	const from = address.start - passage.address.start;
	if (from < 0 || address.start >= passage.address.end) return null;
	const length = Array.from(passage.text).length;
	// a highlight that runs on to the next page is drawn to the end of this passage
	const to = address.end_page === address.page ? Math.min(address.end - passage.address.start, length) : length;
	return [codePoints(passage.text, 0, from), codePoints(passage.text, from, to), codePoints(passage.text, to)];
}

/** Where a piece of evidence opens the reader. */
export function readerTarget(evidence: Evidence) {
	const established = evidence.established;
	return { paper: evidence.paper, address: established.kind === "verified" ? established.address : null, quote: evidence.quote };
}
