"use client";

import { useCallback, useEffect, useState, type DependencyList } from "react";

/** Something fetched: exactly one of these at a time, so "loading with stale data and an error" cannot exist. */
export type Async<T> =
	| { status: "idle" }
	| { status: "loading" }
	| { status: "ready"; data: T }
	| { status: "failed"; error: string };

/**
 * Run `load` whenever `deps` change and report where it stands. Pass null to stay idle (nothing to load yet).
 * An answer that arrives after the deps changed again is dropped. `reload` runs it once more.
 */
export function useAsync<T>(load: (() => Promise<T>) | null, deps: DependencyList): Async<T> & { reload: () => void } {
	const [state, setState] = useState<Async<T>>({ status: load ? "loading" : "idle" });
	const [version, setVersion] = useState(0);
	const active = load !== null;

	useEffect(() => {
		if (!load) {
			setState({ status: "idle" });
			return;
		}
		let stale = false;
		setState((previous) => (previous.status === "ready" ? previous : { status: "loading" }));
		load().then(
			(data) => !stale && setState({ status: "ready", data }),
			(problem: Error) => !stale && setState({ status: "failed", error: problem.message }),
		);
		return () => {
			stale = true;
		};
		// `load` is a fresh closure every render; the caller's deps say when it really changed.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [...deps, active, version]);

	const reload = useCallback(() => setVersion((value) => value + 1), []);
	return { ...state, reload };
}

/** The data when ready, otherwise undefined. */
export function ready<T>(state: Async<T>): T | undefined {
	return state.status === "ready" ? state.data : undefined;
}
