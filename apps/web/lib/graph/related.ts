/** The papers the graph links a paper to, as the reader lists them. */
import type { GraphData, GraphNode } from "@rp/contracts";

/**
 * The papers `paper` is linked to in `graph`, closest first; none when the graph does not have it (a paper opened
 * from a run that has since been read again without it). A link that names a paper the graph does not have (a file
 * of an older version, or written halfway) is left out rather than listed as a paper with no title.
 */
export function linkedPapers(graph: GraphData, paper: string | null): (GraphNode & { similarity: number })[] {
	const index = paper === null ? -1 : graph.nodes.findIndex((node) => node.id === paper);
	if (index < 0) return [];
	return graph.edges
		.flatMap(([a, b, similarity]) => {
			if (a !== index && b !== index) return [];
			const other = graph.nodes[a === index ? b : a];
			return other && other.id !== paper ? [{ ...other, similarity }] : [];
		})
		.sort((a, b) => b.similarity - a.similarity);
}
