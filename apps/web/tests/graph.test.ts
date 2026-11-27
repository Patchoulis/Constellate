/** The interface's pure logic: the community tree, the layout, labels. Run with `npm test`. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Community, GraphData } from "@rp/contracts";
import { wrapLabel } from "@/components/graph-canvas";
import { communityLabel, communityTree, groupsAt, treeDepth } from "@/lib/community";
import { placeLabels, type LabelBox } from "@/lib/labels";
import { layoutGraph } from "@/lib/layout";
import { hsl, titleCase } from "@/lib/utils";

/** A community of the given papers, optionally split into halves. */
function community(id: number, members: number[], children: Community[] = [], name: string | null = null): Community {
	return {
		id,
		tree_path: `c${id}`,
		size: members.length,
		name,
		description: null,
		keywords: ["time series", "llms", "forecasting"],
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

const range = (from: number, to: number) => Array.from({ length: to - from }, (_, i) => from + i);

/** Two communities of 60 and 40 papers; the first is split 35 / 25, and its first half again 20 / 15. */
function fixture() {
	const deep = [community(0, range(0, 20)), community(1, range(20, 35))];
	const halves = [community(0, range(0, 35), deep), community(1, range(35, 60))];
	const roots = [community(0, range(0, 60), halves), community(1, range(60, 100))];
	roots[0].closest_communities = [{ id: 1, cosine: 0.4 }];
	roots[1].closest_communities = [{ id: 0, cosine: 0.4 }];
	const graph: GraphData = {
		nodes: range(0, 100).map((i) => ({ id: `p${i}`, title: `Paper ${i}`, community: i < 60 ? 0 : 1, x: i < 60 ? -0.5 : 0.5, y: 0, ai: false, acceptance: null })),
		edges: [],
	};
	return { graph, tree: communityTree(roots) };
}

test("a title is capitalised, and abbreviations keep their own capitals", () => {
	assert.equal(titleCase("time series · llms"), "Time Series · LLMs");
	assert.equal(titleCase("vision-language lora"), "Vision-Language LoRA");
});

test("a written name is shown as written; without one, the two leading terms are capitalised", () => {
	assert.equal(communityLabel(community(0, [0], [], "Theory of Bandits and RL")), "Theory of Bandits and RL");
	assert.equal(communityLabel(community(0, [0])), "Time Series · LLMs");
});

test("colours wrap around the hue circle", () => {
	assert.equal(hsl(370), hsl(10));
	assert.equal(hsl(-10), hsl(350));
});

test("the tree links parents, children and siblings, and sub-communities stay near their parent's colour", () => {
	const { tree } = fixture();
	const [first, second] = tree;
	assert.equal(first.parent, null);
	assert.equal(first.children.length, 2);
	assert.equal(first.children[0].parent, first);
	assert.equal(first.children[0].siblings, first.children);
	assert.equal(first.siblings[1], second);
	assert.equal(first.children[0].depth, 1);
	assert.notEqual(first.children[0].hue, first.children[1].hue);
	assert.ok(Math.abs(first.children[0].hue - first.hue) <= 30);
	assert.notEqual(first.hue, second.hue);
	assert.equal(treeDepth(tree), 2);
});

test("each level of detail groups every paper exactly once", () => {
	const { tree } = fixture();
	for (const level of [0, 1, 2, 3]) {
		const groups = groupsAt(tree, level);
		const papers = groups.flatMap((node) => node.community.members).sort((a, b) => a - b);
		assert.deepEqual(papers, range(0, 100), `level ${level}`);
	}
	assert.deepEqual(groupsAt(tree, 0).map((node) => node.community.size), [60, 40]);
	assert.deepEqual(groupsAt(tree, 1).map((node) => node.community.size), [35, 25, 40]); // an unsplit community carries on
	assert.deepEqual(groupsAt(tree, 2).map((node) => node.community.size), [20, 15, 25, 40]);
});

test("the layout places every paper inside its community, and communities do not overlap", () => {
	const { graph, tree } = fixture();
	const layout = layoutGraph(graph, tree);
	assert.equal(layout.points.length, 100);
	const [a, b] = tree.map((node) => layout.discs.get(node)!);
	assert.ok(Math.hypot(a.x - b.x, a.y - b.y) > a.r + b.r, "community discs overlap");
	for (const node of tree) {
		const disc = layout.discs.get(node)!;
		for (const index of node.community.members) {
			const point = layout.points[index];
			assert.ok(Math.hypot(point.x - disc.x, point.y - disc.y) <= disc.r + 1e-6, `paper ${index} outside its community`);
		}
	}
	const spots = new Set(layout.points.map((point) => `${point.x.toFixed(3)},${point.y.toFixed(3)}`));
	assert.equal(spots.size, 100, "two papers share a spot");
});

test("a sub-community is one patch: its papers are nearer its own centre than its sibling's", () => {
	const { graph, tree } = fixture();
	const layout = layoutGraph(graph, tree);
	const [left, right] = tree[0].children.map((node) => ({ node, disc: layout.discs.get(node)! }));
	for (const [mine, other] of [[left, right], [right, left]] as const) {
		const nearer = mine.node.community.members.filter((index) => {
			const point = layout.points[index];
			return Math.hypot(point.x - mine.disc.x, point.y - mine.disc.y) < Math.hypot(point.x - other.disc.x, point.y - other.disc.y);
		});
		assert.ok(nearer.length >= 0.9 * mine.node.community.size, "a sub-community is scattered through its sibling");
	}
});

test("the layout is the same every time", () => {
	const first = layoutGraph(fixture().graph, fixture().tree);
	const again = layoutGraph(fixture().graph, fixture().tree);
	assert.deepEqual(first.points, again.points);
});

test("a label is cut only when it does not fit, and then says so", () => {
	const name = "Efficient Transformer Architectures for LLMs: Sparse Attention and Adaptive Compute";
	const roomy = wrapLabel(name, 40, 6);
	assert.equal(roomy.join(" "), name);
	const tight = wrapLabel(name, 24, 2);
	assert.equal(tight.length, 2);
	assert.ok(tight[1].endsWith("…"));
	assert.ok(name.startsWith(tight.join(" ").replace("…", "")));
	assert.deepEqual(wrapLabel("Short", 24, 3), ["Short"]);
});

const overlaps = (a: { left: number; top: number }, b: { left: number; top: number }, boxes: LabelBox[], i: number, j: number) =>
	a.left < b.left + boxes[j].width && b.left < a.left + boxes[i].width && a.top < b.top + boxes[j].height && b.top < a.top + boxes[i].height;

test("hover labels stay inside the graph, even for a point in its corner", () => {
	const frame = { width: 800, height: 500 };
	const boxes: LabelBox[] = [
		{ x: 795, y: 495, width: 300, height: 80, gap: 14 },
		{ x: 2, y: 2, width: 200, height: 40, gap: 8, above: true },
	];
	placeLabels(frame, boxes).forEach((at, i) => {
		assert.ok(at.left >= 0 && at.top >= 0, "outside the top left");
		assert.ok(at.left + boxes[i].width <= frame.width && at.top + boxes[i].height <= frame.height, "outside the bottom right");
	});
});

test("labels of papers that are close together do not cover one another", () => {
	const frame = { width: 1000, height: 700 };
	const boxes: LabelBox[] = [
		{ x: 500, y: 350, width: 300, height: 90, gap: 14 },
		...[0, 1, 2, 3, 4, 5].map((i) => ({ x: 505 + 6 * i, y: 352 + 4 * i, width: 220, height: 44, gap: 8, above: true })),
	];
	const places = placeLabels(frame, boxes);
	for (let i = 0; i < places.length; i++)
		for (let j = i + 1; j < places.length; j++) assert.ok(!overlaps(places[i], places[j], boxes, i, j), `labels ${i} and ${j} overlap`);
});

test("a lone label goes below and to the right of its point, and above when asked", () => {
	const frame = { width: 1000, height: 700 };
	assert.deepEqual(placeLabels(frame, [{ x: 100, y: 100, width: 200, height: 50, gap: 10 }]), [{ left: 110, top: 110 }]);
	assert.deepEqual(placeLabels(frame, [{ x: 100, y: 100, width: 200, height: 50, gap: 10, above: true }]), [{ left: 110, top: 40 }]);
});
