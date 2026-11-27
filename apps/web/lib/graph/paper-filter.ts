import type { GraphNode } from "@rp/contracts";
import { termsOf } from "@/lib/acceptance";

/**
 * The kinds of paper the graph can show or dim, by two independent traits:
 *   how it was accepted    oral / spotlight / poster / none (not a conference paper)
 *   the AI-text estimate   ai / human
 * A paper is shown when neither of its two kinds is switched off.
 */
export const PAPER_KINDS = [
	{ kind: "oral", label: "Orals", note: "Papers the conference picked for a talk" },
	{ kind: "spotlight", label: "Spotlights", note: "Papers the conference singled out, a tier below an oral" },
	{ kind: "poster", label: "Posters", note: "Ordinary acceptances" },
	{ kind: "none", label: "Not from a conference", note: "Papers with no acceptance tier, such as arXiv papers" },
	{ kind: "ai", label: "AI detected", note: "Papers whose abstract the statistical AI-text detector flagged: an estimate, not proof" },
	{ kind: "human", label: "Not AI detected", note: "Papers the AI-text detector did not flag" },
] as const;

export type PaperKind = (typeof PAPER_KINDS)[number]["kind"];

const kindsOf = (node: GraphNode): [PaperKind, PaperKind] => [node.acceptance ?? "none", node.ai ? "ai" : "human"];

/** How many papers there are of each kind. */
export function countKinds(nodes: GraphNode[]): Record<PaperKind, number> {
	const counts: Record<PaperKind, number> = { oral: 0, spotlight: 0, poster: 0, none: 0, ai: 0, human: 0 };
	for (const node of nodes) for (const kind of kindsOf(node)) counts[kind] += 1;
	return counts;
}

/**
 * What each kind's chip says on hover. The tiers are Constellate's, so the orals' and spotlights' notes name the
 * conferences' own terms they include among these papers ("Spotlights: Spotlight, Highlight, Notable top 25%").
 */
export function kindNotes(nodes: GraphNode[]): Record<PaperKind, string> {
	const notes = Object.fromEntries(PAPER_KINDS.map(({ kind, note }) => [kind, note])) as Record<PaperKind, string>;
	for (const [tier, label] of [
		["oral", "Orals"],
		["spotlight", "Spotlights"],
	] as const) {
		const terms = termsOf(nodes, tier);
		if (terms.length) notes[tier] += `. ${label} here: ${terms.join(", ")}`;
	}
	return notes;
}

/**
 * The papers to show in full (the rest are dimmed), as indexes into `nodes`: those of `members` (all, when null)
 * whose kinds are not `hidden`. Null when that is every paper, so nothing needs dimming.
 */
export function shownPapers(nodes: GraphNode[], hidden: ReadonlySet<PaperKind>, members: number[] | null): Set<number> | null {
	if (!members && hidden.size === 0) return null;
	const candidates = members ?? nodes.map((_, index) => index);
	return new Set(candidates.filter((index) => kindsOf(nodes[index]).every((kind) => !hidden.has(kind))));
}
