/** The shapes the backend's HTTP API returns (apps/backend/rp/server.py, apps/engine) and the web app reads. */

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
	/** the job's number (absent from the Python backend): what `api.cancelJob` names */
	id?: number;
	/** what kind of work it is ("a run", "asking about papers") */
	kind?: string;
	state: JobState;
	/** name of the finished run, once state is "done" */
	result: string | null;
	error: string | null;
	/** log lines after the `since` the request asked for */
	log: string[];
	total: number;
	/** what is unsafe about how the engine listens (apps/engine); absent from the Python backend */
	warnings?: string[];
	/** what kind of engine this is; absent from the Python backend, which is always local */
	mode?: EngineMode;
	/** the settings the server decides, which the interface does not offer to change */
	server_owned?: (keyof Settings)[];
	/** how far a job that does many things has got; null for one that does not say */
	progress?: JobProgress | null;
	/** whether a key for the Claude API is set (never the key itself); absent from the Python backend */
	api_key_set?: boolean;
}

/** n of m done, and what it has spent so far on the Claude API, in US dollars. */
export interface JobProgress {
	done: number;
	of: number;
	usd: number;
}

/** On the user's own computer, or one person's server reached across a network (apps/engine/src/api/config.rs). */
export type EngineMode = "local" | "network";

