import type { Settings, SignInSession, Status } from "@rp/contracts";

/** Where the owner signs in, on a server (apps/engine/src/api/login). */
export const SIGN_IN_PAGE = "/auth/";

/** The header a server reads the session's anti-forgery token from. */
export const CSRF_HEADER = "X-Constellate-Csrf";

/** The settings this engine decides itself: none on the user's own computer (and from the Python backend). */
export function serverOwned(status: Pick<Status, "mode" | "server_owned"> | undefined): ReadonlySet<keyof Settings> {
	if (status?.mode !== "network") return new Set();
	return new Set(status.server_owned ?? []);
}

/** A group's fields split into those the window may change and those the server decides. */
export function splitByOwner<F extends { key: keyof Settings }>(fields: readonly F[], owned: ReadonlySet<keyof Settings>) {
	return {
		shown: fields.filter((field) => !owned.has(field.key)),
		decided: fields.filter((field) => owned.has(field.key)),
	};
}

/**
 * Where to send the window after an answer: to the sign-in page when a server says nobody is signed in (its 401
 * names the `Session` scheme). A 401 on the user's own computer is a wrong token, which signing in cannot mend.
 */
export function signInRedirect(status: number, challenge: string | null, path: string): string | null {
	const onSignInPage = path === "/auth" || path.startsWith(SIGN_IN_PAGE);
	const signInNeeded = status === 401 && (challenge ?? "").trim().toLowerCase().startsWith("session");
	return signInNeeded && !onSignInPage ? SIGN_IN_PAGE : null;
}

/** Does a request with this method change something (and so need the session's token on a server)? */
export function changesSomething(method: string | undefined): boolean {
	return !["GET", "HEAD", "OPTIONS"].includes((method ?? "GET").toUpperCase());
}

/** `init` with the session's token added when the request changes something and there is a token. */
export function withCsrf(init: RequestInit | undefined, csrf: string | null): RequestInit | undefined {
	if (!csrf || !changesSomething(init?.method)) return init;
	const headers = new Headers(init?.headers);
	headers.set(CSRF_HEADER, csrf);
	return { ...init, headers };
}

/** The session's token from what `GET /auth/session` answered; null on the user's own computer or when signed out. */
export function csrfOf(session: unknown): string | null {
	const told = session as Partial<SignInSession> | null;
	return told?.signed_in && typeof told.csrf === "string" ? told.csrf : null;
}
