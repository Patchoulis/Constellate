/**
 * A made-up run the size of a large real one, for timing the interface's hot paths (perf.bench.ts): papers in
 * communities split two levels deep, links mostly inside a community, titles of realistic length. The same
 * seed gives the same run.
 */
import type { Community, GraphData } from "@rp/contracts";

/** A small deterministic random number generator (mulberry32): the same seed, the same numbers. */
export function randomFrom(seed: number): () => number {
	let state = seed | 0;
	return () => {
		state = (state + 0x6d2b79f5) | 0;
		let t = Math.imul(state ^ (state >>> 15), 1 | state);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

const WORDS =
	"learning neural diffusion transformers attention graph reinforcement policy optimization robust adversarial generalization sparse contrastive language vision multimodal alignment reward scaling inference quantization federated privacy causal bayesian kernel stochastic convergence benchmark retrieval reasoning planning agents robotics segmentation".split(
		" ",
	);

function community(id: number, members: number[], depth: number, random: () => number): Community {
	const half = Math.floor(members.length * (0.35 + random() * 0.3));
	const children =
		depth < 2 && members.length > 40
			? [community(0, members.slice(0, half), depth + 1, random), community(1, members.slice(half), depth + 1, random)]
			: [];
	return {
		id,
		tree_path: `c${id}.${depth}`,
		size: members.length,
		name: null,
		description: null,
		keywords: [WORDS[id % WORDS.length], WORDS[(id + depth + 3) % WORDS.length], "models"],
		mix: {},
		mean_inner_similarity: 0.4,
		similarity_to_mean: 0.6,
		closest_communities: [],
		nearest_to_centroid: [],
		acceptance: {},
		ai_flagged: 0,
		members,
		children,
	};
}

/** `papers` papers in `communities` communities (each split twice more), and about `links` links. */
export function generatedRun({ papers = 12_500, communities = 24, links = 22_000, seed = 1 } = {}): {
	graph: GraphData;
	communities: Community[];
} {
	const random = randomFrom(seed);
	const weights = Array.from({ length: communities }, () => 0.3 + random() * 1.7);
	const total = weights.reduce((sum, weight) => sum + weight, 0);
	const sizes = weights.map((weight) => Math.max(60, Math.round((weight / total) * papers)));
	sizes[communities - 1] += papers - sizes.reduce((sum, size) => sum + size, 0);
	const centres = sizes.map(() => ({ x: random() * 2 - 1, y: random() * 2 - 1 }));
	const nodes: GraphData["nodes"] = [];
	const memberLists = sizes.map((size, group) =>
		Array.from({ length: size }, () => {
			const index = nodes.length;
			let title = "Towards";
			const length = 60 + Math.floor(random() * 70);
			while (title.length < length) title += ` ${WORDS[Math.floor(random() * WORDS.length)]}`;
			const tier = random();
			nodes.push({
				id: `p${index}`,
				title,
				community: group,
				x: centres[group].x + (random() - 0.5) * 0.3,
				y: centres[group].y + (random() - 0.5) * 0.3,
				ai: random() < 1 / 3,
				acceptance: tier < 0.02 ? "oral" : tier < 0.08 ? "spotlight" : tier < 0.75 ? "poster" : null,
			});
			return index;
		}),
	);
	const roots = memberLists.map((members, id) => community(id, members, 0, random));
	for (const root of roots)
		root.closest_communities = [1, 2, 3].map((step) => ({ id: (root.id + step) % communities, cosine: 0.2 + random() * 0.5 }));
	const edges: GraphData["edges"] = [];
	const seen = new Set<string>();
	while (edges.length < links) {
		const a = Math.floor(random() * papers);
		const own = memberLists[nodes[a].community];
		const b = random() < 0.9 ? own[Math.floor(random() * own.length)] : Math.floor(random() * papers);
		const key = a < b ? `${a},${b}` : `${b},${a}`;
		if (a === b || seen.has(key)) continue;
		seen.add(key);
		edges.push([a, b, 0.55 + random() * 0.4]);
	}
	return { graph: { nodes, edges }, communities: roots };
}
