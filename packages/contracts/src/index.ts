/** The shapes the backend's HTTP API returns (apps/backend/rp/server.py) and the web app reads. */

export interface Settings {
	embedding_model: string;
	center_embeddings: boolean;
	/** how communities are cut: one of Catalog.clustering */
	clustering: string;
	max_communities: number;
	min_community_size: number;
	min_split_modularity: number;
	/** how many times a community is split again into sub-communities */
	sub_levels: number;
	/** most links drawn from one paper to its closest papers */
	max_links: number;
	/** score abstracts for AI-style wording and tag those above the threshold */
	detect_ai: boolean;
	/** false: papers the AI-text detector flags are left out of a run (they are tagged either way) */
	include_ai_papers: boolean;
	/** which conference papers go into a run: one of Catalog.acceptance */
	acceptance_filter: string;
	/** papers whose authors' declarations raise one of these flags are left out (Catalog.flags) */
	exclude_flags: string[];
	describe: boolean;
	describe_model: string;
}

/** A step whose stored results a run can be told to ignore and produce again (rp/pipeline.py REDO). */
export type RedoStep = "download" | "embeddings" | "descriptions";

/** Every step that can be done again, in the order a run does them. */
export const REDO_STEPS: RedoStep[] = ["download", "embeddings", "descriptions"];

/** Conferences, or a submission-date range with optional categories (rp/selection.py checks it); or an earlier
 *  run to do again. Either way `redo` lists the steps to do again despite what is stored. */
export type RunRequest = (
	{ venues: string[]; include_workshops: boolean } | { start: string; end: string; categories: string[] } | { rerun: string }
) & { redo?: RedoStep[] };

/** The backend's job state machine (rp/jobs.py). */
export type JobState = "idle" | "running" | "done" | "failed";

export interface Status {
	state: JobState;
	/** name of the finished run, once state is "done" */
	result: string | null;
	error: string | null;
	/** log lines after the `since` the request asked for */
	log: string[];
	total: number;
}

export interface RunSummary {
	/** its folder: what the API calls it */
	name: string;
	/** what it is shown as; the name unless it was renamed */
	title: string;
	papers: number;
	communities: number;
	modularity: number;
}

export interface NearestPaper {
	id: string;
	title: string;
	cosine: number;
	in_community: boolean;
}

export interface Community {
	/** position among its siblings */
	id: number;
	/** its unique place in the run's tree: a root name, then ".0" or ".1" per split ("c7.0.1") */
	tree_path: string;
	size: number;
	name: string | null;
	description: string | null;
	keywords: string[];
	mix: Record<string, number>;
	/** mean adjacency weight between two members */
	mean_inner_similarity: number;
	/** mean cosine similarity of the members to the community's mean embedding */
	similarity_to_mean: number;
	/** cosine similarity between mean embeddings (overall mean removed), most similar sibling first */
	closest_communities: { id: number; cosine: number }[];
	nearest_to_centroid: NearestPaper[];
	/** how many of its papers were accepted as orals, spotlights, or ordinary papers */
	acceptance: Partial<Record<Acceptance, number>>;
	/** how many of its papers the AI-text detector flagged */
	ai_flagged: number;
	/** indices into the run's graph nodes */
	members: number[];
	/** its two sub-communities, or none when it does not split any further */
	children: Community[];
	/** what it cites and how often it is cited, from the reference lists Constellate has read; absent in runs
	 *  saved before these were counted */
	citations?: CommunityCitations | null;
}

/** A community's citations, among the run's papers whose reference lists were read (engine analysis/citations.rs). */
export interface CommunityCitations {
	/** of its papers, how many have had their reference list read: `cites` is counted over these */
	papers_read: number;
	/** citations its papers receive from the run's read papers, inside the community and outside it (each citing
	 *  and cited pair once) */
	cited_by: number;
	/** of those, from its own papers */
	cited_by_inside: number;
	/** distinct works its read papers cite, found or not */
	cites: number;
	/** of those, papers of the user's library */
	cites_in_library: number;
}

/** What a reference was found to be. */
export type CitationLink = { kind: "in_library"; paper: string } | { kind: "on_arxiv"; arxiv: string } | { kind: "unresolved" };

