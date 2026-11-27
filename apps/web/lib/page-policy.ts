/**
 * What the interface's own server answers, and what its pages may do (proxy.ts applies both).
 *
 * The server forwards /api and /auth to the backend and, in doing so, rewrites `Host` to the backend's own
 * address. A backend's check of `Host` (against DNS rebinding) then sees the right name whatever name the browser
 * used, so the check has to be made here, on the name the browser used: a page of another site whose name was
 * made to point at 127.0.0.1 would otherwise read everything the backend answers through this server.
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

/** Paths whose answers are the backend's: it sets their headers itself (the sign-in page has a policy of its own). */
export function isBackendPath(path: string): boolean {
	return path === "/api" || path.startsWith("/api/") || path === "/auth" || path.startsWith("/auth/");
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