/** What `GET /auth/session` says on a server (apps/engine/src/api/login). */
export interface SignInSession {
	signed_in: boolean;
	/** the ways in this server offers */
	methods: ("provider" | "password")[];
	/** the provider's name for its button ("Google"), when it is offered */
	provider: string | null;
	/** a password is still to be set with the setup code the server printed */
	setup: boolean;
	/** to the owner only: their address */
	email?: string | null;
	/** to the owner only: the session's token, sent back in X-Constellate-Csrf with every change */
	csrf?: string;
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
	/** the conference's own term, for orals and spotlights; absent in runs saved before terms were kept */
	acceptance_term?: AcceptanceTerm | null;
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
	/** the conference's own term for it; absent from a backend older than terms */
	acceptance_term?: AcceptanceTerm | null;
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

/** A stretch of a kept reviews page's text, in characters (Unicode code points, as `Array.from(text)` counts
 *  them, not UTF-16 units): `start` included, `end` excluded. */
export interface Span {
	start: number;
	end: number;
}

/** A conference and year: what decides a review's fields and their scales. */
export interface Venue {
	/** "NeurIPS", "ICLR", "ICML"; others in capitals */
	conference: string;
	year: number;
}

/** Who signed a note. Reviewers are anonymous; a public commenter's name is not kept. */
export type Signer =
	| { role: "reviewer"; id: string }
	| { role: "authors" | "area_chair" | "senior_area_chair" | "program_chairs" | "ethics_reviewer" | "public" | "not_shown" };

/** What a reply answers; `note` is a review's index among the notes. A guess says it is one, and why. */
export type ReviewReply =
	| { answers: "submission" }
	| { answers: "named"; note: number }
	| { answers: "guessed"; note: number; basis: "same_reviewer" | "mentions_reviewer" | "follows_review" }
	| { answers: "not_known" };

/** One field of a note as the venue's form names it ("Strengths And Weaknesses"); "" for text before any field.
 *  `text` is exactly the kept page's text at `span`. */
export interface ReviewSection {
	name: string;
	text: string;
	span: Span;
}

export interface ReviewScore {
	field: string;
	value: number;
	/** what the score means, as the review says ("good"); "" when it says nothing */
	meaning: string;
}

/** One note of a paper's discussion on OpenReview, as read from the page the user opened (engine text/discussion.rs). */
export type ReviewNote = (
	| { kind: "official_review"; scores: ReviewScore[]; confidence: ReviewScore | null }
	| { kind: "decision"; decision: string }
	| {
			kind:
				| "meta_review"
				| "author_response"
				| "reviewer_comment"
				| "chair_comment"
				| "comment"
				| "public_comment"
				| "ethics_review"
				| "withdrawal";
	  }
) & {
	signer: Signer;
	/** the heading as written */
	heading: string;
	span: Span;
	sections: ReviewSection[];
	reply: ReviewReply;
};

/** The lowest and highest score a field can be given at a venue in a year. "assumed": the venue or year is not
 *  known to the engine, so the field's usual scale is taken: an estimate. */
export interface Scale {
	minimum: number;
	maximum: number;
	source: "published" | "assumed";
}

/** One scored field of one paper; every figure is on `scale`, never comparable with another scale's. */
export interface FieldFigures {
	field: string;
	scale: Scale;
	reviews: number;
	mean: number;
	median: number;
	minimum: number;
	maximum: number;
	/** standard deviation of the reviews' scores */
	spread: number;
	/** the mean's position within the scale, 0 to 1: normalised, for comparing venues only */
	normalised_mean: number;
	/** a mean in which each review counts as its reviewer's confidence, over the reviews that gave one */
	confidence_weighted: { mean: number; reviews: number } | null;
	/** scores outside the scale (a venue that changed its form), counted as given */
	outside_scale: number;
}

/** How far a paper's reviewers were apart on their overall judgement (two reviews or more). */
export interface Disagreement {
	field: string;
	scale: Scale;
	spread: number;
	range: number;
	/** the spread as a share of the scale's width: normalised, for comparing venues */
	normalised_spread: number;
}

export interface PaperFigures {
	paper_id: string;
	venue: Venue | null;
	reviews: number;
	fields: FieldFigures[];
	disagreement: Disagreement | null;
}

/** `GET /api/papers/<id>/reviews`: a paper's kept discussion and what its scores come to (engine api/reviews.rs). */
export interface PaperReviews {
	paper_id: string;
	/** the address of the page the reader showed */
	address: string;
	/** when the page was read, UTC */
	read_at: string;
	/** the version of the reader that read it */
	parser: number;
	/** the venue the page names */
	venue: Venue | null;
	notes: ReviewNote[];
	figures: PaperFigures;
}

/** `POST /api/papers/<id>/reviews`: what came of a page handed over, and what is kept now. */
export interface ReviewsRead {
	outcome: "kept" | "kept_fuller" | "not_its_page" | "no_reviews";
	/** the scores alone, as the old scores-only route answered them */
	ratings: Ratings | null;
	reviews: PaperReviews | null;
}

export interface RankedPaper {
	paper_id: string;
	venue: Venue | null;
	/** its overall judgement on its own scale */
	overall: FieldFigures;
	/** what it was ranked by, normalised to its scale (0 to 1) so that venues compare */
	normalised: number;
}

/** The per-paper mean of one field at one venue and year, over a community's or run's papers. */
export interface VenueRatings {
	venue: Venue;
	field: string;
	scale: Scale;
	papers: number;
	mean: number;
	median: number;
	lowest: number;
	highest: number;
	/** one per unit of the scale: papers whose mean is from `from` up to `to` (the last includes the top) */
	bins: { from: number; to: number; papers: number }[];
}

/** A place a theme was said: the sentence of a review's section, in the paper's kept page text. */
export interface ThemeSource {
	paper_id: string;
	/** the review's index among the paper's notes */
	note: number;
	signer: Signer;
	section: string;
	span: Span;
	excerpt: string;
}

/** A term reviewers often use when they praise (or criticise), counted without a language model. */
export interface ReviewTheme {
	term: string;
	/** praising (criticising) sections that use it */
	sections: number;
	papers: number;
	/** its share of those sections, 0 to 1 */
	share: number;
	/** how many times more often it is in them than in the reviews' sections overall */
	distinctiveness: number;
	sources: ThemeSource[];
	more_sources: number;
}

/** `GET /api/runs/<run>/reviews/summary` and `.../communities/<tree path>/reviews/summary`. */
export interface ReviewSummary {
	papers: number;
	/** of them, whose reviews page was read: reviews are known only for pages the user opened */
	pages_read: number;
	papers_with_reviews: number;
	reviews: number;
	ratings: VenueRatings[];
	/** papers with reviews whose venue is not known, left out of `ratings` */
	papers_without_venue: number;
	most_disagreed: RankedPaper[];
	most_agreed: RankedPaper[];
	highest_rated: RankedPaper[];
	praise: ReviewTheme[];
	criticism: ReviewTheme[];
	/** by paper id, the title of every paper it ranks or quotes */
	titles: Record<string, string>;
}

/** How a conference paper was accepted, in tiers that hold across conferences; best first. */
export type Acceptance = "oral" | "spotlight" | "poster";

/** How the conference itself named a paper's acceptance (rp/tiers.py, docs/acceptance-tiers.md). */
export interface AcceptanceTerm {
	/** the conference and year: "CVPR 2024" */
	conference: string;
	/** its own word for it, as it writes it: "Highlight", "Notable top 5%" */
	called: string;
	/** what it is there: its share of the list's papers and what it involves */
	means: string;
	/** false when the term was never seen at that conference, so its tier was read from its wording */
	known: boolean;
}

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
	/** the prompt read at the full rate */
	input_tokens: number;
	output_tokens: number;
	/** read from and written to the prompt cache, each billed at its own rate (absent from older backends) */
	cache_read_tokens?: number;
	cache_write_tokens?: number;
	usd: number;
	unpriced_calls: number;
}

