/**
 * The tree of communities as the UI handles it: the engine's records plus what the UI adds (a hue, links up and
 * down the tree, siblings). Communities of a run are the roots; each may have two sub-communities, and so on.
 */
import type { Community } from "@rp/contracts";
import { titleCase } from "@/lib/utils";

export interface CommunityNode {
	community: Community;
	hue: number;
	/** 0 for a community of the run, +1 per level of sub-community */
	depth: number;
	parent: CommunityNode | null;
	children: CommunityNode[];
	/** the communities it was produced together with, itself included, indexed by Community.id */
	siblings: CommunityNode[];
}

/** One hue per community, spread by the golden angle so neighbouring ids never look alike. */
export function communityHue(id: number): number {
	return (210 + id * 137.508) % 360;
}

/** Sub-communities keep their parent's colour, shifted apart so the two can be told from each other. */
const HUE_SHIFT = 30;

function family(communities: Community[], parent: CommunityNode | null): CommunityNode[] {
	const depth = parent ? parent.depth + 1 : 0;
	const nodes: CommunityNode[] = [];
	for (const community of communities) {
		const hue = parent ? parent.hue + (community.id === 0 ? -1 : 1) * (HUE_SHIFT / depth) : communityHue(community.id);
		const node: CommunityNode = { community, hue, depth, parent, children: [], siblings: nodes };
		node.children = family(community.children, node);
		nodes.push(node);
	}
	return nodes;
}

export function communityTree(communities: Community[]): CommunityNode[] {
	return family(communities, null);
}

/** The node of `roots` (or of their sub-communities) at `treePath`: a community selected before the tree was made
 *  again (its citation figures counted since) is found in the new one. */
export function nodeAt(roots: CommunityNode[], treePath: string): CommunityNode | null {
	for (const node of roots) {
		if (node.community.tree_path === treePath) return node;
		const below = nodeAt(node.children, treePath);
		if (below) return below;
	}
	return null;
}

/** The groups one level of detail shows: every root followed `level` steps down, as far as each branch goes. */
export function groupsAt(roots: CommunityNode[], level: number): CommunityNode[] {
	return roots.flatMap((node) => (level === 0 || node.children.length === 0 ? [node] : groupsAt(node.children, level - 1)));
}

export function treeDepth(roots: CommunityNode[]): number {
	return roots.reduce((deepest, node) => Math.max(deepest, node.children.length ? 1 + treeDepth(node.children) : 0), 0);
}

/** A community's title: its written name as it is, or its two most characteristic terms, capitalised; its place in
 *  the tree for one that has neither (a run saved before terms were kept). */
export function communityLabel(community: Community): string {
	return community.name ?? (titleCase(community.keywords.slice(0, 2).join(" · ")) || `Community ${community.tree_path}`);
}

/**
 * How far a community's papers are, on average, from what the community is about: 1 minus the mean cosine
 * similarity of its papers to its mean embedding, a cosine distance. Near 0: its papers all do much the same thing.
 * Higher: its papers differ more from each other. Never below 0 (a similarity above 1 is rounding).
 */
export function uniqueness(community: Pick<Community, "similarity_to_mean">): number {
	return Math.max(0, 1 - community.similarity_to_mean);
}

export const UNIQUENESS_NOTE =
	"How much its papers differ from each other: the mean cosine distance of its papers from the community's mean embedding (1 minus their cosine similarity to it). Low: its papers are all doing much the same thing. High: the work in it is more varied.";

/**
 * Where a list's bars start and how much of the bar one unit is: from 0 for counts, or (`fromLowest`) from just under
 * the lowest figure, for figures that sit in a narrow band. Finite whatever the figures are: with none known yet (a
 * list sorted by citations before they are counted) every bar is empty, not NaN wide.
 */
export function barScale(values: readonly (number | null)[], fromLowest: boolean): { low: number; span: number } {
	const known = values.filter((value): value is number => value !== null && Number.isFinite(value));
	if (known.length === 0) return { low: 0, span: 1 };
	const low = fromLowest ? known.reduce((least, value) => Math.min(least, value), Infinity) * 0.9 : 0;
	const high = known.reduce((most, value) => Math.max(most, value), -Infinity);
	return { low, span: high - low || 1 };
}

/** The share of a bar a figure fills, 0 to 100: 0 for one not known. */
export function barWidth(value: number | null, { low, span }: { low: number; span: number }): number {
	if (value === null || !Number.isFinite(value)) return 0;
	return Math.min(100, Math.max(0, (100 * (value - low)) / span));
}

/** Of a community's papers, the share accepted as orals or spotlights; not known (NaN) for one with no papers. */
export function highlightShare(community: Pick<Community, "acceptance" | "size">): number {
	const singled = (community.acceptance.oral ?? 0) + (community.acceptance.spotlight ?? 0);
	return community.size > 0 ? singled / community.size : Number.NaN;
}