/** How far a reference list entry was understood: by an arXiv id or DOI, by its title, or only as written. */
export type ReferenceConfidence = "identified" | "title_parsed" | "raw_only";

/** A work next to the seed paper of a citation network. */
export interface CitationNeighbour {
	/** its name within the network: what `edges` and `similar` refer to */
	key: string;
	/** the title, or the entry as written when no title could be read from it */
	title: string;
	/** the first few, as written */
	authors: string[];
	authors_total: number;
	year: number | null;
	venue: string | null;
	link: CitationLink;
	/** where to read it outside the app, when known */
	url: string | null;
	confidence: ReferenceConfidence;
	/** how many papers Constellate has read cite it */
	cited_by: number;
	/** the entry as written in the seed's reference list; null for a citing paper */
	entry: string | null;
}

/**
 * The citation network around one paper, built from the reference lists of the papers Constellate has read
 * (engine analysis/citations.rs). Its references are complete once its own PDF is read; the papers citing it
 * are only those among the read ones, not the whole literature.
 */
export interface CitationNetwork {
	seed: string;
	/** has the seed's own reference list been read? */
	seed_state: "read" | "not_read";
	/** prior work: the works it cites, in the order listed */
	references: CitationNeighbour[];
	/** derivative work: read papers that cite it, the most cited first */
	citations: CitationNeighbour[];
	/** citations between neighbours (from cites to) */
	edges: { from: string; to: string }[];
	/** neighbours that share references (bibliographic coupling) or are cited together (co-citation) */
	similar: { a: string; b: string; shared_references: number; co_cited: number }[];
	totals: {
		references: number;
		references_in_library: number;
		references_on_arxiv: number;
		references_unresolved: number;
		citations: number;
		/** of those, how many are listed, the most cited first */
		citations_shown: number;
		/** of those, how many are papers of the run asked about (null when none was) */
		citations_in_run: number | null;
	};
	/** whose reference lists it comes from */
	papers_read: number;
	papers_in_library: number;
	/** the day it was built, YYYY-MM-DD */
	built: string;
}

export interface RunResult {
	name: string;
	papers: number;
	model: string;
	centered: boolean;
	clustering: string;
	modularity: number;
	ai_flagged: number;
	communities: Community[];
	/** of the run's papers, how many have had their reference list read: whose citations `Community.citations`
	 *  counts; absent in runs saved before these were counted */
	citations_read?: number | null;
}

export interface GraphNode {
	id: string;
	title: string;
	/** index of its community in RunResult.communities */
	community: number;
	/** rough position from the embeddings (two leading principal components), in [-1, 1] */
	x: number;
	y: number;
	/** flagged by the AI-text detector */
	ai: boolean;
	acceptance: Acceptance | null;
}

export interface GraphData {
	nodes: GraphNode[];
	/** [node index, node index, similarity]: each paper's few closest papers, each pair once */
	edges: [number, number, number][];
}

export interface Paper {
	id: string;
	title: string;
	abstract: string;
	authors: string[];
	published: string;
	comment: string;
	primary_category: string;
	categories: string[];
	venues?: string[];
	/** the paper's page: arXiv abstract, or the conference's link */
	url?: string;
	pdf_url?: string | null;
	/** what the AI-text detector found; null when the paper was never scored or is too short to score */
	ai: AiVerdict | null;
	acceptance: Acceptance | null;
	/** can the backend serve this paper's PDF (it is published somewhere a program may fetch from)? */
	readable: boolean;
	/** what its authors declare in the paper; null until it has been read from the PDF */
	declarations: Declarations | null;
	/** the scores its reviewers gave; null until its reviews page has been opened in the reader */
	reviews: Ratings | null;
}

/** Reviewers' scores, by field, as the conference names and scales them (rp/reviews.py). */
export interface Ratings {
	/** how many reviews gave scores */
	reviews: number;
	/** were the reviews told apart by their headings? If not, the count is a guess and the scores may include quoted ones */
	exact: boolean;
	/** field ("Rating", "Quality", "Soundness", ...) -> one score per review; overall judgement first */
	ratings: Record<string, { values: number[]; mean: number; /** what each score means, e.g. "fair" */ meanings: string[] }>;
}

/** How a conference paper was accepted, in tiers that hold across conferences; best first. */
export type Acceptance = "oral" | "spotlight" | "poster";

