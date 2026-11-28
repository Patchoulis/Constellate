/**
 * The papers of a community closest in meaning to each statement of its description, found by the engine with the
 * run's embedding model on this computer (engine features/writing/closest.rs): free, and nothing is sent anywhere.
 * What is said of them, and which are shown. Nothing here sends anything.
 */
import type { ClosestPaper, ClosestPapers, ClosestStatement, CommunitySection } from "@rp/contracts";
import { similarity } from "@/lib/units";

/** What the closest papers are, said on hover wherever they are listed. */
export const CLOSEST_NOTE =
	"Found by comparing this statement with the abstracts of this community's papers using the embedding model: closeness of meaning, not a check that the paper says this.";

/** What the model's own citations are, said over them. */
export const CITED_LABEL = "Cited by the model, quotes checked";

/** Closest papers shown before "show all". */
export const SHOWN_AT_FIRST = 3;

/** "0.71 cosine similarity". */
export function closeness(paper: Pick<ClosestPaper, "cosine">): string {
	return similarity(paper.cosine);
}

/** Whether a paper's supporting words are its title: then they are not shown again under it. */
export function quotesTitle(paper: Pick<ClosestPaper, "evidence">): boolean {
	const established = paper.evidence.established;
	return established.kind === "abstract" && established.field === "title";
}

/** The papers of `section`'s model citations, by id: never listed again under its closest papers. */
export function citedIn(section: Pick<CommunitySection, "cites"> | null | undefined): Set<string> {
	return new Set((section?.cites ?? []).map((cite) => cite.evidence.paper));
}

/** The closest papers to list: none already cited, at most `SHOWN_AT_FIRST` until all are asked for, and how many
 *  more there are. */
export function shownPapers(
	papers: ClosestPaper[],
	cited: Set<string>,
	all: boolean,
): { shown: ClosestPaper[]; more: number; listed: number } {
	const listed = papers.filter((paper) => !cited.has(paper.id));
	const shown = all ? listed : listed.slice(0, SHOWN_AT_FIRST);
	return { shown, more: listed.length - shown.length, listed: listed.length };
}

/** The statement found for a section, by its heading; null when there is none (yet). */
export function statementOf(status: ClosestPapers | null, heading: string): ClosestStatement | null {
	return status?.statements?.find((statement) => statement.section === heading) ?? null;
}

/** What a statement with no close paper says, with the floor in its unit. */
export function noneCloseWords(floor: number): string {
	return `No paper of this community is close to this (none reaches ${similarity(floor)}).`;
}

/** Where finding them stands, as the quiet line under the description; null once they are shown. */
export function closestLine(
	status: ClosestPapers | null,
	job: { state: string; error?: string },
	elsewhere: boolean,
	unreachable: string | null,
): { text: string; tone: "quiet" | "problem" } | null {
	if (status?.state === "done") return null;
	if (unreachable) return { text: `The closest papers could not be asked for: ${unreachable}`, tone: "problem" };
	if (status?.unavailable) return { text: `The closest papers cannot be found. ${status.unavailable}`, tone: "quiet" };
	// another job is running (and refused this one as busy): it is started once that ends
	if (elsewhere && job.state !== "running" && status?.state !== "running")
		return { text: "The closest papers are found once what is being done now ends.", tone: "quiet" };
	if (job.state === "failed") return { text: `The closest papers could not be found: ${job.error ?? "the job failed"}`, tone: "problem" };
	if (status?.state === "failed")
		return { text: `The closest papers could not be found: ${status.error ?? "the job failed"}`, tone: "problem" };
	return { text: "Finding the closest papers…", tone: "quiet" };
}
