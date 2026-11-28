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
import type { CommunityNode } from "@/lib/community/community";

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

/**
 * A community's papers that the graph has: what the layout places. A run's two files are written together, but a
 * community naming a paper the graph does not have (a file of an older version, one written halfway) must not stop
 * the page; such a member is left out rather than placed nowhere.
 */
function placedMembers(graph: GraphData, node: CommunityNode): number[] {
	return node.community.members.filter((index) => Number.isInteger(index) && index >= 0 && index < graph.nodes.length);
}

/** A number from the engine to compute with: one that is not (NaN, missing) counts as 0. */
const finite = (value: number) => (Number.isFinite(value) ? value : 0);

/** Step 1: a disc per community. */
function placeCommunities(graph: GraphData, roots: CommunityNode[]): Home[] {
	const homes: Home[] = roots.map((node) => {
		const members = placedMembers(graph, node);
		const sum = members.reduce(
			(total, index) => ({ x: total.x + finite(graph.nodes[index].x), y: total.y + finite(graph.nodes[index].y) }),
			{
				x: 0,
				y: 0,
			},
		);
		const r = SPACING * Math.sqrt(members.length);
		const of = members.length || 1; // a community with no papers starts at the origin
		return { x: (sum.x / of) * 2400, y: (sum.y / of) * 2400, r };
	});
	// The pulls name communities by their place among the roots (`Community.id`); one naming no root, or itself, is
	// left out: d3 refuses a link to a node it does not have, and would stop the page.
	const placeOf = new Map(roots.map((node, index) => [node.community.id, index]));
	const pulls: Pull[] = roots.flatMap(({ community }, index) =>
		(community.closest_communities ?? []).flatMap((other) => {
			const target = placeOf.get(other.id);
			return target === undefined || target === index || !(other.cosine > 0) ? [] : [{ source: index, target, strength: other.cosine }];
		}),
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

/** The disc around `spots`; with none (a community with no papers), a point at `fallback`, not one at NaN. */
function discAround(spots: Point[], fallback: Point): Disc {
	if (spots.length === 0) return { x: fallback.x, y: fallback.y, r: 0 };
	const x = spots.reduce((sum, spot) => sum + spot.x, 0) / spots.length;
	const y = spots.reduce((sum, spot) => sum + spot.y, 0) / spots.length;
	return { x, y, r: Math.sqrt(spots.reduce((most, spot) => Math.max(most, (spot.x - x) ** 2 + (spot.y - y) ** 2), 0)) };
}

/** Step 3: give `spots` to the papers of `node`, cutting them between its sub-communities on the way down. */
function deal(graph: GraphData, node: CommunityNode, spots: Point[], layout: Layout, fallback: Point) {
	const disc = discAround(spots, fallback);
	layout.discs.set(node, disc);
	if (node.children.length === 0) {
		// a spot for each paper; one more paper than spots (a size that does not match its members) stays where it is
		placedMembers(graph, node).forEach((index, order) => {
			if (spots[order]) layout.points[index] = spots[order];
		});
		return;
	}
	// Cut across a direction that turns a quarter with every level, so patches do not become slivers.
	const angle = node.community.id * 0.7 + (node.depth * Math.PI) / 2;
	const along = (spot: Point) => spot.x * Math.cos(angle) + spot.y * Math.sin(angle);
	const ordered = [...spots].sort((a, b) => along(a) - along(b));
	let taken = 0;
	for (const child of node.children) {
		const papers = placedMembers(graph, child).length;
		const share = ordered.slice(taken, taken + papers);
		taken += papers;
		const centre = discAround(share, disc);
		deal(
			graph,
			child,
			share.map((spot) => ({
				x: centre.x + (spot.x - centre.x) * DRAW_IN,
				y: centre.y + (spot.y - centre.y) * DRAW_IN,
			})),
			layout,
			disc,
		);
	}
}

/**
 * Where every paper and every community is. Whatever the run's files hold, every community of the tree gets a disc
 * and every paper a finite place: members the graph does not have, a size that does not match the members, a pull
 * to a community that is not there, NaN positions from the engine (all of which a file of an older version, or one
 * written halfway, can have) never stop the page or put a paper at NaN.
 */
export function layoutGraph(graph: GraphData, roots: CommunityNode[]): Layout {
	const homes = placeCommunities(graph, roots);
	// no community at all: a box around the origin, not one from Infinity to -Infinity that no camera can fit
	const edge = (pick: (home: Home) => number, most: (...values: number[]) => number) => (homes.length ? most(...homes.map(pick)) : 0);
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
	roots.forEach((node, index) => deal(graph, node, sunflower(homes[index], placedMembers(graph, node).length), layout, homes[index]));
	return layout;
}
