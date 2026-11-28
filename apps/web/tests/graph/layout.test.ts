/** The layout of runs whose files are odd: written by an older version, halfway, or with NaN positions. Whatever
 *  they hold, every paper gets a finite place and every community a finite disc, and nothing throws. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Community, GraphData } from "@rp/contracts";
import { communityTree } from "@/lib/community/community";
import { layoutGraph, type Layout } from "@/lib/graph/layout";

function community(id: number, members: number[], children: Community[] = []): Community {
	return {
		id,
		tree_path: `c${id}`,
		size: members.length,
		name: null,
		description: null,
		keywords: ["graphs"],
		mix: {},
		mean_inner_similarity: 0.2,
		similarity_to_mean: 0.6,
		closest_communities: [],
		nearest_to_centroid: [],
		ai_flagged: 0,
		acceptance: {},
		members,
		children,
	};
}

function graphOf(papers: number, at: (index: number) => { x: number; y: number } = () => ({ x: 0, y: 0 })): GraphData {
	return {
		nodes: Array.from({ length: papers }, (_, index) => ({ id: `p${index}`, title: `Paper ${index}`, community: 0, ai: false, acceptance: null, ...at(index) })),
		edges: [],
	};
}

function assertFinite(layout: Layout) {
	for (const point of layout.points) assert.ok(Number.isFinite(point.x) && Number.isFinite(point.y), `a paper at ${point.x}, ${point.y}`);
	for (const disc of layout.discs.values()) assert.ok([disc.x, disc.y, disc.r].every(Number.isFinite), `a disc at ${JSON.stringify(disc)}`);
	assert.ok(Object.values(layout.bounds).every(Number.isFinite));
}

test("a community naming papers the graph does not have places the others, and every community still has a disc", () => {
	const roots = communityTree([community(0, [0, 1, 7, -1, 2.5]), community(1, [2, 3])]);
	const layout = layoutGraph(graphOf(4), roots);
	assertFinite(layout);
	assert.equal(layout.points.length, 4);
	assert.equal(layout.discs.size, 2);
});

test("a size that does not match the members, and a split whose halves do not add up, leave no paper at NaN", () => {
	const halves = [community(0, [0, 1, 2]), community(1, [3, 4, 5, 6, 7])];
	const parent = { ...community(0, [0, 1, 2, 3]), size: 9, children: halves };
	const layout = layoutGraph(graphOf(8), communityTree([parent]));
	assertFinite(layout);
	assert.equal(layout.discs.size, 3);
});

test("a community with no papers, and a sub-community with none, get a finite disc", () => {
	const roots = communityTree([community(0, [], []), community(1, [0, 1], [community(0, [0, 1]), community(1, [])])]);
	const layout = layoutGraph(graphOf(2), roots);
	assertFinite(layout);
	assert.equal(layout.discs.size, 4);
});

test("pulls to communities that are not there, or to itself, are left out instead of stopping the layout", () => {
	const one = community(0, [0, 1]);
	one.closest_communities = [
		{ id: 5, cosine: 0.8 },
		{ id: 0, cosine: 0.9 },
		{ id: 1, cosine: Number.NaN },
	];
	const two = community(1, [2]);
	two.closest_communities = [{ id: 0, cosine: 0.4 }];
	// an older file without the list at all
	const three = { ...community(2, [3]), closest_communities: undefined as unknown as Community["closest_communities"] };
	const layout = layoutGraph(graphOf(4), communityTree([one, two, three]));
	assertFinite(layout);
});

test("NaN and missing positions from the engine still give finite places", () => {
	const graph = graphOf(6, (index) => (index % 2 ? { x: Number.NaN, y: Number.NaN } : ({} as { x: number; y: number })));
	const layout = layoutGraph(graph, communityTree([community(0, [0, 1, 2]), community(1, [3, 4, 5])]));
	assertFinite(layout);
});

test("every paper at the same place, and a run of papers in no community, lay out to a finite box", () => {
	assertFinite(layoutGraph(graphOf(5, () => ({ x: 0.3, y: 0.3 })), communityTree([community(0, [0, 1, 2, 3, 4])])));
	assertFinite(layoutGraph(graphOf(3), []));
});
