import type {
	Catalog,
	Declarations,
	GraphData,
	Library,
	OverviewStatus,
	Paper,
	Ratings,
	RunRequest,
	RunResult,
	RunSummary,
	Settings,
	Status,
	Usage,
} from "@rp/contracts";

async function call<T>(path: string, init?: RequestInit): Promise<T> {
	let response: Response;
	try {
		response = await fetch(path, { cache: "no-store", ...init });
	} catch {
		throw new Error("The backend is not reachable. Start it with: npm run dev");
	}
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
	readRatings: (id: string, text: string, url: string) => call<Ratings | null>(`/api/reviews/${id}`, send("POST", { text, url })),
	/** start the job that does that for every paper of a run */
	readDeclarations: (name: string) => call<{ ok: true }>(`${run(name)}/declarations`, { method: "POST" }),
};