/**
 * Where something is in a paper (engine text/passages.rs). `page` and `end_page` are the PDF's own page numbers
 * from 1 (what `#page=N` opens), not those the paper prints. `start` counts characters into the text of `page`,
 * `end` into the text of `end_page` (the same page unless a quote runs on to the next). Characters are Unicode
 * code points, not UTF-16 units: slice with `Array.from(text)` (lib/evidence.ts `codePoints`), not `slice`.
 */
export interface PassageAddress {
	page: number;
	/** the section's heading as the paper writes it ("3.2. Results"); "" before the first heading */
	section: string;
	/** within the section, from 0 (the heading itself is 0 when there is one) */
	paragraph: number;
	start: number;
	end_page: number;
	end: number;
}

export type PassageKind = "title" | "heading" | "text" | "figure_caption" | "table_caption" | "equation" | "footnote" | "fragment";

export type PassagePart = "front" | "body" | "appendix" | "checklist";

/** One paragraph (or heading, caption, equation) of a page. */
export interface Passage {
	kind: PassageKind;
	part: PassagePart;
	/** "continues" when it carries on a paragraph from the page before */
	flow: "starts" | "continues";
	address: PassageAddress;
	text: string;
	/** its sentences, as places in its page's text (characters, like `address.start`) */
	sentences: { start: number; end: number }[];
}

/**
 * A paper's text made addressable, from its PDF: `GET /api/papers/<id>/passages`. The reference list is in the
 * pages' text but not among the passages. Addresses are stable for one `reader` version.
 */
export interface PaperPassages {
	reader: number;
	pages: { number: number; text: string }[];
	passages: Passage[];
}

/** How a quote was found: letter for letter (spacing, hyphens, ligatures, quote marks and case aside), or with a
 *  few letters different. */
export type QuoteMatch = { kind: "exact" } | { kind: "close"; differences: number };

/** `POST /api/papers/<id>/locate` with `{ quote }`: where the quote is in the paper's text, in its own words. */
export interface LocateRequest {
	quote: string;
}

export type LocateAnswer = { found: true; address: PassageAddress; wording: string; matched: QuoteMatch } | { found: false };

/** How a piece of evidence was established. There is no variant for an unchecked quote: none is ever sent. */
export type Established =
	| { kind: "verified"; address: PassageAddress; matched: QuoteMatch }
	/** found in the title or abstract only, at `start..end` (code points) of it: the PDF has not been read */
	| { kind: "abstract"; field: "title" | "abstract"; start: number; end: number; matched: QuoteMatch };

/** "This statement rests on that place in that paper" (engine evidence.rs); shown with components/evidence-link.tsx. */
export interface Evidence {
	paper: string;
	/** the paper's own words at that place, which is what is shown (never the model's copy) */
	quote: string;
	established: Established;
}

/** A statement a feature shows, with the evidence it rests on: every piece of it was found in its paper. */
export interface Cited<T> {
	statement: T;
	evidence: Evidence[];
}

