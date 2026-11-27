"use client";

import { useCallback, useEffect, useReducer, useRef } from "react";
import type { JobState, RunRequest, RunStep, Status } from "@rp/contracts";
import { api } from "@/lib/net/api";
import { startRun, type StartDeps, type Started } from "@/lib/shell/run-replace";
import { poller, type Look } from "@/lib/shell/use-followed-job";

/**
 * The pipeline job as the browser sees it. It mirrors the engine's job (apps/engine/src/jobs.rs):
 *
 *     idle --start--> running --> done | failed --start--> running
 *
 * While the job is running, and only then, the engine is polled for new log lines and the next state.
 *
 * A finished job opens a run only when it made one (its `outcome`), and only when this page saw it go from
 * running to finished: a job that had finished before
 * the page asked opens nothing, so that the last run is not opened again at every load. A feature's job is its
 * component's to follow (lib/shell/use-followed-job.ts).
 */
export interface Job {
	state: JobState;
	log: string[];
	error: string | null;
	/** the step of the run being made and how far it has got, while one is (engine `Status.step`) */
	step?: RunStep | null;
}

export type Event =
	| { type: "started" }
	| { type: "reported"; status: Status }
	// the engine did not answer, or refused to start a run: what the job is doing is unchanged
	| { type: "unanswered"; error: string };

/**
 * The run to open when `status` follows `before`: the run a job made, when this page saw that job running and now
 * sees it done; null otherwise (a feature's job, nothing to open, or a job that had finished already).
 */
export function finishedRun(before: Job, status: Status): string | null {
	if (before.state !== "running" || status.state !== "done") return null;
	return status.outcome?.kind === "run" ? status.outcome.run : null;
}

/**
 * Is this a feature's job rather than a run (or the declarations of one)? A feature shows its own job's progress
 * and failure beside the button that started it (lib/shell/use-followed-job.ts); the run's log keeps its lines, but its
 * failure is not the run's ("New run" would say the extraction table's budget ran out). The engine says which
 * feature a job is for from its start (`feature`), also once it failed, and a finished one's `outcome` says it too.
 */
export function isFeatureJob(status: Pick<Status, "feature" | "outcome">): boolean {
	return status.feature != null || status.outcome?.kind === "feature";
}

export function reduce(job: Job, event: Event): Job {
	switch (event.type) {
		case "started":
			return { state: "running", log: [], error: null };
		case "reported": {
			const status = event.status;
			// a feature's failure is said by its own section, not under "New run" as if a run had failed
			const said = isFeatureJob(status) ? null : status.error;
			// the step of the run being made, while it is made (shown by the folded log, components/shell/workspace.tsx)
			const step = status.state === "running" ? (status.step ?? null) : null;
			// the poller's look carries the job's whole log (lib/shell/use-followed-job.ts `createPoller`)
			return { state: status.state, log: status.log, error: said, step };
		}
		case "unanswered":
			return { ...job, error: event.error };
	}
}

/**
 * The run's job, looked at through the window's one poller of `/api/status` (lib/shell/use-followed-job.ts `poller`),
 * which the features' sections share: the engine is asked once however many of them look, and an engine that does
 * not answer is asked less and less often (the poller's backoff).
 */
export function useJob(onDone: (run: string) => void, ask: StartDeps["ask"]) {
	const [job, dispatch] = useReducer(reduce, { state: "idle", log: [], error: null });
	// the job as this page last saw it, for telling a finish it watched from one it only heard of
	const last = useRef<Job>(job);
	last.current = job;
	const done = useRef(onDone);
	done.current = onDone;

	const listen = useCallback((look: Look) => {
		if (!("seen" in look)) return dispatch({ type: "unanswered", error: look.unanswered });
		const run = finishedRun(last.current, look.seen);
		dispatch({ type: "reported", status: look.seen });
		if (run) done.current(run);
	}, []);

	// Pick up a job that was already running when the page opened.
	useEffect(() => {
		let gone = false;
		void poller.probe().then((look) => gone || listen(look));
		return () => {
			gone = true;
		};
	}, [listen]);

	// Looked at while running; any other state asks nothing.
	const running = job.state === "running";
	useEffect(() => (running ? poller.watch(listen) : undefined), [running, listen]);

	const asking = useRef(ask);
	asking.current = ask;
	/** Start a run: one that would replace a run, or pay for names, only once the person agreed
	 *  (lib/shell/run-replace.ts). Answers how it went, or null when the engine refused (said as the job's error). */
	const start = useCallback(
		async (request: RunRequest): Promise<Started | null> => {
			try {
				const started = await startRun(request, {
					send: api.start,
					beforeRun: api.describeBeforeRun,
					ask: (question) => asking.current(question),
				});
				if (started.kind === "started") {
					poller.restart(); // a look asked before would tell the last job's end as this one's
					dispatch({ type: "started" });
				}
				return started;
			} catch (error) {
				dispatch({ type: "unanswered", error: (error as Error).message });
				void poller.probe().then(listen); // "already running" is a refusal too: show what the engine is really doing
				return null;
			}
		},
		[listen],
	);

	/** A job was started some other way (not through `start`): begin showing it. */
	const follow = useCallback(() => {
		poller.restart();
		dispatch({ type: "started" });
		void poller.probe().then(listen);
	}, [listen]);

	return { job, start, follow };
}
