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

/** A community's title: its written name as it is, or its two most characteristic terms, capitalised. */
export function communityLabel(community: Community): string {
	return community.name ?? titleCase(community.keywords.slice(0, 2).join(" · "));
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
