/**
 * A section of the window that stopped while being drawn (the graph, a panel, a feature's section), kept apart from
 * the rest: the section says so quietly, with the reason and a way to try again, and the rest of the window keeps
 * working (components/ui/section-boundary.tsx shows it).
 *
 *     shown --failed--> failed --retried--> shown (one try more)
 *     failed --changed--> shown (no tries)    what it is drawn from changed: what stopped it may be gone
 *
 * A section that stops again after being tried again says how often it was, so that a retry that cannot help says
 * it did not.
 */
import { errorLine } from "@/lib/shell/page-error";

export type Section = { state: "shown"; tries: number } | { state: "failed"; reason: string; tries: number };

export type SectionEvent = { kind: "failed"; error: unknown } | { kind: "retried" } | { kind: "changed" };

export const SECTION_SHOWN: Section = { state: "shown", tries: 0 };

export function sectionStep(section: Section, event: SectionEvent): Section {
	switch (event.kind) {
		case "failed":
			return { state: "failed", reason: errorLine(event.error as { message?: unknown } | null), tries: section.tries };
		case "retried":
			return section.state === "failed" ? { state: "shown", tries: section.tries + 1 } : section;
		case "changed":
			return SECTION_SHOWN;
	}
}

/** What a failed section says: that it could not be shown, the reason, and, after a retry that did not help, so. */
export function sectionLines(
	section: Extract<Section, { state: "failed" }>,
	what: string,
): { title: string; reason: string; again: string | null } {
	return {
		title: `${what.charAt(0).toUpperCase()}${what.slice(1)} could not be shown.`,
		reason: section.reason,
		again: section.tries > 0 ? `Tried again ${section.tries === 1 ? "once" : `${section.tries} times`}: it stopped the same way.` : null,
	};
}

/** Have the values a section is drawn from changed (compared one by one, as React compares an effect's deps)? */
export function changedKeys(before: readonly unknown[] | undefined, now: readonly unknown[] | undefined): boolean {
	const one = before ?? [];
	const other = now ?? [];
	return one.length !== other.length || one.some((value, index) => !Object.is(value, other[index]));
}
