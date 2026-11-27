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
	/** the language model's provider (ModelProvider.id); absent from the Python backend, which knows Claude only */
	describe_provider?: ProviderId;
	/** one of that provider's models, or an id typed for a provider that takes one */
	describe_model: string;
	/** how hard the model thinks; absent from the Python backend */
	describe_effort?: Effort;
}

/** How hard a language model thinks before it answers (apps/engine/src/model/language). */
export type Effort = "low" | "medium" | "high" | "xhigh" | "max";

/** A provider of language models (apps/engine/src/model/language/provider.rs). */
export type ProviderId =
	| "anthropic"
	| "openai"
	| "gemini"
	| "compatible"
	| "openrouter"
	| "xai"
	| "mistral"
	| "deepseek"
	| "groq"
	| "together"
	| "fireworks"
	| "cerebras"
	| "moonshot"
	| "minimax"
	| "zai"
	| "nvidia"
	| "huggingface"
	| "vercel"
	| "baseten"
	| "meta";

/** A model's price as the engine knows it: never 0 for one whose price is not known. */
export type ModelPrice =
	/** US dollars per million tokens, as published */
	| { kind: "known"; input: number; output: number; cache_read: number | null }
	| { kind: "not_known" }
	/** a server on this computer: no API fee */
	| { kind: "this_computer" };

/** One model a provider offers. */
export interface ModelEntry {
	id: string;
	label: string;
	note: string;
	/** the efforts it takes; none when it has no such setting */
	efforts: Effort[];
	price: ModelPrice;
}

/** What a provider can and cannot do (the engine's capability table). */
export interface ModelCapabilities {
	answers: "enforced" | "requested" | "json_mode" | "prompted";
	effort: "by_model" | "not_sent";
	caching: "marked" | "automatic_writes_billed" | "automatic" | "not_reported";
	counting: "endpoint" | "characters";
}

/** One provider, as `GET /api/models` tells it: never a key, of an address only its host. */
export interface ModelProvider {
	id: ProviderId;
	label: string;
	key: { variable: string; /** a server on this computer usually needs none */ needed: boolean; set: boolean };
	address: { variable: string | null; host: string | null; place: "provider" | "set" | "this_computer" | "not_set" };
	available: { state: "ready" } | { state: "unavailable"; /** what to do: "set OPENAI_API_KEY" */ why: string };
	/** listed only, listed or typed, or typed only (a server's own models) */
	ids: "listed" | "listed_or_typed" | "typed";
	capabilities: ModelCapabilities;
	/** what it cannot do, in plain words */
	limits: string[];
	models: ModelEntry[];
	/** the price of a model id typed rather than listed */
	price_of_typed: ModelPrice;
	/** where the facts about it were read */
	docs: string;
}

/** `GET /api/models` (apps/engine/src/api/models.rs). */
export interface ModelCatalog {
	providers: ModelProvider[];
	/** the model the settings choose */
	chosen: { provider: ProviderId; model: string; effort: Effort };
}

/** Tokens of a call or a batch, by how they are billed (engine usage/prices.rs `Tokens`). */
export interface BilledTokens {
	input: number;
	output: number;
	cache_read: number;
	cache_write_5m: number;
	cache_write_1h: number;
}

/** How prompt tokens were reckoned before anything was sent: from characters, at a ratio assumed or measured. */
export interface TokenCounting {
	kind: "assumed" | "measured";
	chars_per_token: number;
}

/**
 * What asking would cost, before anything is sent (engine model/language `Estimate`): the one shape every
 * feature's estimate has, whichever provider is chosen. An estimate, always.
 */
export interface AskingCost {
	/** the provider and model that would be asked: the estimate is at that provider's prices */
	provider: ProviderId;
	model: string;
	calls: number;
	/** likely tokens, by how they are billed */
	tokens: BilledTokens;
	/** likely and at most, in US dollars; null when the model's price is not known or it runs on this computer */
	usd: number | null;
	most_usd: number | null;
	/** tokens of every kind at most: the budget a model with no price is held to */
	most_tokens: number;
	counting: TokenCounting;
	/** how the shared text was counted: cached as the provider caches, or in full for every question */
	caching: ModelCapabilities["caching"];
	/** a server on this computer: no API fee */
	on_this_computer: boolean;
}

/** What a batch would cost (engine model/batch.rs `BatchEstimate`): its items, and what asking those not kept would cost. */
export interface BatchCost extends AskingCost {
	items: number;
	/** kept from before: they cost nothing */
	kept: number;
	to_ask: number;
}

