/**
 * Marked papers: the ones a user singled out in a run, to keep in view and come back to.
 *
 * Marks belong to a run and live in this browser's localStorage, one key per run name, as
 * `{"version":1,"papers":[...ids, newest first],"hidden":[...ids],"noLinks":[...ids]}`: the marked papers not drawn
 * as marked on the graph, and those whose links are not drawn. Both lists are optional, so what was stored before
 * they existed reads as every mark shown with its links. The engine does not keep any of it.
 *
 * The core is pure (a list of paper ids, newest first, the two switches, and what can be done to them); `useMarks`
 * holds one run's marks in React state and writes every change through to storage.
 */
import { useCallback, useEffect, useMemo, useState } from "react";

/** A run's marked papers: their ids, newest mark first, each once. */
export type Marks = readonly string[];

const VERSION = 1;
/** More than anyone marks by hand: whatever is in storage beyond this is not read. */
const MOST = 5000;

/** The localStorage key of a run's marks. The run's name is encoded, so no name can reach another run's key. */
export function marksKey(run: string): string {
	return `constellate.marks.${encodeURIComponent(run)}`;
}

export function isMarked(marks: Marks, id: string): boolean {
	return marks.includes(id);
}

/** Mark a paper: it becomes the newest. Marking a paper that is marked already changes nothing. */
export function addMark(marks: Marks, id: string): Marks {
	return isMarked(marks, id) ? marks : [id, ...marks];
}

/** Mark several papers at once (a lasso's): those not marked yet become the newest, in the order given. */
export function addMarks(marks: Marks, ids: readonly string[]): Marks {
	const fresh = [...new Set(ids)].filter((id) => !isMarked(marks, id));
	return fresh.length === 0 ? marks : [...fresh, ...marks];
}

export function removeMark(marks: Marks, id: string): Marks {
	return isMarked(marks, id) ? marks.filter((other) => other !== id) : marks;
}

/** Mark a paper, or unmark it if it is marked. Twice on an unmarked paper is no change at all; twice on a marked
 *  one marks it again as the newest. */
export function toggleMark(marks: Marks, id: string): Marks {
	return isMarked(marks, id) ? removeMark(marks, id) : addMark(marks, id);
}

/** Only the marks of papers in the run's graph: a paper that is no longer in it cannot be shown or opened. */
export function keepKnown(marks: Marks, known: ReadonlySet<string>): Marks {
	return marks.every((id) => known.has(id)) ? marks : marks.filter((id) => known.has(id));
}

/**
 * A run's marks with their two switches: which marked papers are not drawn as marked on the graph (they stay marked,
 * listed and used by everything else marks are for), and which are drawn without their links. Each switch only ever
 * names marked papers.
 */
export interface MarkState {
	papers: Marks;
	hidden: ReadonlySet<string>;
	noLinks: ReadonlySet<string>;
}

const NONE: ReadonlySet<string> = new Set();
export const NO_MARKS: MarkState = { papers: [], hidden: NONE, noLinks: NONE };

/** A switch's set with only the ids still marked, the same set when nothing goes. */
function within(set: ReadonlySet<string>, papers: Marks): ReadonlySet<string> {
	if (set.size === 0) return set;
	const marked = new Set(papers);
	return [...set].every((id) => marked.has(id)) ? set : new Set([...set].filter((id) => marked.has(id)));
}

/** A switch's set with `ids` added or taken out. */
function switched(set: ReadonlySet<string>, ids: readonly string[], on: boolean): ReadonlySet<string> {
	const next = new Set(set);
	for (const id of ids) {
		if (on) next.add(id);
		else next.delete(id);
	}
	return next.size === set.size && ids.every((id) => set.has(id) === on) ? set : next;
}

/** The marks changed by `change` on their ids: the switches of a paper no longer marked go, and a paper newly marked
 *  (one by one or with a lasso) is shown with its links. */