/** What the authors of a paper declare in it (rp/proceedings.py). */
export interface Declarations {
	/** the NeurIPS-style checklist, when the paper has one */
	checklist: { number: number; title: string; answer: "Yes" | "No" | "NA"; justification: string }[];
	/** free-text statements found by their headings: impact, ethics, use of language models, ... */
	statements: { kind: string; title: string; text: string }[];
	/** ids of the flags these raise (Catalog.flags) */
	flags: string[];
}

/** A statistical estimate, not proof: see rp/detect.py. */
export interface AiVerdict {
	/** higher = more machine-like */
	score: number;
	/** scores above this are flagged */
	threshold: number;
	flagged: boolean;
	/** the share of pre-LLM abstracts that score above the threshold */
	false_alarms: number;
	/** the words and phrases of this abstract that raised its score the most */
	markers: string[];
}

/** A point of an overview: a claim, what the cited papers share, and the papers. */
export interface OverviewPoint {
	title: string;
	insight: string;
	papers: { id: string; title: string }[];
}

/** What a community's papers have in common, written by Claude from all of them (rp/overview.py). */
export interface Overview {
	title: string;
	summary: string;
	sections: { heading: string; points: OverviewPoint[] }[];
	model: string;
	papers: number;
}

/** Where the writing of an overview stands. */
export interface OverviewStatus {
	state: JobState;
	overview: Overview | null;
	error: string | null;
}

/** One thing that can be picked from a menu, and a few words about it. */
export interface CatalogItem {
	/** what is stored and sent back */
	name: string;
	/** what a menu shows instead, when the name is an internal id */
	label?: string;
	note: string;
}

export interface CatalogGroup {
	field: string;
	items: CatalogItem[];
}

/** What the run form offers to pick from (rp/catalog.py). */
export interface Catalog {
	conferences: CatalogGroup[];
	categories: CatalogGroup[];
	embedding_models: CatalogItem[];
	acceptance: CatalogItem[];
	flags: CatalogGroup[];
	describe_models: CatalogItem[];
	clustering: CatalogItem[];
}

/** What the Claude API has cost (rp/usage.py). Counted from `since`: earlier calls were not recorded. */
export interface Usage {
	provider: { /** "Anthropic API", or the host requests are sent to */ name: string; /** are credentials set? */ ready: boolean };
	/** the model that writes names and overviews (Settings) */
	model: { name: string; label: string };
	/** the day the ledger starts, or null if nothing has been spent */
	since: string | null;
	/** the currency asked for and what a US dollar is worth in it; the dollar itself when no rate could be had */
	currency: { code: string; rate: number; date: string | null };
	/** everything below is in US dollars, the currency the API bills in */
	usd: number;
	calls: number;
	/** calls to a model whose price is not known: counted in tokens only, so `usd` is short by them */
	unpriced_calls: number;
	/** since the app was started */
	session_usd: number;
	session_calls: number;
	models: Record<string, UsageEntry>;
	/** what the calls were for: naming communities, overviews, ... */
	purposes: Record<string, UsageEntry>;
}

export interface UsageEntry {
	calls: number;
	input_tokens: number;
	output_tokens: number;
	usd: number;
	unpriced_calls: number;
}

export interface Library {
	papers: number;
	embeddings: Record<string, number>;
	/** whether abstracts get scored in the next run, and why (rp/detect.py `availability`) */
	ai_detector: { state: "off" | "untrained" | "ready"; why: string };
	/** what this computer can do: its NVIDIA graphics card, if any, and what that means for embedding */
	machine: { gpu: string | null; embedding: string };
	/** what was measured about the AI-text detector on papers it was not trained on; null until it is trained */
	ai_validation: {
		/** the models whose text it was trained to recognise */
		generators: string[];
		held_out: number;
		/** share of human abstracts flagged, by construction of the threshold */
		false_alarms: number;
		/** share caught of abstracts a model wrote from the title */
		written_caught: number;
		/** share caught of human abstracts a model polished */
		polished_caught: number;
		/** the words that most point to a model */
		machine_words: string[];
		/** share of arXiv abstracts flagged in a week of each year, if that check was run */
		by_year: Record<string, { abstracts: number; flagged: number }> | null;
	} | null;
}
