import { type NextRequest, NextResponse } from "next/server";
import {
	addressedToUs,
	allowedHosts,
	contentSecurityPolicy,
	engineAddress,
	engineRequestHeaders,
	engineUrl,
	fromAnotherSite,
	isBackendPath,
	newNonce,
	windowAdmitted,
} from "@/lib/page-policy";

/**
 * Runs before every request the interface's server answers (lib/page-policy.ts says why): a request addressed to
 * a name that is not the interface's is refused; /api and /auth are forwarded to the engine, with the engine's
 * token added on that hop alone (the page never has it); every page gets a Content-Security-Policy with a nonce
 * of its own, which Next puts on the scripts it writes.
 *
 * The forwarding is Next's own (a rewrite to another server): the request's body and the answer stream through
 * as they come, byte ranges (a PDF in the reader) and a page that stops waiting included. Headers set for the
 * engine here are the request's only: Next sends none of them back to the browser (it does name the engine's
 * address, not its token, in `x-middleware-rewrite`).
 */
export function proxy(request: NextRequest) {
	if (!addressedToUs(request.headers.get("host"), allowedHosts(process.env.CONSTELLATE_WEB_HOSTS))) {
		return NextResponse.json({ error: "this interface answers only requests addressed to it" }, { status: 403 });
	}
	if (isBackendPath(request.nextUrl.pathname)) {
		const path = request.nextUrl.pathname;
		if (fromAnotherSite(request.headers, path) || !windowAdmitted(request.headers.get("cookie"), process.env.CONSTELLATE_WINDOW_KEY)) {
			return NextResponse.json({ error: "this interface answers only its own pages" }, { status: 403 });
		}
		const engine = engineAddress(process.env.CONSTELLATE_ENGINE);
		const headers = engineRequestHeaders(request.headers, process.env.CONSTELLATE_ENGINE_TOKEN);
		return NextResponse.rewrite(engineUrl(engine, request.nextUrl), { request: { headers } });
	}
	const policy = contentSecurityPolicy(newNonce(), process.env.NODE_ENV === "development");
	const headers = new Headers(request.headers);
	headers.set("Content-Security-Policy", policy); // Next reads the nonce from here while it renders the page
	const response = NextResponse.next({ request: { headers } });
	response.headers.set("Content-Security-Policy", policy);
	response.headers.set("X-Content-Type-Options", "nosniff");
	response.headers.set("Referrer-Policy", "no-referrer");
	return response;
}

export const config = {
	// every path but the build's own files (scripts and styles, which carry no policy of their own)
	matcher: ["/((?!_next/static).*)"],
};