export function withPapers(state: MarkState, change: (papers: Marks) => Marks): MarkState {
	const papers = change(state.papers);
	if (papers === state.papers) return state;
	const before = new Set(state.papers);
	const fresh = papers.filter((id) => !before.has(id));
	return {
		papers,
		hidden: switched(within(state.hidden, papers), fresh, false),
		noLinks: switched(within(state.noLinks, papers), fresh, false),
	};
}

/** Show a marked paper on the graph, or hide it there (it stays marked). Papers not marked are left alone. */
export function setMarkShown(state: MarkState, ids: readonly string[], shown: boolean): MarkState {
	const marked = ids.filter((id) => state.papers.includes(id));
	const hidden = switched(state.hidden, marked, !shown);
	return hidden === state.hidden ? state : { ...state, hidden };
}

/** Draw a marked paper's links, or not. Papers not marked are left alone. */
export function setMarkLinks(state: MarkState, ids: readonly string[], links: boolean): MarkState {
	const marked = ids.filter((id) => state.papers.includes(id));
	const noLinks = switched(state.noLinks, marked, !links);
	return noLinks === state.noLinks ? state : { ...state, noLinks };
}

/** Only the marks of papers in the run's graph, and their switches. */
export function keepKnownState(state: MarkState, known: ReadonlySet<string>): MarkState {
	const papers = keepKnown(state.papers, known);
	if (papers === state.papers) return state;
	return { papers, hidden: within(state.hidden, papers), noLinks: within(state.noLinks, papers) };
}

/** What the graph is given of the marks: the marked papers drawn as marked, newest first, and those of them drawn
 *  without their links. */
export function marksOnGraph(state: MarkState): { shown: Marks; noLinks: ReadonlySet<string> } {
	const shown = state.hidden.size === 0 ? state.papers : state.papers.filter((id) => !state.hidden.has(id));
	return { shown, noLinks: state.noLinks.size === 0 ? NONE : within(state.noLinks, shown) };
}

/** The Marked list's header: "12 papers marked in this run, newest first · 3 hidden on the graph · 2 without links". */
export function markedListLine(total: number, hidden: number, linksOff: number): string {
	const papers = `${total.toLocaleString("en-US")} ${total === 1 ? "paper" : "papers"} marked in this run, newest first`;
	return [
		papers,
		...(hidden ? [`${hidden.toLocaleString("en-US")} hidden on the graph`] : []),
		...(linksOff ? [`${linksOff.toLocaleString("en-US")} without links`] : []),
	].join(" · ");
}

export function serialiseMarks(marks: Marks | MarkState): string {
	const state = "papers" in marks ? marks : { papers: marks, hidden: NONE, noLinks: NONE };
	return JSON.stringify({
		version: VERSION,
		papers: state.papers,
		// only when there are any, so marks with nothing switched are stored as they always were
		...(state.hidden.size ? { hidden: [...state.hidden] } : {}),
		...(state.noLinks.size ? { noLinks: [...state.noLinks] } : {}),
	});
}

/** Ids that can be trusted, each once, at most MOST. */
function ids(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return [...new Set(value.slice(0, MOST).filter((id): id is string => typeof id === "string" && id.length > 0 && id.length <= 512))];
}

/**
 * Whatever storage holds for a run, read as marks and their switches. Anything else there (nothing, broken JSON,
 * another shape, another version, ids that are not strings, the same id twice) gives what can be trusted of it,
 * never an error; a switch that cannot be trusted reads as every mark shown with its links.
 */
export function parseMarkState(text: string | null | undefined): MarkState {
	if (!text) return NO_MARKS;
	let stored: unknown;
	try {
		stored = JSON.parse(text);
	} catch (error) {
		return NO_MARKS;
	}
	if (typeof stored !== "object" || stored === null) return NO_MARKS;
	const { version, papers, hidden, noLinks } = stored as { version?: unknown; papers?: unknown; hidden?: unknown; noLinks?: unknown };
	if (version !== VERSION || !Array.isArray(papers)) return NO_MARKS;
	const marked = ids(papers);
	return { papers: marked, hidden: within(new Set(ids(hidden)), marked), noLinks: within(new Set(ids(noLinks)), marked) };
}

