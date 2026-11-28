/**
 * A run's two files as the page can draw them. The engine serves `communities.json` and `graph.json` as they were
 * written, and a run saved by an older version (the Python one, or 0.1 to 0.2.2) lacks fields that were added
 * since; one written halfway or by a newer version may hold what this one does not expect. Every field the page
 * reads is given here a value of its type, once, where the files are read, so that no panel finds a list that is
 * not one or a count that is undefined while it is drawn. A figure that is missing stays not known (NaN, which
 * lib/units.ts says is not known), never 0.
 */
import type { Community, CommunityCitations, CommunitySection, GraphData, GraphNode, NearestPaper, RunResult } from "@rp/contracts";

type Raw = Record<string, unknown>;

const record = (value: unknown): Raw => (value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Raw) : {});
const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const text = (value: unknown, otherwise = ""): string => (typeof value === "string" ? value : otherwise);
const textOrNull = (value: unknown): string | null => (typeof value === "string" ? value : null);
/** A figure: not known (NaN) when it is not a number. */
const figure = (value: unknown): number => (typeof value === "number" ? value : Number.NaN);
/** A count: 0 when it is not a number, for counts that are only ever added up ("how many of its papers ..."). */
const tally = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : 0);

function counts(value: unknown): Record<string, number> {
	return Object.fromEntries(Object.entries(record(value)).filter((entry): entry is [string, number] => typeof entry[1] === "number"));
}

function nearest(value: unknown): NearestPaper[] {
	return list(value).flatMap((one) => {
		const paper = record(one);
		if (typeof paper.id !== "string") return [];
		return [{ id: paper.id, title: text(paper.title, paper.id), cosine: figure(paper.cosine), in_community: paper.in_community === true }];
	});
}

/** Sections kept only when they are a list of sections with their texts: else the description is plain. */
function sections(value: unknown): CommunitySection[] | undefined {
	if (!Array.isArray(value)) return undefined;
	return value.flatMap((one) => {
		const section = record(one);
		if (typeof section.heading !== "string" || typeof section.text !== "string") return [];
		const cites = list(section.cites).filter((cite): cite is CommunitySection["cites"][number] => {
			const evidence = record(record(cite).evidence);
			return (
				typeof evidence.paper === "string" && typeof evidence.quote === "string" && typeof record(evidence.established).kind === "string"
			);
		});
		return [{ heading: section.heading, text: section.text, cites: cites.map((cite) => ({ ...cite, title: text(cite.title) })) }];
	});
}

function community(value: unknown, index: number, parentPath: string | null): Community {
	const raw = record(value);
	const members = list(raw.members).filter((member): member is number => Number.isInteger(member) && (member as number) >= 0);
	const treePath = text(raw.tree_path) || (parentPath === null ? `c${index}` : `${parentPath}.${index}`);
	const described = sections(raw.sections);
	return {
		...(raw as Partial<Community>),
		id: Number.isInteger(raw.id) ? (raw.id as number) : index,
		tree_path: treePath,
		size: typeof raw.size === "number" && Number.isFinite(raw.size) ? raw.size : members.length,
		name: textOrNull(raw.name),
		description: textOrNull(raw.description),
		...(described ? { sections: described } : { sections: undefined }),
		sections_left_out: typeof raw.sections_left_out === "number" ? raw.sections_left_out : undefined,
		keywords: list(raw.keywords).filter((keyword): keyword is string => typeof keyword === "string"),
		mix: counts(raw.mix),
		mean_inner_similarity: figure(raw.mean_inner_similarity),
		similarity_to_mean: figure(raw.similarity_to_mean),
		closest_communities: list(raw.closest_communities).flatMap((one) => {
			const other = record(one);
			return Number.isInteger(other.id) ? [{ id: other.id as number, cosine: figure(other.cosine) }] : [];
		}),
		nearest_to_centroid: nearest(raw.nearest_to_centroid),
		acceptance: counts(raw.acceptance),
		ai_flagged: tally(raw.ai_flagged),
		members,
		children: list(raw.children).map((child, at) => community(child, at, treePath)),
		citations: raw.citations && typeof raw.citations === "object" ? (raw.citations as CommunityCitations) : null,
	};
}

/** `GET /api/runs/{name}` as the page reads it, whatever the file it was read from holds. */
export function runOf(value: unknown, name: string): RunResult {
	const raw = record(value);
	return {
		...(raw as Partial<RunResult>),
		name: text(raw.name, name),
		papers: tally(raw.papers),
		model: text(raw.model),
		centered: raw.centered === true,
		clustering: text(raw.clustering),
		modularity: figure(raw.modularity),
		ai_flagged: tally(raw.ai_flagged),
		communities: list(raw.communities).map((one, index) => community(one, index, null)),
		citations_read: typeof raw.citations_read === "number" ? raw.citations_read : null,
	};
}

const ACCEPTANCE = new Set(["oral", "spotlight", "poster"]);

/**
 * `GET /api/runs/{name}/graph` as the page reads it. Every paper is kept at its place (the communities name papers by
 * it); a link that names a paper the graph does not have, or the same paper twice, is left out. A tier this version
 * does not know is no tier: the paper is drawn as an ordinary one.
 */
export function graphOf(value: unknown): GraphData {
	const raw = record(value);
	const nodes = list(raw.nodes).map((one, index): GraphNode => {
		const node = record(one);
		const id = text(node.id) || `#${index}`;
		const term = record(node.acceptance_term);
		return {
			id,
			title: text(node.title, id),
			community: Number.isInteger(node.community) ? (node.community as number) : -1,
			x: typeof node.x === "number" && Number.isFinite(node.x) ? node.x : 0,
			y: typeof node.y === "number" && Number.isFinite(node.y) ? node.y : 0,
			ai: node.ai === true,
			acceptance:
				typeof node.acceptance === "string" && ACCEPTANCE.has(node.acceptance) ? (node.acceptance as GraphNode["acceptance"]) : null,
			acceptance_term:
				typeof term.called === "string" && typeof term.conference === "string"
					? { conference: term.conference, called: term.called, means: text(term.means), known: term.known !== false }
					: null,
		};
	});
	const paper = (value: unknown): value is number => Number.isInteger(value) && (value as number) >= 0 && (value as number) < nodes.length;
	const edges = list(raw.edges).flatMap((one): GraphData["edges"] => {
		const [a, b, similarity] = list(one);
		return paper(a) && paper(b) && a !== b ? [[a, b, figure(similarity)]] : [];
	});
	return { nodes, edges };
}
