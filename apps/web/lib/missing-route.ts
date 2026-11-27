/**
 * Does an error say the backend has no such route, so that it is an older one (the Python backend) and the feature
 * is not offered? Both backends answer an unknown route with exactly `{"error": "not found"}` (lib/api.ts throws
 * its `error`), or with no body ("The backend answered 404"). Only those: a longer error that happens to contain
 * the words ("the quote was not found in the paper") is a real failure of a route that exists, and is shown.
 */
export function isMissingRoute(error: unknown): boolean {
	const message = error instanceof Error ? error.message : typeof error === "string" ? error : null;
	return message !== null && (/^\s*not found\s*$/i.test(message) || /answered 404\b/i.test(message));
}
