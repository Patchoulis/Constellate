import type {
	AskedFrom,
	Catalog,
	CitationNetwork,
	Critique,
	CritiqueStarted,
	ClaimsStart,
	ClaimsView,
	ContestedClaims,
	Declarations,
	GraphData,
	Library,
	LocateAnswer,
	ModelCatalog,
	OverviewStatus,
	Paper,
	PaperPassages,
	PaperReviews,
	Ratings,
	ReviewsRead,
	ReviewSummary,
	RunCitations,
	RunRequest,
	RunResult,
	RunSummary,
	Settings,
	Status,
	Usage,
	ExplainedPassages,
	ExplanationPrepared,
	ExplanationStarted,
	PassageAddress,
	RelatedPassages,
	CitationKindsEstimate,
	CitationKindsScope,
	CitationKindsSummary,
	ClassifyCitationsRequest,
	PaperCitationContexts,
} from "@rp/contracts";
import { estimateQuery, type NamingMode } from "@/lib/features/writing/naming";
import { changesSomething, csrfOf, signInRedirect, withCsrf } from "@/lib/net/engine";
import { answered, currentSignal, POLLING, QUICK, READS_PAPERS, request, STARTS_JOB, type Patience } from "@/lib/net/request";

/** On a server, the session's token that every change carries, asked for once; null on the user's own computer. */
let csrf: Promise<string | null> | null = null;
const sessionToken = () =>
	(csrf ??= request("/auth/session", undefined, { patience: QUICK }).then(
		(answer) => (answer.ok && answer.body.kind === "json" ? csrfOf(answer.body.value) : null),
		() => null,
	));

/**
 * Ask the engine, within `patience` (lib/net/request.ts: a deadline, and whether a GET may be asked once more when
 * nothing answered). Throws a `RequestError` that says what happened and carries the HTTP status. A request made
 * inside `withSignal` (what `useAsync` does) is called off when that signal aborts.
 */
async function call<T>(path: string, init?: RequestInit, patience: Patience = QUICK): Promise<T> {
	const signal = currentSignal(); // before the first await: the signal of whoever started this request
	const token = changesSomething(init?.method) ? await sessionToken() : null;
	const answer = await request(path, withCsrf(init, token), { patience, signal });
	// a server whose session ended (or was never started): sign in again; a refused token is asked for anew
	if (answer.status === 401 || answer.status === 403) csrf = null;
	const signIn = signInRedirect(answer.status, answer.headers.get("WWW-Authenticate"), window.location.pathname);
	if (signIn) window.location.assign(signIn);
	return answered<T>(answer);
}

/** `call` for a route that may read a paper's PDF the first time (lib/net/request.ts `READS_PAPERS`). */
const reading = <T>(path: string, init?: RequestInit) => call<T>(path, init, READS_PAPERS);
/** `call` for starting a job the model is paid for: the engine checks the estimate shown again first, which may read
 *  papers, so it is given as long as an estimate (lib/net/request.ts `STARTS_JOB`); a deadline that passed first would
 *  say "not asked" of a job the engine then starts. Never sent twice. */
const starting = <T>(path: string, init?: RequestInit) => call<T>(path, init, STARTS_JOB);

const send = (method: string, body: unknown): RequestInit => ({
	method,
	headers: { "Content-Type": "application/json" },
	body: JSON.stringify(body),
});

const run = (name: string) => `/api/runs/${encodeURIComponent(name)}`;

/** The run a paper is read from inside, for the body of a question about it: what the question costs is that run's. */
const askedFrom = (fromRun: string | undefined): AskedFrom => (fromRun ? { run: fromRun } : {});

/** Hand over the text of a paper's reviews page and its address: the engine keeps the whole discussion. */
const readReviews = (id: string, text: string, url: string) =>
	call<ReviewsRead>(`/api/papers/${encodeURIComponent(id)}/reviews`, send("POST", { text, url }));

