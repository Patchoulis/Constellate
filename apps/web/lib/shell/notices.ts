/**
 * What the engine reports that is not about any one run: how it listens (`Status.warnings`), and what of the
 * earlier version's data could not be moved in (`Library.import_problems`, which the engine also lists among the
 * warnings: shown once, as an import problem, not twice).
 */

/** What the person can do about an import problem; the problem itself names the part and the file. */
export const IMPORT_ADVICE =
	"Closing Constellate and opening it again tries once more. If this is still said after that, the file it names (in the earlier version's library folder) may be damaged: check it, or move it aside to leave that part behind.";

/** The engine's warnings that are not import problems: those are shown on their own. */
export function engineWarnings(warnings: string[] | undefined, importProblems: string[] | undefined): string[] {
	const problems = new Set(importProblems ?? []);
	return (warnings ?? []).filter((warning) => !problems.has(warning));
}

/** The import problems a notice in the sidebar still shows: those not dismissed in this window since it started. */
export function notDismissed(problems: string[] | undefined, dismissed: ReadonlySet<string>): string[] {
	return (problems ?? []).filter((problem) => !dismissed.has(problem));
}

const DISMISSED_KEY = "constellate.dismissed-import-problems";

/** The import problems dismissed in this window, kept until the app is closed (a reload keeps them). */
export function readDismissed(): Set<string> {
	try {
		const kept = JSON.parse(sessionStorage.getItem(DISMISSED_KEY) ?? "[]");
		return new Set(Array.isArray(kept) ? kept.filter((one): one is string => typeof one === "string") : []);
	} catch (error) {
		return new Set();
	}
}

export function keepDismissed(dismissed: ReadonlySet<string>): void {
	try {
		sessionStorage.setItem(DISMISSED_KEY, JSON.stringify([...dismissed]));
	} catch (error) {
		// storage refused: dismissed until this page is reloaded
	}
}
