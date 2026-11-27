/**
 * One request to the engine, with a deadline, a way for the asker to call it off, and an error that says what
 * happened. `lib/api.ts` `call()` adds what is the window's (the session's token, signing in again) on top.
 *
 * A request ends in exactly one of these: an answer (`Answer`: the status and the body read whole), or a
 * `RequestError` whose `problem` says which of the ways it went wrong:
 *
 *     unreachable   nothing answered at all (the engine is not running, the network is down)
 *     timeout       nothing came back within the request's deadline
 *     aborted       the asker no longer wanted it (a component went away, or asked for something else)
 *     refused       the engine answered with an error status (its `{ "error": ... }`, when it sent one)
 *     unreadable    an answer of success whose body is not JSON, is empty, or was cut off half-way
 *
 * Retries: a request that could cost money or change something is never sent twice by this file. A GET is
 * asked once more when nothing answered at all (a dropped connection), never after a deadline passed or an
 * answer came; the poller of `/api/status` asks with `retry: "never"`, its own backoff (lib/use-followed-job.ts
 * `pollDelay`) being the retry.
 */

/** Why a request has no usable answer. */
export type RequestProblem =
	| { kind: "unreachable" }
	| { kind: "timeout"; seconds: number }
	| { kind: "aborted" }
	| { kind: "refused"; status: number; error: string | null }
	| { kind: "unreadable"; status: number; why: "not_json" | "empty" | "cut_off" };

/** What `call()` throws: the problem for code, its plain words for people, and the HTTP status when one came. */
export class RequestError extends Error {
	readonly problem: RequestProblem;
	/** the status of the answer that came, null when none came (unreachable, timeout, aborted) */
	readonly status: number | null;
	constructor(problem: RequestProblem) {
		super(problemMessage(problem));
		this.name = "RequestError";
		this.problem = problem;
		this.status = problem.kind === "refused" || problem.kind === "unreadable" ? problem.status : null;
	}
}

/** The problem of a thrown value, when it is a request's. */
export function problemOf(error: unknown): RequestProblem | null {
	return error instanceof RequestError ? error.problem : null;
}

/** The problem in the words a person reads. */
export function problemMessage(problem: RequestProblem): string {
	switch (problem.kind) {
		case "unreachable":
			return "The engine is not reachable. Start it with: npm run dev";
		case "timeout":
			return `The engine did not answer within ${problem.seconds} seconds.`;
		case "aborted":
			return "The request was called off.";
		case "refused":
			if (problem.error) return problem.error;
			// the engine's own limit on a request's time (apps/engine api/routes.rs PATIENCE) answers 408 with no body
			if (problem.status === 408) return "The engine stopped working on this request: it took too long.";
			return `The engine answered ${problem.status}.`;
		case "unreadable":
			switch (problem.why) {
				case "not_json":
					return `The engine's answer could not be read: it is not JSON (status ${problem.status}).`;
				case "empty":
					return `The engine's answer was empty (status ${problem.status}).`;
				case "cut_off":
					return `The engine's answer was cut off before its end (status ${problem.status}).`;
			}
	}
}

/** How long a request may take, and whether it may be asked again when nothing answered. */
export interface Patience {
	/** the deadline, in milliseconds, from the request's start to its body read whole */
	ms: number;
	retry: "once_if_unanswered" | "never";
}

/** Reads, estimates, settings, starting a job: the engine answers these from what it keeps, at once. */
export const QUICK: Patience = { ms: 20_000, retry: "once_if_unanswered" };
/**
 * Routes that may read a paper's PDF (or fetch it) the first time they are asked: its passages, where a quote is,
 * the weaknesses or claims view, an explanation's preparation, a paper's citing sentences, an estimate over a
 * community's papers. The engine itself gives up on any request after 30 seconds (408), so this is past that:
 * its own answer comes first. Anything longer than this is a job (a run, the declarations of a run).
 */
export const READS_PAPERS: Patience = { ms: 60_000, retry: "once_if_unanswered" };
/** Starting a paid job: past the engine's own 30 seconds, so that its answer (or its 408) always comes first. */
export const STARTS_JOB: Patience = { ms: 60_000, retry: "never" };
/** `/api/status`: the poller's backoff is its retry. */
export const POLLING: Patience = { ms: 20_000, retry: "never" };

/** How long to wait before asking a GET once more after nothing answered. */
export const RETRY_PAUSE_MS = 400;

/** Is this method one that may be sent twice without changing anything (and costing nothing)? */
export function idempotent(method: string | undefined): boolean {
	return ["GET", "HEAD"].includes((method ?? "GET").toUpperCase());
}

