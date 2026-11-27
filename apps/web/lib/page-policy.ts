/**
 * What the interface's own server answers, and what its pages may do (proxy.ts applies both).
 *
 * The server forwards /api and /auth to the engine and, in doing so, rewrites `Host` to the engine's own
 * address. The engine's check of `Host` (against DNS rebinding) then sees the right name whatever name the browser
 * used, so the check has to be made here, on the name the browser used: a page of another site whose name was
 * made to point at 127.0.0.1 would otherwise read everything the engine answers through this server, with the
 * token this server adds.
 */

/** Names this computer is reached by. */
const THIS_COMPUTER = new Set(["127.0.0.1", "localhost", "[::1]"]);

/** The host of a `Host` header ("127.0.0.1:3210" → "127.0.0.1", "[::1]:80" → "[::1]"), lower case. */
export function hostName(host: string | null | undefined): string {
	const value = (host ?? "").trim().toLowerCase();
	if (value.startsWith("[")) return value.slice(0, value.indexOf("]") + 1);
	return value.split(":", 1)[0] ?? "";
}

/**
 * The names the interface may be reached by: this computer's, and on a server those listed in
 * `CONSTELLATE_WEB_HOSTS` (comma-separated, "papers.example.org").
 */
export function allowedHosts(listed: string | undefined): ReadonlySet<string> {
	const extra = (listed ?? "")
		.split(",")
		.map((name) => name.trim().toLowerCase())
		.filter(Boolean);
	return new Set([...THIS_COMPUTER, ...extra]);
}

/** Is a request whose `Host` is `host` addressed to this interface under a name of its own? */
export function addressedToUs(host: string | null | undefined, allowed: ReadonlySet<string>): boolean {
	const name = hostName(host);
	return name !== "" && allowed.has(name);
}

/** Paths whose answers are the engine's: it sets their headers itself (the sign-in page has a policy of its own). */
export function isBackendPath(path: string): boolean {
	return path === "/api" || path.startsWith("/api/") || path === "/auth" || path.startsWith("/auth/");
}

/**
 * Is this a request another site's page made, which the browser says in `Sec-Fetch-Site`? Any web page open in a
 * browser on this computer can reach this server, and a request it sends without `Origin` (an image, a script,
 * a frame) would reach the engine with the token this server adds. Only the interface's own pages, a request no
 * page made (a typed address, a program) and the browser being sent to a page (a sign-in coming back from its
 * provider) are forwarded.
 */
export function fromAnotherSite(headers: Headers): boolean {
	const site = headers.get("sec-fetch-site");
	if (site === null || site === "same-origin" || site === "none") return false;
	const navigating = headers.get("sec-fetch-mode") === "navigate" && headers.get("sec-fetch-dest") === "document";
	return !navigating;
}

/** The engine's address when the server is told none: `next dev` started by hand, beside an engine started with
 *  CONSTELLATE_PORT=8765 (`npm run dev`, scripts/dev.mjs, tells it the address instead). */
export const DEV_ENGINE = "http://127.0.0.1:8765";

/** The engine's address: `CONSTELLATE_ENGINE` (the desktop shell sets it to where the engine said it listens). */
export function engineAddress(told: string | undefined): URL {
	const address = new URL(told?.trim() || DEV_ENGINE);
	if (address.protocol !== "http:" && address.protocol !== "https:") throw new Error("CONSTELLATE_ENGINE is not an http(s) address");
	return address;
}

/** The engine's own address for the interface's `url`: the same path and query (`/auth` is the sign-in page, `/auth/`). */
export function engineUrl(engine: URL, url: URL): URL {
	const path = url.pathname === "/auth" ? "/auth/" : url.pathname;
	return new URL(`${path}${url.search}`, engine);
}

/**
 * The headers of a request forwarded to the engine: the browser's, with the engine's token as the only
 * `Authorization` (a page cannot send one of its own along). `Origin` stays the page's, which the engine checks;
 * `Host` becomes the engine's own address on the way (Next's forwarding sets it), and the name the browser used
 * goes along in `X-Forwarded-Host`, which the engine checks too. Without a token (a server, where the owner signs
 * in) nothing is added.
 */
export function engineRequestHeaders(incoming: Headers, token: string | undefined): Headers {
	const headers = new Headers(incoming);
	headers.delete("authorization");
	if (token) headers.set("authorization", `Bearer ${token}`);
	return headers;
}

/**
 * The Content-Security-Policy of the interface's pages: scripts only from this server and those Next marks with
 * the page's nonce, requests only to this server, nothing framed but this server's own pages (a paper's PDF) and
 * https pages (the reader), no plugins, no `<base>`, forms only to this server, and no other site may frame it.
 * Inline styles stay allowed: React's `style={...}`, KaTeX's output and the dialogs' scroll lock (a `<style>` it
 * adds without a nonce) need them, and a style cannot run code. `development`: React's debugging needs `eval`.
 */
export function contentSecurityPolicy(nonce: string, development: boolean): string {
	return [
		"default-src 'self'",
		`script-src 'self' 'nonce-${nonce}' 'strict-dynamic'${development ? " 'unsafe-eval'" : ""}`,
		"style-src 'self' 'unsafe-inline'",
		"img-src 'self' data: blob:",
		"font-src 'self' data:",
		`connect-src 'self'${development ? " ws:" : ""}`,
		"frame-src 'self' https:",
		"worker-src 'self' blob:",
		"object-src 'none'",
		"base-uri 'none'",
		"form-action 'self'",
		"frame-ancestors 'self'",
	].join("; ");
}

/** A nonce: 128 random bits, base64. */
export function newNonce(): string {
	const bytes = crypto.getRandomValues(new Uint8Array(16));
	return btoa(String.fromCharCode(...bytes));
}
