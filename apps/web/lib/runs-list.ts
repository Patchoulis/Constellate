import type { RunPage, RunRow, RunSummary } from "@rp/contracts";
import { count } from "@/lib/units";

/**
 * The sidebar's list of runs, a page at a time, searched as the user types.
 *
 * The engine serves pages of the list (`GET /api/runs?offset=&limit=&sort=&search=`, a `RunPage`): a library with
 * hundreds of runs is never read or drawn whole. The Python backend ignores the query and answers every run (a
 * `RunSummary[]`): that list is searched and paged here, the same way, so the sidebar behaves alike with both.
 *
 * The list is in exactly one of these at a time, and `runsListStep` is the one place it changes:
 *
 *     loading      the first page for `search` is asked for; `rows` are the last search's, kept in view until then
 *     shown        `rows` (pages 1..n) for `search`, of `total`
 *     more         a further page is asked for; `rows` stay
 *     failed       asking failed (`error`); `rows` are what was shown before, if anything
 *
 * A page answered for another search than the one asked now, or for an offset that is not the next one, changes
 * nothing: typing on, or a "Show more" clicked twice, never mixes two lists.
 */

/** Runs asked for at a time. */
export const RUNS_PAGE = 50;
/** How long typing must pause before the search is asked. */
export const SEARCH_DEBOUNCE_MS = 250;

/** A run as the list shows it. */
export interface ListedRun {
	name: string;
	title: string;
	/** its papers, null for a run not read yet */
	papers: number | null;
	/** "1,204 papers · 31 communities"; null for a run not read yet */
	details: string | null;
	/** why it cannot be opened (its files cannot be read), shown instead of opening it */
	problem: string | null;
}

export type RunsList =
	| { state: "loading"; search: string; rows: ListedRun[]; total: number | null }
	| { state: "shown"; search: string; rows: ListedRun[]; total: number }
	| { state: "more"; search: string; rows: ListedRun[]; total: number }
	| { state: "failed"; search: string; rows: ListedRun[]; total: number | null; error: string };

export type RunsListEvent =
	| { kind: "search"; search: string }
	| { kind: "more" }
	| { kind: "page"; search: string; offset: number; rows: ListedRun[]; total: number }
	| { kind: "refused"; search: string; error: string };

export const FIRST: RunsList = { state: "loading", search: "", rows: [], total: null };

export function runsListStep(list: RunsList, event: RunsListEvent): RunsList {
	switch (event.kind) {
		case "search":
			return { state: "loading", search: event.search, rows: list.rows, total: list.total };
		case "more":
			return list.state === "shown" && list.rows.length < list.total ? { ...list, state: "more" } : list;
		case "page": {
			if (event.search !== list.search) return list; // another search's page
			if (list.state === "loading" && event.offset === 0)
				return { state: "shown", search: list.search, rows: event.rows, total: event.total };
			if (list.state === "more" && event.offset === list.rows.length)
				return { state: "shown", search: list.search, rows: [...list.rows, ...event.rows], total: event.total };
			return list;
		}
		case "refused":
			if (event.search !== list.search || (list.state !== "loading" && list.state !== "more")) return list;
			return { state: "failed", search: list.search, rows: list.rows, total: list.total, error: event.error };
	}
}

/** The offset of the page a list asks for now: the first while loading, the next while asking for more; null otherwise. */
export function offsetAsked(list: RunsList): number | null {
	return list.state === "loading" ? 0 : list.state === "more" ? list.rows.length : null;
}

const detailsOf = (papers: number, communities: number) => `${count(papers, "paper")} · ${count(communities, "community", "communities")}`;

export function listedOfSummary(run: RunSummary): ListedRun {
	return { name: run.name, title: run.title, papers: run.papers, details: detailsOf(run.papers, run.communities), problem: null };
}

export function listedOfRow(row: RunRow): ListedRun {
	switch (row.kind) {
		case "run":
			return { name: row.name, title: row.title, papers: row.papers, details: detailsOf(row.papers, row.communities), problem: null };
		case "unreadable":
			return { name: row.name, title: row.name, papers: null, details: null, problem: `Its files cannot be read: ${row.reason}` };
		case "unread":
			return { name: row.name, title: row.name, papers: null, details: null, problem: null };
	}
}

/** Does a run match a search: every word of it in its title or name, whatever the case (as the engine searches)? */
export function matchesSearch(run: { name: string; title: string }, search: string): boolean {
	const words = search.toLowerCase().split(/\s+/).filter(Boolean);
	const where = `${run.title}\n${run.name}`.toLowerCase();
	return words.every((word) => where.includes(word));
}

/**
 * What the list endpoint answered, as one page for `search` from `offset`: the engine's page as it is, or the Python
 * backend's whole list searched and cut here.
 */
export function pageOf(answer: RunPage | RunSummary[], search: string, offset: number): { rows: ListedRun[]; total: number } {
	if (Array.isArray(answer)) {
		const found = answer.filter((run) => matchesSearch(run, search));
		return { rows: found.slice(offset, offset + RUNS_PAGE).map(listedOfSummary), total: found.length };
	}
	return { rows: answer.rows.map(listedOfRow), total: answer.total };
}

/**
 * The rows to draw: the list, with the open run first when it is not among them (another page, or not matching the
 * search), so that the run being looked at is always in sight. `open` is what is known of it, or its name alone.
 */
export function withOpenRun(
	rows: ListedRun[],
	open: string | null,
	known: ListedRun | undefined,
): { rows: ListedRun[]; pinned: string | null } {
	if (open === null || rows.some((row) => row.name === open)) return { rows, pinned: null };
	const shown = known ?? { name: open, title: open, papers: null, details: null, problem: null };
	return { rows: [shown, ...rows], pinned: open };
}

/** "Showing 50 of 1,203 runs" when not all are shown; null when they are. */
export function shownLine(list: RunsList): string | null {
	if (list.total === null || list.rows.length >= list.total) return null;
	return `Showing ${list.rows.length.toLocaleString()} of ${count(list.total, "run")}`;
}