/** What came back: its status, its headers, and its body as read. */
export interface Answer {
	status: number;
	ok: boolean;
	headers: Headers;
	body: { kind: "json"; value: unknown } | { kind: "empty" } | { kind: "not_json" };
}

/** The body of an answer of success, or the problem it is: an error status, or a body that cannot be read. */
export function answered<T>(answer: Answer): T {
	if (!answer.ok) {
		const told = answer.body.kind === "json" ? (answer.body.value as { error?: unknown } | null) : null;
		const error = typeof told?.error === "string" && told.error.trim() !== "" ? told.error : null;
		throw new RequestError({ kind: "refused", status: answer.status, error });
	}
	if (answer.body.kind === "json") return answer.body.value as T;
	throw new RequestError({ kind: "unreadable", status: answer.status, why: answer.body.kind });
}

export interface RequestOptions {
	patience: Patience;
	/** the asker's: aborting it calls the request off (`RequestError` "aborted") */
	signal?: AbortSignal | null;
	/** the tests' own fetch */
	fetcher?: typeof fetch;
}

/** The body read whole, or "cut_off" when the connection broke while it was read. */
async function bodyOf(response: Response): Promise<Answer["body"] | "cut_off"> {
	let text: string;
	try {
		text = await response.text();
	} catch {
		return "cut_off";
	}
	if (text.trim() === "") return { kind: "empty" };
	try {
		return { kind: "json", value: JSON.parse(text) };
	} catch {
		return { kind: "not_json" };
	}
}

/**
 * Send `init` to `path` and read its answer whole, within `patience.ms`. Throws a `RequestError` for every way it
 * can fail to get an answer (an answer with an error status is returned: `answered` says what it is).
 */
export async function request(path: string, init: RequestInit | undefined, options: RequestOptions): Promise<Answer> {
	const { patience, signal, fetcher = fetch } = options;
	if (signal?.aborted) throw new RequestError({ kind: "aborted" });
	const stop = new AbortController();
	// what ended the request before its answer, in the order it happened: the first one is the one said
	let ended: RequestProblem | null = null;
	let fail: (problem: RequestProblem) => void = () => {};
	const given = new Promise<never>((_, reject) => {
		fail = (problem) => {
			ended ??= problem;
			stop.abort();
			reject(new RequestError(ended));
		};
	});
	given.catch(() => {}); // only ever awaited in a race
	const timer = setTimeout(() => fail({ kind: "timeout", seconds: Math.round(patience.ms / 1000) }), patience.ms);
	const onAbort = () => fail({ kind: "aborted" });
	signal?.addEventListener("abort", onAbort, { once: true });
	// a fetch (or a body) that does not heed the abort is still not waited for past the deadline
	const within = <T>(promise: Promise<T>) => Promise.race([promise, given]);
	try {
		const retries = patience.retry === "once_if_unanswered" && idempotent(init?.method) ? 1 : 0;
		for (let attempt = 0; ; attempt += 1) {
			let response: Response;
			try {
				response = await within(fetcher(path, { cache: "no-store", ...init, signal: stop.signal }));
			} catch (error) {
				if (ended !== null) throw new RequestError(ended);
				if (error instanceof RequestError) throw error;
				if (attempt >= retries) throw new RequestError({ kind: "unreachable" });
				let pause: ReturnType<typeof setTimeout> | undefined;
				try {
					await within(new Promise((resolve) => (pause = setTimeout(resolve, RETRY_PAUSE_MS))));
				} finally {
					clearTimeout(pause);
				}
				continue;
			}
			const body = await within(bodyOf(response));
			if (ended !== null) throw new RequestError(ended);
			if (body === "cut_off") throw new RequestError({ kind: "unreadable", status: response.status, why: "cut_off" });
			return { status: response.status, ok: response.ok, headers: response.headers, body };
		}
	} finally {
		clearTimeout(timer);
		signal?.removeEventListener("abort", onAbort);
	}
}

/**
 * The signal of the request being made right now from inside `withSignal`: how `useAsync` hands its signal to
 * the `api` functions without each one taking it. `call()` reads it before its first `await`, so only the
 * request a `withSignal` callback starts synchronously gets it; one started later (after an await) does not.
 */
let ambient: AbortSignal | null = null;

/**
 * Run `ask` (which starts requests through `api`) so that the requests it starts are called off by `signal`; with
 * null, so that they are called off by nobody: a request kept and shared by several askers (a cache of promises)
 * is started that way, so that the first asker going away does not call it off for the others.
 */
export function withSignal<T>(signal: AbortSignal | null, ask: () => T): T {
	const before = ambient;
	ambient = signal;
	try {
		return ask();
	} finally {
		ambient = before;
	}
}

/** The signal `withSignal` set for the request starting now, if any. */
export function currentSignal(): AbortSignal | null {
	return ambient;
}
