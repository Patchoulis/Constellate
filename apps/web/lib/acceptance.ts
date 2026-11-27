import type { Acceptance, AcceptanceTerm, GraphNode } from "@rp/contracts";

/**
 * What a paper's acceptance tag says. The words are the conference's own ("Highlight", "Notable top 5%"), the
 * colour is Constellate's tier, and the hover text says how the two meet. Runs saved before conferences' terms
 * were kept have only the tier, and say it in the tier's generic words.
 */

const GENERIC: Record<Acceptance, { called: string; means: string }> = {
	oral: {
		called: "Oral",
		means: "Accepted as an oral: the conference's committee picked it for a talk, which only the top few percent of accepted papers get.",
	},
	spotlight: {
		called: "Spotlight",
		means:
			"Accepted as a spotlight (or highlight): singled out by the conference's committee, a tier below an oral and above an ordinary acceptance.",
	},
	poster: { called: "Poster", means: "An ordinary acceptance." },
};

const AS = { oral: "an oral", spotlight: "a spotlight", poster: "an ordinary acceptance (a poster)" } satisfies Record<Acceptance, string>;

/** The tag's words: the conference's term, or the tier's name. */
export function tagText(tier: Acceptance, term?: AcceptanceTerm | null): string {
	return term?.called || GENERIC[tier].called;
}

/** The tag's hover text: "CVPR 2024's Highlight: 11.9% of the papers in its list; ... Counted as a spotlight." */
export function tagTitle(tier: Acceptance, term?: AcceptanceTerm | null): string {
	if (!term) return GENERIC[tier].means;
	return `${term.conference}'s ${term.called}: ${term.means}. Constellate counts it as ${AS[tier]}.`;
}

/**
 * The conferences' terms a tier stands for in these papers, each once, most papers first: what the "Orals" and
 * "Spotlights" chips include. Empty for a run saved before terms were kept.
 */
export function termsOf(nodes: GraphNode[], tier: Acceptance): string[] {
	const counts = new Map<string, number>();
	for (const node of nodes) {
		const called = node.acceptance === tier ? node.acceptance_term?.called : undefined;
		if (called) counts.set(called, (counts.get(called) ?? 0) + 1);
	}
	return [...counts].sort((a, b) => b[1] - a[1]).map(([called]) => called);
}