/** What a request accepts of an estimate shown: its most in US dollars, or in tokens for a model with no price. */
export type AcceptedCost = { accepted_usd: number; accepted_tokens?: never } | { accepted_tokens: number; accepted_usd?: never };

/** A budget of the user's own: in US dollars, or in tokens for a model with no price. */
export type BudgetOf = { budget_usd: number; budget_tokens?: never } | { budget_tokens: number; budget_usd?: never };

/** The model an estimate was shown for: a request for another one (of another provider) is refused. */
export interface ShownModel {
	provider: ProviderId;
	model: string;
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

/**
 * What a finished job made (engine jobs.rs `Finished`): a run to open, a feature's answers about a subject (the
 * feature's own component shows them), or nothing to open.
 */
export type JobOutcome =
	{ kind: "run"; run: string } | { kind: "feature"; feature: FeatureName; subject: FeatureSubject } | { kind: "nothing" };

/** The features that keep the model's answers (engine library/answers.rs `Feature`). */
export type FeatureName = "extraction" | "citation_kinds" | "stances" | "answers" | "critique" | "explanation" | "claim_tree";

/** What a feature's answers are about. */
export type FeatureSubject = { paper: string } | { run: string } | { community: { run: string; community: string } };

export interface Status {
	/** the job's number (absent from the Python backend): what `api.cancelJob` names */
	id?: number;
	/** what kind of work it is ("a run", "asking about papers") */
	kind?: string;
	state: JobState;
	/** the name of the run a finished job made, once state is "done"; null for any other job */
	result: string | null;
	/** what a finished job made; absent from the Python backend, whose finished jobs are runs (`result`) */
	outcome?: JobOutcome | null;
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
	/** how many of its papers a job that counts them has done; null for one that does not */
	papers?: JobPapers | null;
	/** whether a key for the chosen provider is set (never the key itself); absent from the Python backend */
	api_key_set?: boolean;
}

/** n of m done, and what it has spent so far on the language model's API, in US dollars (of calls with a price). */
export interface JobProgress {
	done: number;
	of: number;
	usd: number;
}

/** Papers done, of how many. */
export interface JobPapers {
	done: number;
	of: number;
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

/** What the language model has cost (rp/usage.py). Counted from `since`: earlier calls were not recorded. */
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

// ---- Weaknesses (critique) ----

/** What a reviewer's point raises, as its section's name says (engine text/review_points.rs). */
export type ReviewConcern = "weakness" | "question" | "limitation";

/** How an authors' reply is attached to a review: named by its own title, or a guess and what it rests on. */
export type ReplyAttachment = { kind: "named" } | { kind: "guessed"; basis: "same_reviewer" | "mentions_reviewer" | "follows_review" };

/** A score a reviewer gave, on its venue's scale. */
export interface CritiqueScore {
	field: string;
	value: number;
	meaning: string;
	scale: Scale;
}

/** A section of a review a point is in; `text` is exactly the kept page's text at `span`. */
export interface CritiqueSection {
	/** its index among the review's sections */
	index: number;
	name: string;
	text: string;
	span: Span;
}

/** One point a reviewer made: exactly the kept page's text at `span` (characters, as code points). */
export interface CritiquePoint {
	/** "p3": what the model's reading of it names it; null when it was not asked about */
	id: string | null;
	concern: ReviewConcern;
	/** its section's index among the review's sections */
	section: number;
	text: string;
	span: Span;
}

/** A note of the paper's authors attached to a review. */
export interface CritiqueReply {
	note: number;
	heading: string;
	attachment: ReplyAttachment;
	/** the reply's text from the kept page, cut when `cut` */
	text: string;
	cut: boolean;
	span: Span;
}

export interface CritiqueReviewer {
	note: number;
	/** the reviewer's anonymous id ("aBcD"), when the page shows one */
	reviewer: string | null;
	heading: string;
	rating: CritiqueScore | null;
	confidence: CritiqueScore | null;
	sections: CritiqueSection[];
	points: CritiquePoint[];
	/** sections of strengths and weaknesses together that the reviewer did not mark apart: their names */
	not_split: string[];
	replies: CritiqueReply[];
}

export type CritiqueReviewers =
	{ state: "none_kept" } | { state: "read"; address: string; read_at: string; reviewers: CritiqueReviewer[]; points: number };

export type WeaknessKind =
	| "missing_baseline"
	| "few_datasets_or_seeds"
	| "no_significance_or_variance"
	| "claim_broader_than_evidence"
	| "missing_ablation"
	| "reproducibility_detail_missing"
	| "limitation_not_discussed"
	| "other";

/** Where the model reads a reviewer's point as applying: a passage found in the paper, or no passage on it. */
export type PointLocation = { kind: "passage"; evidence: Evidence } | { kind: "no_passage" };

/** The model's reading of one reviewer's point; every quote in it was found in the paper. `location` is null when
 *  the model's place for the point was not found in the paper; then `paper_says` is there (never both null). */
export type PointReading =
	/** `paper_says`: what the paper says that responds to the point: the paper's words only, no judgement */
	{ point: string; location: PointLocation; paper_says: Evidence | null } | { point: string; location: null; paper_says: Evidence };

/** Whether a weakness says something is absent, and what the engine's search of the paper's text found. */
export type Absence =
	| { kind: "not_claimed" }
	| { kind: "none_found"; searched: string[] }
	| { kind: "not_searched" }
	| { kind: "but_see"; searched: string[]; found: Evidence[] };

/** A weakness the model raises that the reviewers did not: its reading, with the passage it is about. */
export interface ModelWeakness {
	kind: WeaknessKind;
	weakness: string;
	about: Evidence;
	absence: Absence;
}

/** What of the model's answer is not shown, counted. */
export interface CritiqueLeftOut {
	not_in_paper: number;
	malformed: number;
	unknown_point: number;
	beyond_limit: number;
	/** weaknesses worded as a judgement of the paper (accept, reject, a score): the section gives none */
	verdict: number;
	/** points asked about that the answer has no entry for: they have no reading at all */
	not_answered?: number;
}

export interface CritiqueReading {
	points: PointReading[];
	weaknesses: ModelWeakness[];
	left_out: CritiqueLeftOut;
}

/** What asking for a paper's weaknesses would cost: an estimate. */
export type CritiqueEstimate = BatchCost;

export type CritiqueModelPart =
	| { state: "no_pdf" }
	| { state: "unreadable" }
	| {
			state: "not_asked";
			provider: ProviderId;
			model: string;
			estimate: CritiqueEstimate;
			api_key_set: boolean;
			points_asked: number;
			cut: number | null;
	  }
	| { state: "answered"; model: string; kept_at: string; reading: CritiqueReading; points_asked: number; cut: number | null };

/** `GET /api/papers/<id>/critique`: a paper's weaknesses, the reviewers' points first and the model's reading second. */
export interface Critique {
	paper_id: string;
	reviewers: CritiqueReviewers;
	model: CritiqueModelPart;
}

/** `POST /api/papers/<id>/critique`'s body: the model whose estimate was shown, and its worst case accepted (refused
 *  when asking would now cost more: the paper's text, its reviews or the model changed), or a budget of one's own. */
export type CritiqueStart = ShownModel & (AcceptedCost | BudgetOf);

/** `POST /api/papers/<id>/critique`: the job started, or nothing asked because the reading is kept. */
export type CritiqueStarted = { outcome: "started"; job: number } | { outcome: "kept" };

/** `DELETE /api/papers/<id>/critique`: the reading shown (the chosen model's) forgotten, 1, or none there, 0. Other
 *  models' readings of the paper stay. */
export interface CritiqueForgotten {
	forgotten: number;
}

// ---- Agree / disagree (stances) ----

/** Which papers a question is about: a community of the run (by its tree path), or papers the user marked. */
export type StanceScope = { community: string } | { papers: string[] };

/**
 * Where the model placed one paper on a yes/no question (engine analysis/stances.rs). Every placement but
 * `not_addressed` comes with the paper's own words, found in it; a placement whose quotes were not found is
 * `not_verified` and is never counted for or against. `unusable`: the kept answer is not one the question allows.
 */
export type StancePlaced =
	| { group: "supports"; evidence: Evidence[] }
	| { group: "mixed"; /** the model's words for when the answer is yes */ condition: string; evidence: Evidence[] }
	| { group: "contradicts"; evidence: Evidence[] }
	| { group: "not_addressed" }
	| { group: "not_verified"; quotes_not_found: number }
	| { group: "unusable" };

export type StanceGroup = StancePlaced["group"];

export interface StanceRow {
	paper: string;
	title: string;
	tier: Acceptance | null;
	year: number | null;
	/** what the model read: the PDF's text, or only the title and abstract (a weaker reading) */
	read: "pdf" | "abstract";
	placed: StancePlaced;
	/** one of the most central papers, asked first */
	in_sample: boolean;
}

/** What the pre-filter (no model) decided: papers sharing no distinctive word with the question are skipped. */
export interface StanceFilter {
	/** the scope's papers the library has: `related` and `unrelated` add up to it */
	papers: number;
	/** the scope's papers the library does not have: neither asked nor skipped (the engine always sends it) */
	not_in_library?: number;
	related: number;
	unrelated: number;
	include_unrelated: boolean;
	/** the words looked for, folded ("model" for "models") */
	terms: string[];
}

/** What a batch of calls would cost, before anything is sent: an estimate. */
export type StanceCostEstimate = BatchCost;

export interface StancePartEstimate {
	papers: number;
	with_pdf: number;
	abstract_only: number;
	estimate: StanceCostEstimate;
}

/** `POST .../stances/estimate`: what asking a question would cost. Nothing is sent. */
export interface StancePlan {
	id: string;
	question: string;
	provider: ProviderId;
	model: string;
	api_key_set: boolean;
	filter: StanceFilter;
	/** the most central papers, asked first */
	sample: StancePartEstimate;
	/** every paper to ask (those answered already counted as kept) */
	all: StancePartEstimate;
}

/** `GET .../stances/<id>`: what is known so far about a question. */
export interface StanceResult {
	id: string;
	question: string;
	asked_at: string;
	provider: ProviderId;
	model: string;
	filter: StanceFilter;
	/** papers that would be asked */
	to_ask: number;
	/** of them, those with an answer */
	answered: number;
	sample: number;
	/** most central first */
	rows: StanceRow[];
}

/** A question kept for a scope, to open again without paying. */
export interface StanceAsked {
	id: string;
	question: string;
	round: number;
	include_unrelated: boolean;
	papers: string[];
	asked_at: string;
}

/** A word or pair of words a community's titles and abstracts keep using, counted without a model. */
export interface StanceRecurring {
	term: string;
	papers: number;
	/** how many times more often its papers use it than the run's */
	lift: number;
}

/** Questions the model proposed, each with quotes found in at least two papers' abstracts. */
export interface StanceProposals {
	questions: Cited<string>[];
	/** proposals left out: their quotes were not found in two papers' abstracts */
	left_out: number;
	model: string;
	kept_at: string;
}

/** `GET .../stances` (a community) or `POST .../marked/stances/list` (marked papers). */
export interface StanceListing {
	papers: number;
	questions: StanceAsked[];
	suggestions: StanceRecurring[];
	proposals: StanceProposals | null;
}

/** `POST .../stances/proposals/estimate`: what proposing questions would cost. An estimate. */
export interface StanceProposalPlan {
	papers: number;
	provider: ProviderId;
	model: string;
	estimate: StanceCostEstimate;
	api_key_set: boolean;
}

/** A job started: its number (for `/api/status` and `api.cancelJob`) and the question's id. */
export interface StanceStarted {
	job: number;
	id: string;
}

// ---- extraction table ----

/** What kind of answer a column of an extraction table takes (engine analysis/extraction.rs). */
export type ExtractionAnswerKind = "text" | "yes_no" | "number" | "list";

/** A column of an extraction table: a question every paper is asked. */
export interface ExtractionColumn {
	/** stable within a run's table: lower-case letters, digits, "_" and "-" */
	id: string;
	label: string;
	question: string;
	kind: ExtractionAnswerKind;
}

/** `GET /api/runs/<run>/extraction/columns`: the run's columns (kept, or the defaults), and the defaults. */
export interface ExtractionColumns {
	columns: ExtractionColumn[];
	defaults: ExtractionColumn[];
}

/** A stated answer, of its column's kind. A number's `written` is inside one of its cell's found quotes. */
export type ExtractionValue =
	| { kind: "text"; text: string }
	| { kind: "yes_no"; answer: "yes" | "no" }
	| { kind: "number"; written: string; value: number; unit: string; context: string }
	| { kind: "list"; items: string[] };

/** One cell. A value comes only with evidence found in the paper; one whose quotes were not found is
 *  `not_verified` and carries no value. */
export type ExtractionCell =
	| { kind: "stated"; value: ExtractionValue; evidence: Evidence[] }
	| { kind: "not_stated" }
	| { kind: "not_verified"; why: "quotes_not_found" | "number_not_in_quote" | "value_not_in_quote" }
	| { kind: "malformed"; why: "status" | "omitted" | "empty" | "not_yes_no" | "not_a_number" | "too_long" }
	| { kind: "not_asked" };

/** One paper of an extraction table. */
export interface ExtractionRow {
	paper: string;
	title: string;
	year: number | null;
	acceptance: Acceptance | null;
	acceptance_term: AcceptanceTerm | null;
	/** how the paper is read: its PDF, or its title and abstract only */
	text: "pdf" | "no_pdf" | "unreadable" | "not_in_library";
	/** one of the sample's papers */
	sample: boolean;
	/** in the columns' order */
	cells: ExtractionCell[];
}

/** `POST /api/runs/<run>/extraction/table` with `{ scope }`: what is kept, every quote checked again. */
export interface ExtractionTable {
	columns: ExtractionColumn[];
	/** the model whose answers these are, and its provider */
	provider: ProviderId;
	model: string;
	/** most central first: the sample's papers come first */
	rows: ExtractionRow[];
	/** how many papers the sample has */
	sample: number;
	/** cells not shown as values, by why */
	left_out: { quotes_not_found: number; value_not_in_quote: number; malformed: number };
}

/** Which papers a table is about: a community by its tree path, or papers of the run chosen by hand. */
export type ExtractionScope = { community: string } | { papers: string[] };

/** The sample (the most central papers) or the rest. */
export type ExtractionPart = "sample" | "rest";

/** `POST /api/runs/<run>/extraction/estimate`: what asking would cost, before anything is sent. An estimate. */
export interface ExtractionEstimate extends AskingCost {
	part: ExtractionPart;
	papers: number;
	/** papers with a cell to ask; the others are kept */
	to_ask: number;
	api_key_set: boolean;
	/** what it was reckoned from: the papers' own texts, or (for the rest) the sample's */
	basis: { kind: "texts" } | { kind: "sample_texts"; papers: number; characters: number } | { kind: "typical"; characters: number };
}

// ---- explain a passage ----

/** `POST /api/papers/<id>/explanation/prepare` (and the other explanation routes): the passage's address, as
 *  `PaperPassages` gives it. The passage explained is the one the address starts in. */
export interface ExplanationRequest {
	address: PassageAddress;
}

/** `POST /api/papers/<id>/explanation`: the passage, and the model and worst case the reader was shown; the
 *  engine refuses when either changed since, so nothing is paid that was not shown. */
export type ExplanationStart = ExplanationRequest & ShownModel & AcceptedCost;

/** What is asked about a passage, from its kind (engine explanation/mod.rs). */
export type ExplanationQuestion = "passage" | "equation" | "table" | "figure";

export type ExplanationPart = "explanation" | "symbol" | "reading" | "prerequisite";

/** One statement of an explanation: the paper's (with the evidence found for it, never empty) or general
 *  background knowledge, which never carries a link to the paper. */
export type ExplanationStatement =
	| { source: "paper"; part: ExplanationPart; symbol: string | null; text: string; evidence: Evidence[] }
	| { source: "background"; part: ExplanationPart; symbol: string | null; text: string };

/** A kept explanation, its quotes checked again when it is shown. */
export interface Explanation {
	/** the model that wrote it, as the API named it */
	model: string;
	kept_at: string;
	statements: ExplanationStatement[];
	/** statements not shown: said to be the paper's but their quotes were not found in it, or of the wrong shape */
	left_out: { not_found: number; malformed: number; past_bound: number };
}

/** What explaining would cost: an estimate, from characters at an assumed or measured rate. */
export type ExplanationEstimate = AskingCost;

/** A symbol or a capitalised term the passage uses, and the sentence of the paper that introduces it (found by
 *  a program, no model). */
export interface DefinedTerm {
	term: { kind: "symbol" | "name"; text: string };
	evidence: Evidence;
}

export type ExplanationPrepared =
	/** ready: the explanation kept from before (shown at no cost), or, not asked yet, what asking would cost */
	| ({
			state: "ready";
			paper: string;
			passage: { kind: PassageKind; address: PassageAddress };
			question: ExplanationQuestion;
			/** the passage's extracted text may be garbled (equations, tables, figures): only text is sent */
			garbled: boolean;
			/** the model that would be asked, of its provider: the one chosen in the settings */
			provider: ProviderId;
			model: string;
			api_key_set: boolean;
			definitions: DefinedTerm[];
	  } & ({ explained: Explanation; estimate: null } | { explained: null; estimate: ExplanationEstimate }))
	| { state: "no_text"; why: "no_pdf" | "unreadable" };

/** `POST /api/papers/<id>/explanation`: the job that asks, when one was started (none when one is kept). */
export interface ExplanationStarted {
	started: boolean;
	job: number | null;
}

/** `DELETE /api/papers/<id>/explanation`: whether there was an explanation of the passage (by the model chosen) to
 *  forget; the paper's other explanations stay. */
export interface ExplanationRedone {
	redone: boolean;
}

/** `GET /api/papers/<id>/explanations`: the passages with an explanation kept for the model chosen. */
export interface ExplainedPassages {
	model: string;
	explained: PassageAddress[];
}

/** A passage of another paper about the same thing, found by the words they share (no model). */
export interface RelatedPassage {
	paper: string;
	evidence: Evidence;
	/** the words it shares with the passage asked about, the rarest first */
	words: string[];
}

/** `POST /api/runs/<run>/papers/<id>/related`: what was searched and found. */
export interface RelatedPassages {
	/** nearby papers looked at (neighbours in the graph, then the paper's community), at most `bound` */
	nearby: number;
	bound: number;
	by_text: number;
	/** searched by their abstract only: their PDF has not been read */
	by_abstract: number;
	not_in_library: number;
	found: RelatedPassage[];
}

// ---- Claim tree ----

/** What evidence a paper offers for a claim (engine analysis/claim_tree/tree.rs). */
export type ClaimEvidenceKind = "experiment" | "table" | "figure" | "theorem" | "ablation" | "citation" | "argument";

/** One node of a paper's claim tree: a central claim, a finding under it, or the evidence the paper offers. Its
 *  statement is the model's reading of the paper; every piece of `evidence` was found in the paper. */
export type ClaimNode = ({ kind: "claim" } | { kind: "finding" } | { kind: "evidence"; evidence_kind: ClaimEvidenceKind }) & {
	/** "n1", "n2", ... in the tree's order */
	id: string;
	parent: string | null;
	statement: string;
	evidence: Evidence[];
	/** the caption of the table or figure, or the theorem's passage, it names, when the paper has it */
	points_at: Evidence | null;
};

/** Nodes of the model's answer left out, by why: never shown, counted. */
export interface ClaimsLeftOut {
	/** its quotes were not found in the paper */
	unquoted: number;
	/** it stood under a node that was left out, and could not move up */
	orphaned: number;
	no_parent: number;
	misplaced: number;
	/** past the bounds on the tree's size */
	over_bounds: number;
}

export type ClaimTreeState =
	| { state: "no_text" }
	| { state: "not_asked" }
	| { state: "malformed"; why: { why: "duplicate_id" | "cycle"; id: string } | { why: "empty" }; kept_at: string }
	| { state: "made"; nodes: ClaimNode[]; left_out: ClaimsLeftOut; kept_at: string; model: string };

/** What a reviewer's point does to the node it is placed on. */
export type ReviewStance = "supports" | "challenges" | "asks" | "presentation" | "whole_paper";

/** One point of a review, exactly the kept page's text at `span` (code points of the page). */
export interface ReviewPoint {
	/** "p1", "p2", ... */
	id: string;
	/** the review's index among the paper's notes */
	review: number;
	reviewer: string;
	section: string;
	kind: "strength" | "weakness" | "question" | "limitation" | "general";
	text: string;
	span: Span;
}

/** A point placed on a node (null: the paper as a whole) by the model, with the reviewer's own words that justify
 *  it, found in the point at `because_span` (code points of the page). */
export interface PointPlacement {
	point: string;
	node: string | null;
	stance: ReviewStance;
	because: string;
	because_span: Span;
	matched: QuoteMatch;
}

export interface UnplacedPoint {
	point: string;
	why: "not_answered" | "unknown_node" | "not_in_point" | "too_short";
}

export interface PointMapping {
	placed: PointPlacement[];
	/** always shown */
	unplaced: UnplacedPoint[];
	ignored: { unknown_points: number; repeated: number };
}

export type PointMappingState =
	| { state: "no_reviews" }
	| { state: "no_points" }
	| { state: "no_tree" }
	| { state: "not_asked" }
	| { state: "made"; mapping: PointMapping; kept_at: string; model: string };

/** A reviewer's score on the venue's own scale for that field and year. */
export interface ReviewerRating {
	field: string;
	value: number;
	scale: Scale;
}

/** A reply to a review, and how it was attached to it: named by its title, or a guess. */
export interface ClaimReply {
	note: number;
	signer: Signer;
	heading: string;
	attached: { by: "named" } | { by: "guessed"; basis: "same_reviewer" | "mentions_reviewer" | "follows_review" };
	sections: ReviewSection[];
}

export interface ClaimReviewer {
	note: number;
	name: string;
	overall: ReviewerRating | null;
	confidence: ReviewerRating | null;
	replies: ClaimReply[];
}

export interface ClaimReviews {
	address: string;
	read_at: string;
	reviewers: ClaimReviewer[];
	points: ReviewPoint[];
	/** points past the bounds, not mapped */
	left_out: number;
	figures: PaperFigures;
	meta_reviews: { signer: Signer; heading: string; sections: ReviewSection[] }[];
	decision: string | null;
}

/** Reviewers by what their points do to one node, each named once per stance. */
export interface ReviewStances {
	supports: string[];
	challenges: string[];
	asks: string[];
	presentation: string[];
	whole_paper: string[];
}

export interface ClaimNodeReviews {
	/** null: the root, the paper as a whole */
	node: string | null;
	points: string[];
	reviewers: ReviewStances;
	contested: boolean;
}

/** The model's reading of what is missing, shown apart from and after what reviewers said. */
export interface ClaimGap {
	kind: "no_stated_support" | "not_examined";
	node: string;
	statement: string;
	evidence: Evidence[];
}

/** What a batch of calls is likely to cost, before anything is sent: an estimate. */
export type ClaimsEstimate = BatchCost;

/** `GET /api/papers/<id>/claims`: the paper's claim tree with its reviews mapped onto it. */
export interface ClaimsView {
	paper_id: string;
	/** the model chosen in the settings, of its provider */
	provider: ProviderId;
	model: string;
	paper: { state: "read"; passages: number; sent: number } | { state: "no_pdf" } | { state: "unreadable" };
	tree: ClaimTreeState;
	reviews: ClaimReviews | null;
	mapping: PointMappingState;
	/** the root first, then each node with points placed on it */
	node_reviews: ClaimNodeReviews[];
	contested: number;
	gaps: ClaimGap[];
	/** what asking what is not kept would cost; null when nothing can be asked */
	estimate: ClaimsEstimate | null;
}

/** `POST /api/papers/<id>/claims`: the most the calls may spend, and the model the estimate was shown for. */
export type ClaimsStart = BudgetOf & ShownModel;

/** `GET /api/runs/<run>/communities/<tree path>/claims/contested`: per paper mapped, its contested claims. */
export interface ContestedClaims {
	papers: number;
	papers_with_reviews: number;
	papers_mapped: number;
	counts: { paper_id: string; contested: number; nodes: number; reviewers: number }[];
}

// ---- citation contexts and kinds ----

/** The part of a paper a citation is in, normalised from its section's heading (engine analysis/citation_contexts.rs). */
export type CitationSection =
	| { kind: "abstract" | "introduction" | "related_work" | "background" | "method" | "experiments" | "discussion" | "appendix" }
	/** a heading none of those words name, as the paper writes it */
	| { kind: "other"; title: string };

/** Whether the cited work is named alone or in a group ("[3, 7-9]": a grouped citation says less about each work). */
export type CitationGrouping = { kind: "alone" } | { kind: "grouped"; with: number };

/** What a citation does, as the model reads it (engine analysis/citation_kinds.rs). */
export type CitationKind = "uses" | "uses_data" | "baseline" | "extends" | "supports" | "contrasts" | "mention";

/** The model's reading of one citing sentence, shown only beside the sentence. A kind other than mention counts
 *  only when the words that show it (`because`, the citing paper's own) were found in the sentence or its
 *  neighbours; otherwise it is `not_justified`, counted as a mention. */
export type CitationJudgement =
	| { state: "justified"; kind: CitationKind; because: Evidence }
	| { state: "mention" }
	| { state: "not_justified"; claimed: CitationKind }
	| { state: "not_classified" };

/** How a paper is cited, in citing sentences: every count is out of `sentences`, and they add up to it. */
export interface CitationTally {
	sentences: number;
	/** distinct citing papers those sentences are in */
	papers: number;
	/** justified kinds other than mention */
	kinds: Partial<Record<CitationKind, number>>;
	mentions: number;
	not_justified: number;
	not_classified: number;
}

/** One citing sentence: the sentence itself as evidence of the citing paper, its neighbours and what it is. */
export interface CitingSentenceShown {
	evidence: Evidence;
	/** the sentences on either side, also evidence of the citing paper (null when there is none or it was not found) */
	before: Evidence | null;
	after: Evidence | null;
	section: CitationSection;
	grouping: CitationGrouping;
	/** the citation as the sentence writes it: "[3, 7-9]", "Vaswani et al., 2017" */
	written: string;
	judgement: CitationJudgement;
}

/** A paper cited by (in `cites`) or citing (in `cited_by`) the paper asked about, with the sentences. */
export interface CitationContextsOf {
	paper: string;
	title: string;
	tally: CitationTally;
	sentences: CitingSentenceShown[];
}

/** `GET /api/papers/<id>/citation-contexts?run=<name>`: the sentences between this paper and the library's. */
export interface PaperCitationContexts {
	paper: string;
	/** what came of reading this paper: with no PDF, only who cites it is known */
	state: "read" | "no_pdf" | "unreadable";
	/** entries of its reference list; 0 when none was found */
	references_listed: number;
	cites: CitationContextsOf[];
	/** papers whose citing sentences were read that cite it, the most sentences first */
	cited_by: CitationContextsOf[];
	cited_by_total: number;
	/** how many of the run's papers have had their citing sentences read (null without a run) */
	coverage: { papers: number; read: number; without_pdf: number } | null;
	/** sentences whose place could no longer be found in their paper's text: not shown */
	left_out: number;
	/** the model whose readings are shown */
	model: string;
}

/** A cited paper ranked by how often it is cited one way. */
export interface RankedCitation {
	paper: string;
	/** citing sentences read as the kind ranked by */
	count: number;
	tally: CitationTally;
}

/** `GET /api/runs/<name>/citation-kinds?community=<tree path>`: what the community's citations come to. */
export interface CitationKindsSummary {
	papers: number;
	/** of its papers, whose citing sentences were read */
	papers_read: number;
	without_pdf: number;
	/** papers of the library its papers cite */
	cited_papers: number;
	tally: CitationTally;
	baseline: RankedCitation[];
	contrasts: RankedCitation[];
	/** their dataset or benchmark used */
	data: RankedCitation[];
	/** their method used or extended */
	used: RankedCitation[];
	supports: RankedCitation[];
	/** the papers a sample run reads first: the most cited by the community */
	sample: string[];
	titles: Record<string, string>;
	model: string;
}

export type CitationKindsScope = "sample" | "rest";

/** `GET /api/runs/<name>/citation-kinds/estimate`: what reading a scope's kinds would cost; nothing is sent. */
export interface CitationKindsEstimate {
	scope: CitationKindsScope;
	provider: ProviderId;
	model: string;
	sentences: number;
	/** read already: they cost nothing */
	kept: number;
	to_ask: number;
	estimate: BatchCost;
	/** a key is set, so the model can be asked (never the key) */
	can_ask: boolean;
}

/** `POST /api/runs/<name>/citation-kinds`: the most the job may spend, and the model and the sentences to ask of the
 *  estimate shown (anything else is refused: estimate again). */
export type ClassifyCitationsRequest = BudgetOf &
	ShownModel & {
		community?: string | null;
		scope: CitationKindsScope;
		to_ask: number;
	};

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

/** What a call is likely to cost: an estimate either way. */
export type AnswerCost = AskingCost;

/** `GET /api/runs/<run>/answers/estimate?q=...&community=...`: what asking would cost. Nothing is sent. */
export type AnswerEstimate =
	| {
			state: "ready";
			/** the question's id once asked */
			id: string;
			provider: ProviderId;
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

/** `POST /api/runs/<run>/answers`, sent when the user clicks the button showing the estimate: the most it was shown
 *  to cost (in tokens for a model with no price), for the model it was shown for. */
export type AnswerRequest = ShownModel &
	AcceptedCost & {
		/** the id of the estimate shown (`AnswerEstimate.id`): refused when the passages that would be sent changed since */
		id: string;
		question: string;
		/** a community's tree path; absent or null for the whole run */
		community?: string | null;
	};

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
