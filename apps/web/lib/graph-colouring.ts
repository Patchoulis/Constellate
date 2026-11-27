"use client";

import { createContext, useContext, useEffect } from "react";

/**
 * Colouring the graph by groups of papers: one small mechanism every feature shares. A feature that has split
 * papers into groups (an extraction column's values, the stances of a question, the papers an answer cites) sets a
 * colouring; the graph then draws those papers in their groups' hues, dims every other paper and shows a legend.
 *
 * At most one colouring is on the graph at a time, and it is in exactly one of two states: none, or one owned by
 * the feature that set it. Another feature setting one replaces it; only its owner (or the legend's Clear) clears
 * it, so a section that goes away does not clear another's; a new run starts with none (the workspace, which holds
 * it, is mounted again for each run).
 */

/** One group: its name in the legend, its hue (null: a neutral grey, for "not stated", "not verified"...), its papers. */
export interface PaperGroup {
	name: string;
	hue: number | null;
	/** the papers' ids */
	papers: readonly string[];
}

export interface PaperColouring {
	/** who set it: a section ("extraction:<run>:<scope>", "stances:<run>:<scope>:<question>", "answer") */
	owner: string;
	/** which of its owner's colourings it is, when it has several (an extraction table's column) */
	variant?: string;
	/** what it shows, above the legend: "Dataset (extraction table)" */
	title: string;
	groups: readonly PaperGroup[];
}

/** A colouring as a feature works it out from its data; its owner and variant are the section's (`useColouringBy`). */
export type Colouring = Omit<PaperColouring, "owner" | "variant">;

export type ColouringEvent = { kind: "set"; colouring: PaperColouring } | { kind: "clear"; owner: string } | { kind: "clear_all" };

/** The one place the colouring changes. Clearing someone else's colouring changes nothing. */
export function colouringStep(current: PaperColouring | null, event: ColouringEvent): PaperColouring | null {
	switch (event.kind) {
		case "set":
			return event.colouring.groups.some((group) => group.papers.length > 0)
				? event.colouring
				: current?.owner === event.colouring.owner
					? null
					: current;
		case "clear":
			return current?.owner === event.owner ? null : current;
		case "clear_all":
			return null;
	}
}

/** The owner of the colouring by the papers a cited answer cites (components/ask.tsx `onCitedPapers`). */
export const ANSWER_OWNER = "answer";

/** Hues for groups that have no meaning of their own (an extraction column's values): far apart on the wheel. */
export function spreadHues(count: number): number[] {
	return Array.from({ length: count }, (_, index) => (index * 137.508 + 200) % 360);
}

/** Most groups a colouring shows: past this, the colours cannot be told apart; the rest are in "other". */
export const MOST_GROUPS = 8;

/**
 * What the canvas draws: for each paper of the graph the group it is drawn in (-1: none, dimmed), the groups'
 * hues, and the legend: for each group, how many of its papers are on the graph and how many are not in this run.
 * A paper in several groups (a list's items) is drawn in the first.
 */
export interface CanvasColouring {
	of: Int16Array;
	hues: (number | null)[];
	legend: { name: string; hue: number | null; shown: number; elsewhere: number }[];
}

export function canvasColouring(colouring: Colouring, indexOf: ReadonlyMap<string, number>, papers: number): CanvasColouring {
	const of = new Int16Array(papers).fill(-1);
	const legend = colouring.groups.map((group, number) => {
		let shown = 0;
		let elsewhere = 0;
		for (const id of new Set(group.papers)) {
			const index = indexOf.get(id);
			if (index === undefined) elsewhere += 1;
			else {
				shown += 1;
				if (of[index] === -1) of[index] = number;
			}
		}
		return { name: group.name, hue: group.hue, shown, elsewhere };
	});
	return { of, hues: colouring.groups.map((group) => group.hue), legend };
}

/** The colouring as the workspace holds it, for the graph and for the features that set it. */
export interface GraphColouring {
	colouring: PaperColouring | null;
	set: (colouring: PaperColouring) => void;
	clear: (owner: string) => void;
}

export const GraphColouringContext = createContext<GraphColouring | null>(null);

/** The workspace's colouring; null outside a workspace (nothing to colour: the features hide their button). */
export function useGraphColouring(): GraphColouring | null {
	return useContext(GraphColouringContext);
}

/**
 * A section's own colouring of the graph: which of its variants is shown (null: none of its own), and a toggle.
 * What the section set is cleared when it goes away (another community chosen, the panel closed); null outside a
 * workspace.
 */
export function useColouringBy(owner: string): {
	shown: string | null;
	toggle: (variant: string, build: () => Colouring) => void;
} | null {
	const colourings = useGraphColouring();
	const clear = colourings?.clear;
	useEffect(() => () => clear?.(owner), [clear, owner]);
	if (!colourings) return null;
	const mine = colourings.colouring?.owner === owner ? colourings.colouring : null;
	const shown = mine ? (mine.variant ?? "") : null;
	return {
		shown,
		toggle: (variant, build) => (shown === variant ? colourings.clear(owner) : colourings.set({ ...build(), owner, variant })),
	};
}
