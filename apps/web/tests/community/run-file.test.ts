/** A run's files as the page reads them, whatever version wrote them. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { communityTree, communityLabel } from "@/lib/community/community";
import { graphOf, runOf } from "@/lib/community/run-file";
import { layoutGraph } from "@/lib/graph/layout";
import { count, distance, similarity } from "@/lib/units";
import { uniqueness } from "@/lib/community/community";

test("a run saved before many fields were added reads as a run, with what it lacks not known rather than thrown on", () => {
	// what an early communities.json could hold: no paths, no lists the panels read, no figures
	const run = runOf({ communities: [{ members: [0, 1], keywords: ["graphs", "trees"], children: [{ members: [0] }, { members: [1] }] }, {}] }, "old");
	assert.equal(run.name, "old");
	assert.equal(run.communities.length, 2);
	const [first, second] = run.communities;
	assert.equal(first.tree_path, "c0");
	assert.deepEqual(
		first.children.map((child) => child.tree_path),
		["c0.0", "c0.1"],
	);
	assert.equal(first.size, 2);
	assert.equal(first.name, null);
	assert.equal(first.sections, undefined);
	assert.deepEqual(first.closest_communities, []);
	assert.deepEqual(first.nearest_to_centroid, []);
	assert.deepEqual(first.mix, {});
	assert.deepEqual(first.acceptance, {});
	assert.equal(first.ai_flagged, 0);
	assert.equal(first.citations, null);
	assert.deepEqual(second.members, []);
	assert.deepEqual(second.keywords, []);
	// and every figure shown of it says it is not known
	assert.equal(distance(uniqueness(first), 3), "cosine distance not known");
	assert.equal(count(run.papers, "paper"), "0 papers");
	assert.ok(Number.isNaN(run.modularity));
	// the tree, labels and layout are made from it
	const roots = communityTree(run.communities);
	assert.equal(communityLabel(second), "Community c1");
	const layout = layoutGraph(graphOf({ nodes: [{ id: "a" }, { id: "b" }] }), roots);
	assert.equal(layout.discs.size, 4);
});

test("what is kept of a run is kept as it is: a run of this version reads the same", () => {
	const community = {
		id: 0,
		tree_path: "c0",
		size: 1,
		name: "Graphs",
		description: "About graphs.",
		keywords: ["graphs"],
		mix: { cs: 1 },
		mean_inner_similarity: 0.4,
		similarity_to_mean: 0.7,
		closest_communities: [{ id: 1, cosine: 0.2 }],
		nearest_to_centroid: [{ id: "p", title: "P", cosine: 0.9, in_community: true }],
		acceptance: { oral: 1 },
		ai_flagged: 0,
		members: [0],
		children: [],
		citations: { papers_read: 1, cited_by: 2, cited_by_inside: 0, cites: 3, cites_in_library: 1 },
	};
	const raw = { name: "new", papers: 1, model: "m", centered: true, clustering: "leiden", modularity: 0.5, ai_flagged: 0, communities: [community], citations_read: 1 };
	assert.deepEqual(runOf(raw, "new"), { ...raw, communities: [{ ...community, sections: undefined, sections_left_out: undefined }] });
});

test("sections that are not a list of sections leave the description plain; a cite without its evidence is left out", () => {
	const run = runOf(
		{
			communities: [
				{ sections: "Problem" },
				{
					sections: [
						{ heading: "Problem", text: "It is hard.", cites: [{ title: "T", evidence: { paper: "p", quote: "q", established: { kind: "abstract" } } }, { title: "U" }] },
						{ heading: 3 },
					],
				},
			],
		},
		"r",
	);
	assert.equal(run.communities[0].sections, undefined);
	assert.equal(run.communities[1].sections?.length, 1);
	assert.equal(run.communities[1].sections?.[0].cites.length, 1);
});

test("a graph's links to papers it does not have are left out, and papers with no place are put at the origin", () => {
	const graph = graphOf({
		nodes: [{ id: "a", x: Number.NaN, y: 0.5, acceptance: "award" }, { title: "No id" }, { id: "c", x: 0.1, y: 0.2, acceptance: "oral" }],
		edges: [
			[0, 2, 0.8],
			[0, 9, 0.5],
			[1, 1, 0.9],
			[-1, 0, 0.3],
			"not a link",
		],
	});
	assert.equal(graph.nodes.length, 3);
	assert.deepEqual(graph.edges, [[0, 2, 0.8]]);
	assert.equal(graph.nodes[0].x, 0);
	assert.equal(graph.nodes[0].acceptance, null);
	assert.equal(graph.nodes[1].id, "#1");
	assert.equal(graph.nodes[2].acceptance, "oral");
	assert.equal(similarity(graphOf({ nodes: [{ id: "a" }, { id: "b" }], edges: [[0, 1]] }).edges[0][2]), "cosine similarity not known");
	assert.deepEqual(graphOf(null), { nodes: [], edges: [] });
});