/** Where the reader is asked to open a paper: at a place in it, with the words to highlight. */
export interface ReaderTarget {
	paper: string;
	address?: PassageAddress | null;
	/** the words to highlight, as the paper has them */
	quote?: string | null;
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

// ---- cited answers ----

/** How much of a run's or a community's papers a search covers (engine text/retrieval.rs `Coverage`). */
export interface SearchCoverage {
	papers: number;
	/** searched in their full text, read from their PDF */
	full_text: number;
	/** read, but only their first passages are searched: the index was full */
	capped: number;
	/** searched in their title and abstract only: no PDF kept */
	no_pdf: number;
	/** searched in their title and abstract only: their PDF could not be read */
	unreadable: number;
	/** not in the library: not searched */
	missing: number;
}

/** A passage found for a search or a question: the paper's own words there, as evidence one can click. */
export interface PassageFound {
	evidence: Evidence;
	/** does a statement of the answer shown cite it? (always false for a search) */
	cited: boolean;
}

/** While a scope's papers are being indexed (the first search of it reads the PDFs not read before). */
export interface SearchIndexing {
	state: "indexing";
	done: number;
	of: number;
}

/** `GET /api/runs/<run>/passages/found?q=...&community=<tree path>`: found without any model, at no cost. */
export type PassagesFound = { state: "ready"; query: string; coverage: SearchCoverage; passages: PassageFound[] } | SearchIndexing;

/** Tokens of a call, by how they are billed (engine usage/prices.rs `Tokens`). */
export interface AnswerTokens {
	input: number;
	output: number;
	cache_read: number;
	cache_write_5m: number;
	cache_write_1h: number;
}

/** What a call is likely to cost (engine model/language.rs `Estimate`): an estimate either way. */
export interface AnswerCost {
	model: string;
	calls: number;
	tokens: AnswerTokens;
	/** US dollars; null when the model's price is not known */
	usd: number | null;
	most_usd: number | null;
	counting: { kind: "assumed" | "measured"; chars_per_token: number };
}

/** `GET /api/runs/<run>/answers/estimate?q=...&community=...`: what asking would cost. Nothing is sent. */
export type AnswerEstimate =
	| {
			state: "ready";
			/** the question's id once asked */
			id: string;
			model: string;
			/** its answer is kept: reopening it costs nothing */
			kept: boolean;
			/** passages that fit in the prompt, and the papers they are from */
			passages: number;
			papers: number;
			coverage: SearchCoverage;
			prompt_cap_tokens: number;
			max_answer_tokens: number;
			/** null when kept */
			cost: AnswerCost | null;
			/** the most it can cost in US dollars, which the request must accept; null when the price is not known */
			most_usd: number | null;
			/** the most tokens it can take (its cap when the price is not known) */
			most_tokens: number;
			api_key_set: boolean;
	  }
	| SearchIndexing;

/** `POST /api/runs/<run>/answers`, sent when the user clicks the button showing the estimate. */
export interface AnswerRequest {
	question: string;
	/** a community's tree path; absent or null for the whole run */
	community?: string | null;
	/** the model the estimate shown was for: a request for another is refused */
	model: string;
	/** the `most_usd` the user was shown; null when the price is not known */
	accepted_usd: number | null;
}

/** The question's id, and the job asking it (null when its answer was kept). */
export interface AnswerStarted {
	id: string;
	job: number | null;
}

/** One line of `GET /api/runs/<run>/answers`, the questions asked of a run, the latest first. */
export interface AnswerSummary {
	id: string;
	question: string;
	community: string | null;
	model: string;
	asked_at: string;
	/** its answer is kept: reopening it costs nothing */
	answered: boolean;
}

/** One statement of an answer: a model's reading; its evidence is the papers' words. */
export interface AnswerStatement {
	sentence: string;
	about: "one_paper" | "several_papers";
}

export interface AnswerCounts {
	/** statements the model wrote */
	written: number;
	shown: number;
	/** left out: their quotes were not found in the papers they name */
	not_found: number;
	/** left out: not of the shape asked for */
	malformed: number;
}

/** Where a question stands: answered, the papers do not say, too little verified to show, being asked, or not answered. */
export type AnswerState = "answered" | "not_in_papers" | "unverified" | "asking" | "unanswered";

/** `GET /api/runs/<run>/answers/<id>`: a question and its kept answer, its quotes checked again. */
export interface AnswerView {
	id: string;
	question: string;
	community: string | null;
	model: string;
	asked_at: string;
	state: AnswerState;
	/** only when answered */
	statements: Cited<AnswerStatement>[];
	counts: AnswerCounts;
	/** the passages the model was given, best first */
	passages: PassageFound[];
	/** the papers the statements shown cite */
	cited_papers: string[];
}
