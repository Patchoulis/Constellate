/**
 * Does an error say the paper has no PDF the engine can have or read? `GET /api/papers/{id}/passages` answers that
 * with a bare 404, `{"error": "not found"}` (lib/api.ts throws its `error`), or with no body ("The backend answered
 * 404"). Only those: a longer error that happens to contain the words ("the quote was not found in the paper") is a
 * real failure, and is shown.
 */
export function noReadablePdf(error: unknown): boolean {
	const message = error instanceof Error ? error.message : typeof error === "string" ? error : null;
	return message !== null && (/^\s*not found\s*$/i.test(message) || /answered 404\b/i.test(message));
}