export const api = {
	runs: () => call<RunSummary[]>("/api/runs"),
	run: (name: string) => call<RunResult>(run(name)),
	graph: (name: string) => call<GraphData>(`${run(name)}/graph`),
	/** what the run and each of its communities cite and are cited, counted from every reference list of the library read so far */
	runCitations: (name: string) => reading<RunCitations>(`${run(name)}/citations`),
	rename: (name: string, title: string) => call<{ title: string }>(run(name), send("PATCH", { title })),
	remove: (name: string) => call<{ ok: true }>(run(name), { method: "DELETE" }),
	/** where a community's overview stands; with `write`, write it within the most of the estimate shown */
	overview: (name: string, treePath: string, write?: import("@rp/contracts").OverviewRequest | true) =>
		call<OverviewStatus>(
			`${run(name)}/overview/${encodeURIComponent(treePath)}`,
			write === undefined ? { method: "GET" } : write === true ? { method: "POST" } : send("POST", write),
		),
	/** each statement of a community's description with its closest papers, when found (free: nothing is sent anywhere) */
	closest: (name: string, treePath: string) =>
		call<import("@rp/contracts").ClosestPapers>(`${run(name)}/closest/${encodeURIComponent(treePath)}`),
	/** find them with the embedding model on this computer, as the one job; no job when they were found before */
	findClosest: (name: string, treePath: string) =>
		starting<import("@rp/contracts").ClosestStarted>(`${run(name)}/closest/${encodeURIComponent(treePath)}`, { method: "POST" }),
	start: (request: RunRequest) => call<{ ok: true }>("/api/runs", send("POST", request)),
	status: (since: number) => call<Status>(`/api/status?since=${since}`, undefined, POLLING),
	settings: () => call<Settings>("/api/settings"),
	saveSettings: (changes: Partial<Settings>) => call<Settings>("/api/settings", send("PUT", changes)),
	library: () => call<Library>("/api/library"),
	/** what was spent, with the worth of a US dollar in `currency` (a three-letter code); with `ofRun` (a run's
	 *  name), what that run cost beside it */
	usage: (currency: string, ofRun?: string | null) =>
		call<Usage>(`/api/usage?currency=${encodeURIComponent(currency)}${ofRun ? `&run=${encodeURIComponent(ofRun)}` : ""}`),
	/** calls to the language model whose outcome is unknown and that may have been billed: never in `usage`'s totals */
	unsettled: () => call<import("@rp/contracts").Unsettled>("/api/usage/unsettled"),
	catalog: () => call<Catalog>("/api/catalog"),
	/** every language model provider and its models: whether each key is set (never the key), prices, efforts */
	models: () => call<ModelCatalog>("/api/models"),
	/** the models the OpenAI-compatible server says it has */
	endpointModels: () => call<{ models: string[] }>("/api/models/endpoint"),
	paper: (id: string) => call<Paper>(`/api/papers/${encodeURIComponent(id)}`),
	/** the paper's PDF: the kept one, or fetched now from where it is published */
	pdfUrl: (id: string) => `/api/papers/${encodeURIComponent(id)}/pdf`,
	/** the paper's text from its PDF, by page, section and paragraph (slow the first time: the PDF is read); fails
	 *  with "not found" when the engine has no readable PDF of it */
	passages: (id: string) => reading<PaperPassages>(`/api/papers/${encodeURIComponent(id)}/passages`),
	/** where a quote is in the paper's text, in the paper's own words */
	locate: (id: string, quote: string) => reading<LocateAnswer>(`/api/papers/${encodeURIComponent(id)}/locate`, send("POST", { quote })),
	/** ask the running job (by its `Status.id`) to stop: it starts nothing new and finishes what is under way */
	cancelJob: (id: number) => call<{ ok: true }>("/api/job/cancel", send("POST", { id })),
	/** read what a paper's authors declare, from its PDF (slow the first time: the PDF is fetched) */
	declarations: (id: string) => reading<Declarations>(`/api/papers/${encodeURIComponent(id)}/declarations`, { method: "POST" }),
	/** the reviewers' scores of every paper whose reviews page has been opened, by paper id */
	ratings: () => call<Record<string, Ratings>>("/api/reviews"),
	/** hand over the text of a paper's reviews page and the page's address; answers the scores now kept (null if none) */
	readRatings: (id: string, text: string, url: string) =>
		readReviews(id, text, url).then((read) => (read.outcome === "not_its_page" ? null : read.ratings)),
	readReviews,
	/** a paper's kept discussion and what its scores come to; null until its reviews page has been opened */
	paperReviews: (id: string) => call<PaperReviews | null>(`/api/papers/${encodeURIComponent(id)}/reviews`),
	/** what the reviews of a run's papers come to, or of one community's (by its tree path) */
	reviewSummary: (name: string, treePath?: string) =>
		call<ReviewSummary>(`${run(name)}${treePath ? `/communities/${encodeURIComponent(treePath)}` : ""}/reviews/summary`),
	/** start the job that does that for every paper of a run */
	readDeclarations: (name: string) => call<{ ok: true }>(`${run(name)}/declarations`, { method: "POST" }),
	/** the citation network around a paper, from the reference lists read so far; with `inRun` (a run's name), it
	 *  also counts the citing papers of that run */
	citations: (id: string, inRun?: string) =>
		reading<CitationNetwork>(`/api/papers/${encodeURIComponent(id)}/citations${inRun ? `?run=${encodeURIComponent(inRun)}` : ""}`),
	/** read this paper's reference list from its PDF (slow the first time: the PDF is fetched); answers its network */
	readReferences: (id: string, inRun?: string) =>
		reading<CitationNetwork>(`/api/papers/${encodeURIComponent(id)}/references${inRun ? `?run=${encodeURIComponent(inRun)}` : ""}`, {
			method: "POST",
		}),
	/** a paper's weaknesses: the reviewers' points, and the model's reading kept or what asking would cost (nothing is sent) */
	critique: (id: string) => reading<Critique>(`/api/papers/${encodeURIComponent(id)}/critique`),
	/** ask for the model's reading, as the one job, for the model the estimate was shown for: within the worst case
	 *  accepted (refused when asking would now cost more) or a budget of one's own; in US dollars, or in tokens for a
	 *  model with no price; `fromRun`, the run it is read from inside, whose cost it is */
	readCritique: (
		id: string,
		shown: import("@rp/contracts").ShownModel,
		budget: import("@rp/contracts").AcceptedCost | import("@rp/contracts").BudgetOf,
		fromRun?: string,
	) =>
		starting<CritiqueStarted>(
			`/api/papers/${encodeURIComponent(id)}/critique`,
			send("POST", { ...shown, ...budget, ...askedFrom(fromRun) }),
		),
	/** forget the model's reading of the paper, so that it can be asked again */
	forgetCritique: (id: string) => call<{ forgotten: number }>(`/api/papers/${encodeURIComponent(id)}/critique`, { method: "DELETE" }),
	/** where a community's papers (or the marked ones) stand on a question (engine features/stances/routes.rs) */
	stances: {
		listing: (name: string, scope: StanceScope) =>
			"community" in scope
				? reading<import("@rp/contracts").StanceListing>(stancesAt(name, scope))
				: reading<import("@rp/contracts").StanceListing>(`${stancesAt(name, scope)}/list`, send("POST", { papers: scope.papers })),
		/** what asking would cost; nothing is sent to the model */
		estimate: (name: string, scope: StanceScope, question: string, includeUnrelated: boolean) =>
			reading<import("@rp/contracts").StancePlan>(
				`${stancesAt(name, scope)}/estimate`,
				send("POST", { question, include_unrelated: includeUnrelated, papers: "papers" in scope ? scope.papers : [] }),
			),
		/** start the job that asks the most central papers ("sample") or all of them, on the terms the user was shown:
		 *  the model and the most it could cost (the engine refuses a job that would now ask another or cost more) */
		ask: (
			name: string,
			scope: StanceScope,
			question: string,
			includeUnrelated: boolean,
			part: "sample" | "all",
			approved: StanceApproved,
		) =>
			starting<import("@rp/contracts").StanceStarted>(
				`${stancesAt(name, scope)}/ask`,
				send("POST", { question, include_unrelated: includeUnrelated, part, papers: "papers" in scope ? scope.papers : [], ...approved }),
			),
		result: (name: string, scope: StanceScope, id: string) =>
			reading<import("@rp/contracts").StanceResult>(`${stancesAt(name, scope)}/${encodeURIComponent(id)}`),
		/** forget this question's answers for a fresh reading (the next ask pays again) */
		redo: (name: string, scope: StanceScope, id: string) =>
			call<import("@rp/contracts").StanceAsked>(`${stancesAt(name, scope)}/${encodeURIComponent(id)}/redo`, { method: "POST" }),
		/** what proposing questions would cost; `fresh` to propose anew past the proposals kept */
		proposalEstimate: (name: string, community: string, fresh: boolean) =>
			call<import("@rp/contracts").StanceProposalPlan>(`${stancesAt(name, { community })}/proposals/estimate`, send("POST", { fresh })),
		propose: (name: string, community: string, fresh: boolean, approved: StanceApproved) =>
			starting<import("@rp/contracts").StanceStarted>(`${stancesAt(name, { community })}/proposals`, send("POST", { fresh, ...approved })),
	},
	/** the extraction table's columns of a run (kept, or the defaults) */
	extractionColumns: (name: string) => call<import("@rp/contracts").ExtractionColumns>(`${run(name)}/extraction/columns`),
	saveExtractionColumns: (name: string, columns: import("@rp/contracts").ExtractionColumn[]) =>
		call<import("@rp/contracts").ExtractionColumns>(`${run(name)}/extraction/columns`, send("PUT", { columns })),
	/** the table as kept, its quotes checked again; nothing is asked */
	extractionTable: (name: string, scope: import("@rp/contracts").ExtractionScope) =>
		reading<import("@rp/contracts").ExtractionTable>(`${run(name)}/extraction/table`, send("POST", { scope })),
	/** what asking the sample or the rest would cost; nothing is sent */
	extractionEstimate: (name: string, scope: import("@rp/contracts").ExtractionScope, part: import("@rp/contracts").ExtractionPart) =>
		reading<import("@rp/contracts").ExtractionEstimate>(`${run(name)}/extraction/estimate`, send("POST", { scope, part })),
	/** start asking, as a job, within `budget` (US dollars, or tokens for a model with no price), of the model the
	 *  estimate was shown for; answers the job's number */
	runExtraction: (
		name: string,
		scope: import("@rp/contracts").ExtractionScope,
		part: import("@rp/contracts").ExtractionPart,
		budget: import("@rp/contracts").BudgetOf,
		shown: import("@rp/contracts").ShownModel,
	) => starting<{ ok: true; id: number }>(`${run(name)}/extraction/run`, send("POST", { scope, part, ...budget, ...shown })),
	/** forget these papers' cells, so that they are asked again at the next run */
	forgetExtraction: (name: string, papers: string[]) =>
		call<{ forgotten: number }>(`${run(name)}/extraction/forget`, send("POST", { papers })),
	/** what explaining the passage at `address` involves, free: its definitions, a kept explanation or the estimate */
	prepareExplanation: (id: string, address: PassageAddress) =>
		reading<ExplanationPrepared>(`/api/papers/${encodeURIComponent(id)}/explanation/prepare`, send("POST", { address })),
	/** ask the model to explain the passage at `address`, as a job (none is started when one is kept) */
	/** ask, for the model and the worst case the reader was shown (the engine refuses if either changed); `fromRun`,
	 *  the run the paper is read from inside, whose cost it is */
	explain: (
		id: string,
		address: PassageAddress,
		shown: import("@rp/contracts").ShownModel,
		accepted: import("@rp/contracts").AcceptedCost,
		fromRun?: string,
	) =>
		starting<ExplanationStarted>(
			`/api/papers/${encodeURIComponent(id)}/explanation`,
			send("POST", { address, ...shown, ...accepted, ...askedFrom(fromRun) }),
		),
	/** forget the kept explanation of a passage, so that it is asked for again; `redone` says whether there was one */
	redoExplanation: (id: string, address: PassageAddress) =>
		call<{ redone: boolean }>(`/api/papers/${encodeURIComponent(id)}/explanation`, send("DELETE", { address })),
	/** the passages of a paper with an explanation kept for the model chosen */
	explainedPassages: (id: string) => call<ExplainedPassages>(`/api/papers/${encodeURIComponent(id)}/explanations`),
	/** passages of the run's papers near this one that share the passage's words (no model) */
	relatedPassages: (name: string, id: string, address: PassageAddress) =>
		reading<RelatedPassages>(`${run(name)}/papers/${encodeURIComponent(id)}/related`, send("POST", { address })),
	/** a paper's claim tree with its reviews mapped onto it, as kept, and what asking the rest would cost (asks nothing) */
	claims: (id: string) => reading<ClaimsView>(`/api/papers/${encodeURIComponent(id)}/claims`),
	/** ask the model what is not kept, within `budget_usd`; answers the job's number. `fromRun`: the run the paper is
	 *  read from inside, whose cost it is */
	startClaims: (id: string, start: ClaimsStart, fromRun?: string) =>
		starting<{ job: number }>(`/api/papers/${encodeURIComponent(id)}/claims`, send("POST", { ...start, ...askedFrom(fromRun) })),
	/** forget the paper's tree and mapping: asked again on the next start */
	forgetClaims: (id: string) => call<{ forgotten: number }>(`/api/papers/${encodeURIComponent(id)}/claims`, { method: "DELETE" }),
	/** per paper of a community whose reviews were mapped, its contested claims */
	contestedClaims: (name: string, treePath: string) =>
		reading<ContestedClaims>(`${run(name)}/communities/${encodeURIComponent(treePath)}/claims/contested`),
	/** the sentences where this paper cites papers of the library, and where (the run's, or one of its
	 *  communities') papers cite it */
	citationContexts: (id: string, inRun?: string, treePath?: string) =>
		reading<PaperCitationContexts>(
			`/api/papers/${encodeURIComponent(id)}/citation-contexts${inRun ? `?run=${encodeURIComponent(inRun)}` : ""}${
				inRun && treePath ? `&community=${encodeURIComponent(treePath)}` : ""
			}`,
		),
	/** start the job that reads the citing sentences of every paper of a run (free: no model) */
	readCitationContexts: (name: string) => call<{ ok: true; id: number }>(`${run(name)}/citation-contexts`, { method: "POST" }),
	/** what the kinds of a community's (or the run's) citations come to */
	citationKinds: (name: string, treePath?: string) =>
		reading<CitationKindsSummary>(`${run(name)}/citation-kinds${treePath ? `?community=${encodeURIComponent(treePath)}` : ""}`),
	/** what reading those kinds would cost; nothing is sent */
	citationKindsEstimate: (name: string, scope: CitationKindsScope, treePath?: string) =>
		reading<CitationKindsEstimate>(
			`${run(name)}/citation-kinds/estimate?scope=${scope}${treePath ? `&community=${encodeURIComponent(treePath)}` : ""}`,
		),
	/** start the job that asks the model for those kinds, within the budget */
	classifyCitations: (name: string, request: ClassifyCitationsRequest) =>
		starting<{ ok: true; id: number }>(`${run(name)}/citation-kinds`, send("POST", request)),
	/** forget the kinds read for the papers a community cites, so that they are read again */
	forgetCitationKinds: (name: string, treePath?: string) =>
		call<{ ok: true; forgotten: number }>(`${run(name)}/citation-kinds${treePath ? `?community=${encodeURIComponent(treePath)}` : ""}`, {
			method: "DELETE",
		}),
	// ---- cited answers (the types imported where they are used, so that this file's imports stay as they are) ----
	/** the passages of a run's papers (or one community's) that match `query` best: no model, no cost */
	passagesFound: (name: string, query: string, community?: string | null) =>
		reading<import("@rp/contracts").PassagesFound>(
			`${run(name)}/passages/found?${new URLSearchParams({ q: query, ...(community ? { community } : {}) })}`,
		),
	/** what asking the model `question` would cost; nothing is sent */
	answerEstimate: (name: string, question: string, community?: string | null) =>
		reading<import("@rp/contracts").AnswerEstimate>(
			`${run(name)}/answers/estimate?${new URLSearchParams({ q: question, ...(community ? { community } : {}) })}`,
		),
	/** ask it, accepting the most it can cost as it was shown: starts a job (none when the answer is kept) */
	askQuestion: (name: string, request: import("@rp/contracts").AnswerRequest) =>
		starting<import("@rp/contracts").AnswerStarted>(`${run(name)}/answers`, send("POST", request)),
	/** the questions asked of a run and its communities, the latest first */
	questions: (name: string) => call<import("@rp/contracts").AnswerSummary[]>(`${run(name)}/answers`),
	/** one question and its kept answer, its quotes checked again */
	answer: (name: string, id: string) => reading<import("@rp/contracts").AnswerView>(`${run(name)}/answers/${encodeURIComponent(id)}`),
	/** forget a question and its answer */
	forgetQuestion: (name: string, id: string) => call<{ ok: true }>(`${run(name)}/answers/${encodeURIComponent(id)}`, { method: "DELETE" }),
	/** forget its answer only, so that it is asked again (at the cost shown again) */
	redoAnswer: (name: string, id: string) =>
		call<{ ok: true; forgotten: boolean }>(`${run(name)}/answers/${encodeURIComponent(id)}/redo`, { method: "POST" }),
	/** what naming a run made with the settings would cost at most, before it is made: nothing is sent */
	describeBeforeRun: () => call<import("@rp/contracts").DescribeBeforeRun>("/api/describe/estimate"),
	/** what naming a made run's communities would cost (`mode`: as usual, the plain descriptions again with sections,
	 *  or all again): nothing is sent */
	describeEstimate: (name: string, mode: NamingMode = "name") =>
		call<import("@rp/contracts").DescribeEstimate>(`${run(name)}/describe/estimate${estimateQuery(mode)}`),
	/** name them within the most shown, as the one job */
	describeRun: (name: string, request: import("@rp/contracts").DescribeRequest) =>
		starting<import("@rp/contracts").DescribeStarted>(`${run(name)}/describe`, send("POST", request)),
	/** one page of the runs, sorted and searched, without reading every run (the engine only) */
	runsPage: (query: { offset?: number; limit?: number; sort?: import("@rp/contracts").RunSort; search?: string }) => {
		const asked = new URLSearchParams();
		for (const [key, value] of Object.entries(query)) if (value !== undefined && value !== "") asked.set(key, String(value));
		return call<import("@rp/contracts").RunPage>(`/api/runs?${asked.toString() || "offset=0"}`);
	},
	/** the AI-text detector's state and what was measured about it */
	detector: () => call<import("@rp/contracts").DetectorInfo>("/api/detector"),
	/** what building the detector would cost with the chosen model; nothing is sent */
	detectorEstimate: () => call<import("@rp/contracts").DetectorEstimate>("/api/detector/estimate"),
	/** build it, accepting the estimate as shown or within a budget of one's own: starts the job (training again
	 * what is built asks nothing and needs neither) */
	buildDetector: (
		request: import("@rp/contracts").ShownModel & (import("@rp/contracts").AcceptedCost | import("@rp/contracts").BudgetOf),
	) => starting<import("@rp/contracts").DetectorStarted>("/api/detector/build", send("POST", request)),
	/** score a week of arXiv in each year (arXiv only, free): starts the job */
	checkDetectorYears: () => call<import("@rp/contracts").DetectorStarted>("/api/detector/years", { method: "POST" }),
	/** a paper's AI-text verdict: null when it was never scored or is too short */
	paperAi: (id: string) => call<import("@rp/contracts").AiVerdict | null>(`/api/papers/${encodeURIComponent(id)}/ai`),
};

type StanceScope = import("@rp/contracts").StanceScope;
/** What the user was shown when they clicked: the model and the estimate's "at most" in US dollars (null: price not known). */
/** The terms a stances job is asked on: the model (of its provider) and the most it was shown to cost. */
type StanceApproved = import("@rp/contracts").ShownModel & import("@rp/contracts").AcceptedCost;

/** Where a scope's stances routes are: a community's, or the run's marked papers'. */
function stancesAt(name: string, scope: StanceScope): string {
	return "community" in scope ? `${run(name)}/communities/${encodeURIComponent(scope.community)}/stances` : `${run(name)}/marked/stances`;
}
