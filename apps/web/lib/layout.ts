/**
 * Where everything sits. The renderer never computes a position; this module does, once per graph.
 *
 * Papers have no edges between them, so the picture is built from the tree of communities:
 *
 *  1. Communities: one disc per community, sized by its paper count. A short d3-force simulation, run to rest,
 *     packs the discs without overlap, each next to the communities whose mean embeddings are closest to its
 *     own, starting from where the embeddings' principal components put them.
 *  2. Papers: a community's disc is filled evenly with one spot per paper (a sunflower spiral).
 *  3. Sub-communities: the spots are dealt out down the tree. A group's spots are cut in two by a straight line,
 *     in proportion to the sizes of its two halves, and each half is drawn in a little toward its own centre.
 *     Every sub-community is therefore one contiguous patch of its parent, with a thin gap around it.
 *
 * So a community is one round cluster from afar, and up close it falls apart into the patches of its
 * sub-communities.
 */
import { forceCollide, forceLink, forceSimulation, forceX, forceY, type SimulationLinkDatum, type SimulationNodeDatum } from "d3-force";
import type { GraphData } from "@rp/contracts";
import type { CommunityNode } from "@/lib/community";

export interface Point {
	x: number;
	y: number;
}

/** Where a group is: its centre and the radius that contains all its papers. */
export interface Disc extends Point {
	r: number;
}

export interface Box {
	minX: number;
	minY: number;
	maxX: number;
	maxY: number;
}

export interface Layout {
	/** one position per graph node */
	points: Point[];
	discs: Map<CommunityNode, Disc>;
	bounds: Box;
}

const SPACING = 9; // a community's radius = SPACING * sqrt(papers)
const GAP = 34; // clear space between two community discs
const DRAW_IN = 0.9; // how far a sub-community's papers are pulled toward its centre (1 = not at all)
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

export const boxOf = (disc: Disc): Box => ({
	minX: disc.x - disc.r,
	minY: disc.y - disc.r,
	maxX: disc.x + disc.r,
	maxY: disc.y + disc.r,
});

interface Home extends SimulationNodeDatum {
	x: number;
	y: number;
	r: number;
}
interface Pull extends SimulationLinkDatum<Home> {
	strength: number;
}

/** Step 1: a disc per community. */
function placeCommunities(graph: GraphData, roots: CommunityNode[]): Home[] {
	const homes: Home[] = roots.map(({ community }) => {
		const sum = community.members.reduce((total, index) => ({ x: total.x + graph.nodes[index].x, y: total.y + graph.nodes[index].y }), {
			x: 0,
			y: 0,
		});
		const r = SPACING * Math.sqrt(community.size);
		return { x: (sum.x / community.size) * 2400, y: (sum.y / community.size) * 2400, r };
	});
	const pulls: Pull[] = roots.flatMap(({ community }) =>
		community.closest_communities
			.filter((other) => other.cosine > 0)
			.map((other) => ({ source: community.id, target: other.id, strength: other.cosine })),
	);
	forceSimulation(homes)
		.force(
			"similar",
			forceLink<Home, Pull>(pulls)
				.distance((pull) => (pull.source as Home).r + (pull.target as Home).r + GAP)
				.strength((pull) => 0.5 * pull.strength),
		)
		.force("collide", forceCollide<Home>((home) => home.r + GAP / 2).iterations(6))
		.force("x", forceX<Home>(0).strength(0.06))
		.force("y", forceY<Home>(0).strength(0.06))
		.stop()
		.tick(600);
	return homes;
}

/** Step 2: `count` spots spread evenly over a disc. */
function sunflower(disc: Disc, count: number): Point[] {
	return Array.from({ length: count }, (_, order) => {
		const distance = disc.r * Math.sqrt((order + 0.5) / count);
		const angle = order * GOLDEN_ANGLE;
		return { x: disc.x + Math.cos(angle) * distance, y: disc.y + Math.sin(angle) * distance };
	});
}

function discAround(spots: Point[]): Disc {
	const x = spots.reduce((sum, spot) => sum + spot.x, 0) / spots.length;
	const y = spots.reduce((sum, spot) => sum + spot.y, 0) / spots.length;
	return { x, y, r: Math.sqrt(Math.max(...spots.map((spot) => (spot.x - x) ** 2 + (spot.y - y) ** 2))) };
}

/** Step 3: give `spots` to the papers of `node`, cutting them between its sub-communities on the way down. */
function deal(node: CommunityNode, spots: Point[], layout: Layout) {
	layout.discs.set(node, discAround(spots));
	if (node.children.length === 0) {
		node.community.members.forEach((index, order) => (layout.points[index] = spots[order]));
		return;
	}
	// Cut across a direction that turns a quarter with every level, so patches do not become slivers.
	const angle = node.community.id * 0.7 + (node.depth * Math.PI) / 2;
	const along = (spot: Point) => spot.x * Math.cos(angle) + spot.y * Math.sin(angle);
	const ordered = [...spots].sort((a, b) => along(a) - along(b));
	let taken = 0;
	for (const child of node.children) {
		const share = ordered.slice(taken, taken + child.community.size);
		taken += child.community.size;
		const centre = discAround(share);
		deal(
			child,
			share.map((spot) => ({
				x: centre.x + (spot.x - centre.x) * DRAW_IN,
				y: centre.y + (spot.y - centre.y) * DRAW_IN,
			})),
			layout,
		);
	}
}

export function layoutGraph(graph: GraphData, roots: CommunityNode[]): Layout {
	const homes = placeCommunities(graph, roots);
	const edge = (pick: (home: Home) => number, most: (...values: number[]) => number) => most(...homes.map(pick));
	const layout: Layout = {
		points: graph.nodes.map(() => ({ x: 0, y: 0 })),
		discs: new Map(),
		bounds: {
			minX: edge((home) => home.x - home.r, Math.min),
			minY: edge((home) => home.y - home.r, Math.min),
			maxX: edge((home) => home.x + home.r, Math.max),
			maxY: edge((home) => home.y + home.r, Math.max),
		},
	};
	roots.forEach((node, index) => deal(node, sunflower(homes[index], node.community.size), layout));
	return layout;
}
