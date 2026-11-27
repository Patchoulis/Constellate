import type {
	Catalog,
	CitationNetwork,
	Declarations,
	GraphData,
	Library,
	OverviewStatus,
	Paper,
	PaperReviews,
	Ratings,
	ReviewsRead,
	ReviewSummary,
	RunRequest,
	RunResult,
	RunSummary,
	Settings,
	Status,
	Usage,
} from "@rp/contracts";
import { changesSomething, csrfOf, signInRedirect, withCsrf } from "@/lib/engine";

/** On a server, the session's token that every change carries, asked for once; null on the user's own computer. */
let csrf: Promise<string | null> | null = null;
const sessionToken = () =>
	(csrf ??= fetch("/auth/session", { cache: "no-store" })
		.then((response) => (response.ok ? response.json() : null))
		.then(csrfOf, () => null));

async function call<T>(path: string, init?: RequestInit): Promise<T> {
	let response: Response;
	try {
		const token = changesSomething(init?.method) ? await sessionToken() : null;
		response = await fetch(path, { cache: "no-store", ...withCsrf(init, token) });
	} catch {
		throw new Error("The backend is not reachable. Start it with: npm run dev");
	}
	// a server whose session ended (or was never started): sign in again; a refused token is asked for anew
	if (response.status === 401 || response.status === 403) csrf = null;
	const signIn = signInRedirect(response.status, response.headers.get("WWW-Authenticate"), window.location.pathname);
	if (signIn) window.location.assign(signIn);
	const body = await response.json().catch(() => ({}));
	if (!response.ok) throw new Error(body.error ?? `The backend answered ${response.status}`);
	return body as T;
}

const send = (method: string, body: unknown): RequestInit => ({
	method,
	headers: { "Content-Type": "application/json" },
	body: JSON.stringify(body),
});

const run = (name: string) => `/api/runs/${encodeURIComponent(name)}`;

/** Hand over the text of a paper's reviews page and its address: the engine keeps the whole discussion. */
const readReviews = (id: string, text: string, url: string) =>
	call<ReviewsRead>(`/api/papers/${encodeURIComponent(id)}/reviews`, send("POST", { text, url }));

export const api = {
	runs: () => call<RunSummary[]>("/api/runs"),
	run: (name: string) => call<RunResult>(run(name)),
	graph: (name: string) => call<GraphData>(`${run(name)}/graph`),
	rename: (name: string, title: string) => call<{ title: string }>(run(name), send("PATCH", { title })),
	remove: (name: string) => call<{ ok: true }>(run(name), { method: "DELETE" }),
	overview: (name: string, treePath: string, write = false) =>
		call<OverviewStatus>(`${run(name)}/overview/${encodeURIComponent(treePath)}`, { method: write ? "POST" : "GET" }),
	start: (request: RunRequest) => call<{ ok: true }>("/api/runs", send("POST", request)),
	status: (since: number) => call<Status>(`/api/status?since=${since}`),
	settings: () => call<Settings>("/api/settings"),
	saveSettings: (changes: Partial<Settings>) => call<Settings>("/api/settings", send("PUT", changes)),
	library: () => call<Library>("/api/library"),
	/** what was spent, with the worth of a US dollar in `currency` (a three-letter code) */
	usage: (currency: string) => call<Usage>(`/api/usage?currency=${encodeURIComponent(currency)}`),
	catalog: () => call<Catalog>("/api/catalog"),
	paper: (id: string) => call<Paper>(`/api/papers/${id}`),
	pdfUrl: (id: string) => `/api/pdf/${id}`,
	/** read what a paper's authors declare, from its PDF (slow the first time: the PDF is fetched) */
	declarations: (id: string) => call<Declarations>(`/api/declarations/${id}`, { method: "POST" }),
	/** the reviewers' scores of every paper whose reviews page has been opened, by paper id */
	ratings: () => call<Record<string, Ratings>>("/api/reviews"),
	/** hand over the text of a paper's reviews page and the page's address; answers the scores now kept (null if none) */
	readRatings: (id: string, text: string, url: string) =>
		readReviews(id, text, url).then(
			(read) => (read.outcome === "not_its_page" ? null : read.ratings),
			// the Python backend, which has no such route, until the engine serves /api: its scores-only route
			(error: Error) => {
				if (!/not found|answered 404/.test(error.message)) throw error;
				return call<Ratings | null>(`/api/reviews/${encodeURIComponent(id)}`, send("POST", { text, url }));
			},
		),
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
		call<CitationNetwork>(`/api/papers/${id}/citations${inRun ? `?run=${encodeURIComponent(inRun)}` : ""}`),
	/** read this paper's reference list from its PDF (slow the first time: the PDF is fetched); answers its network */
	readReferences: (id: string, inRun?: string) =>
		call<CitationNetwork>(`/api/papers/${id}/references${inRun ? `?run=${encodeURIComponent(inRun)}` : ""}`, { method: "POST" }),
};