/** Whatever storage holds for a run, read as the marked papers alone. */
export function parseMarks(text: string | null | undefined): Marks {
	return parseMarkState(text).papers;
}

/** Read a run's marks from storage; none where storage cannot be read (a private window, a disabled store). */
function load(run: string): MarkState {
	try {
		return parseMarkState(window.localStorage.getItem(marksKey(run)));
	} catch (error) {
		return NO_MARKS;
	}
}

function save(run: string, state: MarkState) {
	try {
		if (state.papers.length === 0) window.localStorage.removeItem(marksKey(run));
		else window.localStorage.setItem(marksKey(run), serialiseMarks(state));
	} catch (error) {
		// storage is full or switched off: the marks still hold until the window is closed
	}
}

/** A run was deleted: its marks go with it, so a run made later under the same name does not find them. */
export function forgetMarks(run: string) {
	save(run, NO_MARKS);
}

export interface MarksOf {
	/** the run's marked papers that are in its graph, newest first */
	marked: Marks;
	/** of those, the ones not drawn as marked on the graph, and the ones drawn without their links */
	hidden: ReadonlySet<string>;
	noLinks: ReadonlySet<string>;
	has: (id: string) => boolean;
	toggle: (id: string) => void;
	/** mark every one of these that is not marked yet */
	addMany: (ids: readonly string[]) => void;
	remove: (id: string) => void;
	clear: () => void;
	/** show these marked papers on the graph, or hide them there */
	setShown: (ids: readonly string[], shown: boolean) => void;
	/** draw these marked papers' links, or not */
	setLinks: (ids: readonly string[], links: boolean) => void;
}

/**
 * One run's marks. `known` is the ids of the papers in the run's graph, or null while it is loading: until then
 * nothing is shown and nothing is dropped. Marks of papers that are not in the graph are left out, and are gone
 * from storage the next time a mark is changed.
 */
export function useMarks(run: string, known: ReadonlySet<string> | null): MarksOf {
	// What storage holds, for which run. Read after mounting: the page is also rendered where there is no storage.
	const [stored, setStored] = useState<{ run: string; marks: MarkState }>({ run, marks: NO_MARKS });
	useEffect(() => {
		setStored({ run, marks: load(run) });
		// another window of the app changed this run's marks
		// (a null key: that window cleared all of storage)
		const onStorage = (event: StorageEvent) =>
			(event.key === marksKey(run) || event.key === null) && setStored({ run, marks: parseMarkState(event.newValue) });
		window.addEventListener("storage", onStorage);
		return () => window.removeEventListener("storage", onStorage);
	}, [run]);

	const state = useMemo(() => (known && stored.run === run ? keepKnownState(stored.marks, known) : NO_MARKS), [known, stored, run]);
	// From the latest state, not this render's: two changes before the next render both count.
	const change = useCallback(
		(next: (marks: MarkState) => MarkState) => {
			if (!known) return;
			setStored((before) => {
				const marks = next(keepKnownState(before.run === run ? before.marks : NO_MARKS, known));
				save(run, marks); // the same value however often React calls this
				return { run, marks };
			});
		},
		[known, run],
	);
	const marked = state.papers;
	return {
		marked,
		hidden: state.hidden,
		noLinks: state.noLinks,
		has: useCallback((id: string) => isMarked(marked, id), [marked]),
		toggle: useCallback((id: string) => change((marks) => withPapers(marks, (papers) => toggleMark(papers, id))), [change]),
		addMany: useCallback((ids: readonly string[]) => change((marks) => withPapers(marks, (papers) => addMarks(papers, ids))), [change]),
		remove: useCallback((id: string) => change((marks) => withPapers(marks, (papers) => removeMark(papers, id))), [change]),
		clear: useCallback(() => change(() => NO_MARKS), [change]),
		setShown: useCallback((ids: readonly string[], shown: boolean) => change((marks) => setMarkShown(marks, ids, shown)), [change]),
		setLinks: useCallback((ids: readonly string[], links: boolean) => change((marks) => setMarkLinks(marks, ids, links)), [change]),
	};
}
