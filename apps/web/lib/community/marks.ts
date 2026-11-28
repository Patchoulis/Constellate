/**
 * Marked papers: the ones a user singled out in a run, to keep in view and come back to.
 *
 * Marks belong to a run and live in this browser's localStorage, one key per run name, as
 * `{"version":1,"papers":[...ids, newest first]}`. The engine does not keep them.
 *
 * The core is pure (a list of paper ids, newest first, and what can be done to it); `useMarks` holds one run's
 * list in React state and writes every change through to storage.
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

export function serialiseMarks(marks: Marks): string {
	return JSON.stringify({ version: VERSION, papers: marks });
}

/**
 * Whatever storage holds for a run, read as marks. Anything else there (nothing, broken JSON, another shape,
 * another version, ids that are not strings, the same id twice) gives what can be trusted of it, never an error.
 */
export function parseMarks(text: string | null | undefined): Marks {
	if (!text) return [];
	let stored: unknown;
	try {
		stored = JSON.parse(text);
	} catch (error) {
		return [];
	}
	if (typeof stored !== "object" || stored === null) return [];
	const { version, papers } = stored as { version?: unknown; papers?: unknown };
	if (version !== VERSION || !Array.isArray(papers)) return [];
	const ids = papers.slice(0, MOST).filter((id): id is string => typeof id === "string" && id.length > 0 && id.length <= 512);
	return [...new Set(ids)];
}

/** Read a run's marks from storage; none where storage cannot be read (a private window, a disabled store). */
function load(run: string): Marks {
	try {
		return parseMarks(window.localStorage.getItem(marksKey(run)));
	} catch (error) {
		return [];
	}
}

function save(run: string, marks: Marks) {
	try {
		if (marks.length === 0) window.localStorage.removeItem(marksKey(run));
		else window.localStorage.setItem(marksKey(run), serialiseMarks(marks));
	} catch (error) {
		// storage is full or switched off: the marks still hold until the window is closed
	}
}

/** A run was deleted: its marks go with it, so a run made later under the same name does not find them. */
export function forgetMarks(run: string) {
	save(run, []);
}

export interface MarksOf {
	/** the run's marked papers that are in its graph, newest first */
	marked: Marks;
	has: (id: string) => boolean;
	toggle: (id: string) => void;
	/** mark every one of these that is not marked yet */
	addMany: (ids: readonly string[]) => void;
	remove: (id: string) => void;
	clear: () => void;
}

/**
 * One run's marks. `known` is the ids of the papers in the run's graph, or null while it is loading: until then
 * nothing is shown and nothing is dropped. Marks of papers that are not in the graph are left out, and are gone
 * from storage the next time a mark is changed.
 */
export function useMarks(run: string, known: ReadonlySet<string> | null): MarksOf {
	// What storage holds, for which run. Read after mounting: the page is also rendered where there is no storage.
	const [stored, setStored] = useState<{ run: string; marks: Marks }>({ run, marks: [] });
	useEffect(() => {
		setStored({ run, marks: load(run) });
		// another window of the app changed this run's marks
		// (a null key: that window cleared all of storage)
		const onStorage = (event: StorageEvent) =>
			(event.key === marksKey(run) || event.key === null) && setStored({ run, marks: parseMarks(event.newValue) });
		window.addEventListener("storage", onStorage);
		return () => window.removeEventListener("storage", onStorage);
	}, [run]);

	const marked = useMemo(() => (known && stored.run === run ? keepKnown(stored.marks, known) : []), [known, stored, run]);
	// From the latest state, not this render's: two changes before the next render both count.
	const change = useCallback(
		(next: (marks: Marks) => Marks) => {
			if (!known) return;
			setStored((before) => {
				const marks = next(keepKnown(before.run === run ? before.marks : [], known));
				save(run, marks); // the same value however often React calls this
				return { run, marks };
			});
		},
		[known, run],
	);
	return {
		marked,
		has: useCallback((id: string) => isMarked(marked, id), [marked]),
		toggle: useCallback((id: string) => change((marks) => toggleMark(marks, id)), [change]),
		addMany: useCallback((ids: readonly string[]) => change((marks) => addMarks(marks, ids)), [change]),
		remove: useCallback((id: string) => change((marks) => removeMark(marks, id)), [change]),
		clear: useCallback(() => change(() => []), [change]),
	};
}
