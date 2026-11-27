import { type NextRequest, NextResponse } from "next/server";
import { addressedToUs, allowedHosts, contentSecurityPolicy, isBackendPath, newNonce } from "@/lib/page-policy";

/**
 * Runs before every request the interface's server answers, the forwarded /api and /auth included
 * (lib/page-policy.ts says why): a request addressed to a name that is not the interface's is refused, and every
 * page gets a Content-Security-Policy with a nonce of its own, which Next puts on the scripts it writes.
 */
export function proxy(request: NextRequest) {
	if (!addressedToUs(request.headers.get("host"), allowedHosts(process.env.CONSTELLATE_WEB_HOSTS))) {
		return NextResponse.json({ error: "this interface answers only requests addressed to it" }, { status: 403 });
	}
	if (isBackendPath(request.nextUrl.pathname)) return NextResponse.next();
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
