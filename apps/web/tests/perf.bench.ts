/**
 * Times the interface's hot paths on a made-up run as large as a big real one (12,500 papers, 24 communities
 * split twice, 22,000 links) and prints the numbers, so a later change can be compared with them. Not part of
 * `npm test`: timings depend on the machine, so nothing here passes or fails.
 *
 *     cd apps/web && npx tsx tests/perf.bench.ts
 *
 * What a frame costs in the browser (drawing, React) is not here: that needs the app open, see the numbers in the
 * commit that added the glow layer.
 */
import { communityTree, groupsAt } from "@/lib/community";
import { glowLayerGeometry } from "@/lib/glow-layer";
import { canvasColouring, MOST_GROUPS, spreadHues } from "@/lib/graph-colouring";
import { pickMarkedLabels, placeLabels, type LabelBox, type MarkedPlace } from "@/lib/labels";
import { layoutGraph } from "@/lib/layout";
import { nearestPaper } from "@/lib/pick";
import { generatedRun, randomFrom } from "./generated-run";

/** Run `work` `times` times after `warm` unmeasured runs; median and worst in milliseconds. */
function time(name: string, work: () => unknown, { times = 20, warm = 3 } = {}) {
	for (let i = 0; i < warm; i++) work();
	const took: number[] = [];
	for (let i = 0; i < times; i++) {
		const start = performance.now();
		work();
		took.push(performance.now() - start);
	}
	took.sort((a, b) => a - b);
	const ms = (value: number) => value.toFixed(value < 1 ? 3 : 1).padStart(8);
	console.log(`${name.padEnd(52)} median ${ms(took[Math.floor(times / 2)])} ms   worst ${ms(took[times - 1])} ms`);
}

const { graph, communities } = generatedRun();
console.log(`${graph.nodes.length} papers, ${communities.length} communities, ${graph.edges.length} links\n`);

time("community tree", () => communityTree(communities));
const roots = communityTree(communities);
time("layout, first in a fresh process", () => layoutGraph(graph, roots), { times: 1, warm: 0 });
time("layout", () => layoutGraph(graph, roots), { times: 10 });
const layout = layoutGraph(graph, roots);
time("groups of the finest level", () => groupsAt(roots, 2));

// Picking: the paper under the pointer, on every mouse move. Zoomed out (a dot ~1px, the margin 5px) and in.
const sizes = Float32Array.from(graph.nodes, (node) => ({ oral: 1.9, spotlight: 1.45, poster: 1 })[node.acceptance ?? "poster"]);
const pointers = Array.from({ length: 256 }, (_, order) => layout.points[(order * 7919) % layout.points.length]);
let next = 0;
const all = () => true;
time("pick a paper, zoomed out", () => nearestPaper(layout.points, sizes, pointers[next++ % 256], 2.4, 12, all), { times: 500 });
time("pick a paper, zoomed in", () => nearestPaper(layout.points, sizes, pointers[next++ % 256], 2.4, 0.5, all), { times: 500 });

// The glow layer of each level of detail: drawn once, kept if it is small enough.
for (const [level, glow] of [48, 34, 24].entries()) {
	const layer = glowLayerGeometry(layout.points, glow, 96);
	const size = glowLayerGeometry(layout.points, glow, 96, Infinity)!;
	console.log(`glow layer of level ${level}: ${size.width} x ${size.height} px, ${layer ? "kept" : "too large: halos drawn one by one"}`);
}
time("size the glow layer", () => glowLayerGeometry(layout.points, 48, 96));

// Colouring the graph by a feature's groups (lib/graph-colouring.ts): worked out once per colouring, then each
// frame of a pan where halos are drawn one by one chooses every paper's hue (components/graph-canvas.ts glowHue).
const ids = graph.nodes.map((node) => node.id);
const indexOf = new Map(ids.map((id, index) => [id, index]));
const colouring = {
	title: "a column",
	groups: spreadHues(MOST_GROUPS).map((hue, group) => ({
		name: `value ${group}`,
		hue,
		papers: ids.filter((_, index) => index % 11 === group),
	})),
};
time("colour the graph by 8 groups (once per colouring)", () => canvasColouring(colouring, indexOf, ids.length));
const painted = canvasColouring(colouring, indexOf, ids.length);
const finest = groupsAt(roots, 1);
const communityOf = new Int32Array(ids.length);
finest.forEach((node, group) => node.community.members.forEach((index) => (communityOf[index] = group)));
const communityHues = finest.map((node) => node.hue);
const huesOf = (paint: { of: ArrayLike<number>; hues: (number | null)[] }) => {
	let sum = 0;
	for (let index = 0; index < ids.length; index++) {
		const group = paint.of[index];
		const hue = group >= 0 ? paint.hues[group] : null;
		if (hue !== null) sum += hue;
	}
	return sum;
};
time("a frame's halo hues, by community", () => huesOf({ of: communityOf, hues: communityHues }), { times: 200 });
time("a frame's halo hues, by a feature's 8 groups", () => huesOf(painted), { times: 200 });
time("the reader's related papers: every link looked at", () => graph.edges.filter(([a, b]) => a === 4321 || b === 4321), { times: 100 });
console.log();

// Labels: the hovered paper, its linked papers, 12 marked papers and 24 of theirs, as on a busy screen.
const random = randomFrom(2);
const frame = { width: 1180, height: 900 };
const somewhere = () => ({ x: random() * frame.width, y: random() * frame.height });
const boxes: LabelBox[] = Array.from({ length: 37 }, (_, order) => ({
	...somewhere(),
	width: order === 0 ? 320 : 180 + random() * 44,
	height: order === 0 ? 96 : 30 + random() * 30,
	gap: order === 0 ? 14 : 8,
	above: order > 0,
	optional: order > 6,
}));
time("place 37 labels", () => placeLabels(frame, boxes), { times: 200 });
const marked: MarkedPlace[] = Array.from({ length: 30 }, (_, index) => ({
	index,
	...somewhere(),
	linked: Array.from({ length: 4 }, (_, other) => ({ index: 100 + index * 4 + other, ...somewhere() })),
}));
time("pick the labels of 30 marked papers", () => pickMarkedLabels(frame, marked, new Set(), { marked: 12, linked: 24 }), { times: 200 });
