import { problemOf, type RequestProblem } from "@/lib/request";

/**
 * Does a request's failure say the backend has no such route, so that it is an older one (the Python backend) and
 * the feature is not offered? Both backends answer an unknown route with status 404 and exactly
 * `{"error": "not found"}`, or with no error of their own (lib/request.ts `RequestError`). Only those: a 404 with a
 * longer error ("the quote was not found in the paper") is a real failure of a route that exists, and is shown;
 * so is any other status, and a request that got no answer at all.
 *
 * Takes what was thrown (a `RequestError`) or the problem a failed `useAsync` keeps.
 */
export function isMissingRoute(failure: unknown): boolean {
	const problem: RequestProblem | null = problemOf(failure) ?? (isProblem(failure) ? failure : null);
	if (problem?.kind !== "refused" || problem.status !== 404) return false;
	return problem.error === null || /^\s*not found\s*$/i.test(problem.error);
}

function isProblem(value: unknown): value is RequestProblem {
	return typeof value === "object" && value !== null && typeof (value as { kind?: unknown }).kind === "string";
}
