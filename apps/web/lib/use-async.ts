"use client";

import { useCallback, useEffect, useRef, useState, type DependencyList } from "react";
import { problemOf, withSignal, type RequestProblem } from "@/lib/request";

/**
 * Something fetched, in exactly one of these at a time:
 *
 *     idle        nothing to load (the caller passed null)
 *     loading     asked for, nothing known yet for what was asked
 *     ready       the answer
 *     reloading   asked again while the last answer stays shown (`reload`, or new deps with `keep`)
 *     failed      the answer is an error; `data` is the answer before it when asking again failed, so what
 *                 the user was reading is not taken away; `problem` is what went wrong for code (a request's
 *                 status: lib/missing-route.ts) when the error was a request's
 *
 * A spinner and an error at once cannot be written, nor an error that hides the answer it came after.
 */
export type Async<T> =
	| { status: "idle" }
	| { status: "loading" }
	| { status: "ready"; data: T }
	| { status: "reloading"; data: T }
	| { status: "failed"; error: string; problem?: RequestProblem; data?: T };

/** What happens to a fetch: asked for anew (other deps), asked again (`reload`), answered, refused, or not needed. */
export type AsyncEvent<T> =
	| { kind: "ask" }
	| { kind: "reask" }
	| { kind: "answer"; data: T }
	| { kind: "refuse"; error: string; problem?: RequestProblem }
	| { kind: "clear" };

/** The answer kept for the deps it was asked for, if any. */
function kept<T>(state: Async<T>): T | undefined {
	return state.status === "idle" || state.status === "loading" ? undefined : state.data;
}

/**
 * The one place a fetch's state changes. Asking anew forgets the last answer (it was for something else);
 * asking again keeps it in view. An answer or a refusal that no request is waiting for changes nothing.
 */
export function asyncStep<T>(state: Async<T>, event: AsyncEvent<T>): Async<T> {
	switch (event.kind) {
		case "clear":
			return { status: "idle" };
		case "ask":
			return { status: "loading" };
		case "reask": {
			const data = kept(state);
			return data === undefined ? { status: "loading" } : { status: "reloading", data };
		}
		case "answer":
			return state.status === "loading" || state.status === "reloading" ? { status: "ready", data: event.data } : state;
		case "refuse": {
			if (state.status !== "loading" && state.status !== "reloading") return state;
			const failed = { status: "failed" as const, error: event.error, ...(event.problem ? { problem: event.problem } : {}) };
			return state.status === "reloading" ? { ...failed, data: state.data } : failed;
		}
	}
}

const sameDeps = (one: DependencyList, other: DependencyList) =>
	one.length === other.length && one.every((value, index) => Object.is(value, other[index]));

export interface AsyncOptions {
	/**
	 * Does the last answer stay in view (as "reloading") while new deps are being answered? Yes (the default)
	 * where the deps only say how fresh it should be, such as a counter bumped after a change elsewhere. No where
	 * they say what is asked about (a paper, a community, a quote): the last answer is another thing's, and is
	 * never shown for the new one, not even for the one render before the new request starts. `reload` keeps
	 * it either way: it asks again about the same thing.
	 */
	keep?: boolean;
}

/** What a change of deps (or of `load` to null) does to what is held. */
export function asked<T>(held: Async<T>, active: boolean, keep: boolean): Async<T> {
	if (!active) return asyncStep(held, { kind: "clear" });
	return asyncStep(held, { kind: keep ? "reask" : "ask" });
}

/**
 * Start `load`, telling `step` its answer or refusal, until the returned function is called: from then on what it
 * answers is dropped and its requests are called off (the signal `load` is given, which `withSignal` also hands to
 * every `api` request `load` starts before its first await). What `useAsync` does for each request, so that one
 * superseded by the next (other deps, `reload`) or by its component going away is aborted, not only ignored.
 */
export function startLoad<T>(load: (signal: AbortSignal) => Promise<T>, step: (event: AsyncEvent<T>) => void): () => void {
	const controller = new AbortController();
	let stale = false;
	let asked: Promise<T>;
	try {
		asked = withSignal(controller.signal, () => load(controller.signal));
	} catch (error) {
		asked = Promise.reject(error);
	}
	asked.then(
		(data) => !stale && step({ kind: "answer", data }),
		(error: unknown) => {
			if (stale) return;
			const problem = problemOf(error);
			step({ kind: "refuse", error: error instanceof Error ? error.message : String(error), ...(problem ? { problem } : {}) });
		},
	);
	return () => {
		stale = true;
		controller.abort();
	};
}

/**
 * Run `load` whenever `deps` change and report where it stands. Pass null to stay idle (nothing to load yet).
 * An answer that arrives after the deps changed again is dropped and its request called off (`startLoad`);
 * `reload` runs it once more.
 */
export function useAsync<T>(
	load: ((signal: AbortSignal) => Promise<T>) | null,
	deps: DependencyList,
	{ keep = true }: AsyncOptions = {},
): Async<T> & { reload: () => void } {
	const active = load !== null;
	const key = [...deps, active];
	// the state, and the deps it is for
	const [held, setHeld] = useState<{ key: DependencyList; state: Async<T> }>(() => ({
		key,
		state: active ? { status: "loading" } : { status: "idle" },
	}));
	const [version, setVersion] = useState(0);
	const last = useRef<{ key: DependencyList; version: number } | null>(null);

	useEffect(() => {
		const before = last.current;
		last.current = { key, version };
		// the same deps, and only `reload` moved: asked again about the same thing
		const again = before !== null && sameDeps(before.key, key) && before.version !== version;
		setHeld((now) => ({ key, state: again ? asyncStep(now.state, { kind: "reask" }) : asked(now.state, active, keep) }));
		if (!load) return;
		const step = (event: AsyncEvent<T>) => setHeld((now) => (sameDeps(now.key, key) ? { key, state: asyncStep(now.state, event) } : now));
		return startLoad(load, step);
		// `load` is a fresh closure every render; the caller's deps say when it really changed.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [...key, version]);

	const reload = useCallback(() => setVersion((value) => value + 1), []);
	// the deps changed and the effect has not run yet: shown as it will be once it has
	const state = sameDeps(held.key, key) ? held.state : asked(held.state, active, keep);
	return { ...state, reload };
}

/**
 * The answer to show: when ready, while it is being asked again, and after asking again failed (beside the
 * error). Undefined while nothing is known for what was asked.
 */
export function ready<T>(state: Async<T>): T | undefined {
	return kept(state);
}
