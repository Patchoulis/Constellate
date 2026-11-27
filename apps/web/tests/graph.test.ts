/** The interface's pure logic: the community tree, the layout, labels. Run with `npm test`. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Community, GraphData } from "@rp/contracts";
import { wrapLabel } from "@/components/graph-canvas";
import { communityLabel, communityTree, groupsAt, treeDepth } from "@/lib/community";
import { pickMarkedLabels, placeLabels, type LabelBox, type MarkedPlace } from "@/lib/labels";
import { countKinds, shownPapers } from "@/lib/paper-filter";
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
		nodes: range(0, 100).map((i) => ({
			id: `p${i}`,
			title: `Paper ${i}`,
			community: i < 60 ? 0 : 1,
			x: i < 60 ? -0.5 : 0.5,
			y: 0,
			ai: false,
			acceptance: null,
		})),
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
	assert.deepEqual(
		groupsAt(tree, 0).map((node) => node.community.size),
		[60, 40],
	);
	assert.deepEqual(
		groupsAt(tree, 1).map((node) => node.community.size),
		[35, 25, 40],
	); // an unsplit community carries on
	assert.deepEqual(
		groupsAt(tree, 2).map((node) => node.community.size),
		[20, 15, 25, 40],
	);
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
	for (const [mine, other] of [
		[left, right],
		[right, left],
	] as const) {
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
	a.left < b.left + boxes[j].width &&
	b.left < a.left + boxes[i].width &&
	a.top < b.top + boxes[j].height &&
	b.top < a.top + boxes[i].height;

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

test("many marked papers' labels never cover one another: those with no free place are hidden", () => {
	const frame = { width: 600, height: 400 };
	// forty labels around nearly one point: far more than there is room for
	const boxes: LabelBox[] = Array.from({ length: 40 }, (_, i) => ({
		x: 300 + (i % 5),
		y: 200 + (i % 3),
		width: 180,
		height: 40,
		gap: 8,
		optional: true,
	}));
	const places = placeLabels(frame, boxes);
	const shown = places.map((at, i) => ({ at, i })).filter(({ at }) => !at.hidden);
	assert.ok(shown.length > 0 && shown.length < boxes.length, "some are shown, not all");
	for (const a of shown) {
		assert.ok(a.at.left >= 0 && a.at.top >= 0 && a.at.left + 180 <= frame.width && a.at.top + 40 <= frame.height, "inside the graph");
		for (const b of shown) if (a.i < b.i) assert.ok(!overlaps(a.at, b.at, boxes, a.i, b.i), `labels ${a.i} and ${b.i} overlap`);
	}
});

test("a hidden label takes no room, and a label that is not optional is always shown", () => {
	const frame = { width: 300, height: 120 };
	const big = { x: 150, y: 60, width: 280, height: 100, gap: 0 };
	const places = placeLabels(frame, [big, { ...big, width: 100, height: 30, optional: true }, { ...big, width: 100, height: 30 }]);
	assert.ok(!places[0].hidden);
	assert.equal(places[1].hidden, true);
	assert.ok(!places[2].hidden, "covers something, but must be shown");
	// a band across the top; a label too tall to go anywhere free; a small one below, where only the tall one
	// would have been in its way
	const room = { width: 400, height: 200 };
	const after = placeLabels(room, [
		{ x: 200, y: 10, width: 380, height: 80, gap: 0 },
		{ x: 200, y: 150, width: 380, height: 150, gap: 0, optional: true },
		{ x: 200, y: 150, width: 100, height: 30, gap: 4, optional: true },
	]);
	assert.equal(after[1].hidden, true);
	assert.deepEqual(after[2], { left: 204, top: 154 }, "shown where the hidden label would have been");
	const alone = placeLabels(frame, [{ x: 10, y: 10, width: 50, height: 20, gap: 4, optional: true }]);
	assert.deepEqual(alone, [{ left: 14, top: 14 }]);
});

test("marked papers are named first, then their linked papers in turn, each once and within the caps", () => {
	const frame = { width: 1000, height: 800 };
	const at = (index: number, x = 100, y = 100) => ({ index, x, y });
	const marked: MarkedPlace[] = [
		{ ...at(1), linked: [at(10), at(11), at(12), at(2)] }, // 2 is marked itself
		{ ...at(2), linked: [at(20), at(10), at(21)] }, // 10 is linked to both
		{ ...at(3, -50, 100), linked: [at(30), at(31, 2000, 100)] }, // 3 is off screen, and so is 31
	];
	const all = pickMarkedLabels(frame, marked, new Set(), { marked: 10, linked: 10 });
	assert.deepEqual(
		all.marked.map((place) => place.index),
		[1, 2],
	);
	// closest of each first: 10, 20, 30; then 11, (10 taken), (31 off screen); then 12, 21
	assert.deepEqual(
		all.linked.map((place) => place.index),
		[10, 20, 30, 11, 12, 21],
	);
	const capped = pickMarkedLabels(frame, marked, new Set(), { marked: 1, linked: 2 });
	assert.deepEqual(
		capped.marked.map((place) => place.index),
		[1],
	);
	// only the marked paper that is named names its neighbours
	assert.deepEqual(
		capped.linked.map((place) => place.index),
		[10, 11],
	);
	// what is labelled already (the hovered paper and its links) is not named twice
	const skipped = pickMarkedLabels(frame, marked, new Set([1, 20]), { marked: 10, linked: 10 });
	assert.deepEqual(
		skipped.marked.map((place) => place.index),
		[2],
	);
	assert.ok(!skipped.linked.some((place) => place.index === 20 || place.index === 1));
	assert.deepEqual(pickMarkedLabels(frame, [], new Set(), { marked: 10, linked: 10 }), { marked: [], linked: [] });
});

test("papers are shown or dimmed by kind, within the focused community", () => {
	const nodes: GraphData["nodes"] = [
		{ id: "a", title: "", community: 0, x: 0, y: 0, ai: false, acceptance: "oral" },
		{ id: "b", title: "", community: 0, x: 0, y: 0, ai: true, acceptance: "poster" },
		{ id: "c", title: "", community: 1, x: 0, y: 0, ai: false, acceptance: "spotlight" },
		{ id: "d", title: "", community: 1, x: 0, y: 0, ai: true, acceptance: null },
	];
	assert.deepEqual(countKinds(nodes), { oral: 1, spotlight: 1, poster: 1, none: 1, ai: 2, human: 2 });
	assert.equal(shownPapers(nodes, new Set(), null), null); // everything: nothing to dim
	assert.deepEqual([...shownPapers(nodes, new Set(["poster", "none"] as const), null)!], [0, 2]);
	assert.deepEqual([...shownPapers(nodes, new Set(["ai"] as const), null)!], [0, 2]);
	assert.deepEqual([...shownPapers(nodes, new Set(), [2, 3])!], [2, 3]);
	assert.deepEqual([...shownPapers(nodes, new Set(["human"] as const), [2, 3])!], [3]);
	assert.deepEqual([...shownPapers(nodes, new Set(["oral", "spotlight", "poster", "none"] as const), null)!], []);
});

test("a run with no papers lays out to a finite box, so the camera can be fitted to it", () => {
	const { bounds, points } = layoutGraph({ nodes: [], edges: [] }, []);
	assert.deepEqual(points, []);
	for (const value of Object.values(bounds)) assert.ok(Number.isFinite(value), `${value} is not a finite coordinate`);
});

test("a run of one paper places it at the centre of its community's disc", () => {
	const graph: GraphData = { nodes: [{ id: "a", title: "", community: 0, x: 0, y: 0, ai: false, acceptance: null }], edges: [] };
	const tree = communityTree([community(0, [0])]);
	const { bounds, points, discs } = layoutGraph(graph, tree);
	assert.equal(points.length, 1);
	for (const value of [...Object.values(bounds), points[0].x, points[0].y, discs.get(tree[0])!.r]) assert.ok(Number.isFinite(value));
	assert.ok(bounds.maxX > bounds.minX && bounds.maxY > bounds.minY, "the box has a size");
});
