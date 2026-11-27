import { problemOf, type RequestProblem } from "@/lib/net/request";

/**
 * Does a request's failure say the paper has no PDF the engine can have or read? `GET /api/papers/{id}/passages`
 * answers that with status 404 and exactly `{"error": "not found"}`, or with no error of its own (lib/net/request.ts
 * `RequestError`). Only those: a 404 with a longer error ("the quote was not found in the paper") is a real failure
 * and is shown; so is any other status, and a request that got no answer at all.
 *
 * Takes what was thrown (a `RequestError`) or the problem a failed `useAsync` keeps.
 */
export function noReadablePdf(failure: unknown): boolean {
	const problem: RequestProblem | null = problemOf(failure) ?? (isProblem(failure) ? failure : null);
	if (problem?.kind !== "refused" || problem.status !== 404) return false;
	return problem.error === null || /^\s*not found\s*$/i.test(problem.error);
}

function isProblem(value: unknown): value is RequestProblem {
	return typeof value === "object" && value !== null && typeof (value as { kind?: unknown }).kind === "string";
}
