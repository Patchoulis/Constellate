"use client";

import { useCallback, useEffect, useReducer, useRef } from "react";
import type { JobState, RunRequest, Status } from "@rp/contracts";
import { api } from "@/lib/api";

/**
 * The pipeline job as the browser sees it. It mirrors the backend's state machine (rp/jobs.py):
 *
 *     idle --start--> running --> done | failed --start--> running
 *
 * While the job is running, and only then, the backend is polled for new log lines and the next state.
 *
 * A finished job opens a run only when it made one (its `outcome`; a Python backend's finished jobs are all runs,
 * named in `result`), and only when this page saw it go from running to finished: a job that had finished before
 * the page asked opens nothing, so that the last run is not opened again at every load. A feature's job is its
 * component's to follow (lib/use-followed-job.ts).
 */
export interface Job {
	state: JobState;
	log: string[];
	error: string | null;
}

export type Event =
	| { type: "started" }
	| { type: "reported"; status: Status }
	// the backend did not answer, or refused to start a run: what the job is doing is unchanged
	| { type: "unanswered"; error: string };

/**
 * The run to open when `status` follows `before`: the run a job made, when this page saw that job running and now
 * sees it done; null otherwise (a feature's job, nothing to open, or a job that had finished already).
 */
export function finishedRun(before: Job, status: Status): string | null {
	if (before.state !== "running" || status.state !== "done") return null;
	if (status.outcome === undefined) return status.result; // the Python backend: every job is a run
	return status.outcome?.kind === "run" ? status.outcome.run : null;
}

export function reduce(job: Job, event: Event): Job {
	switch (event.type) {
		case "started":
			return { state: "running", log: [], error: null };
		case "reported":
			return { state: event.status.state, log: [...job.log, ...event.status.log], error: event.status.error };
		case "unanswered":
			return { ...job, error: event.error }; // a new object: while running, the next poll is still scheduled
	}
}

const POLL_MS = 1200;

export function useJob(onDone: (run: string) => void) {
	const [job, dispatch] = useReducer(reduce, { state: "idle", log: [], error: null });
	const seen = useRef(0);
	seen.current = job.log.length;
	// the job as this page last saw it, for telling a finish it watched from one it only heard of
	const last = useRef<Job>(job);
	last.current = job;
	const done = useRef(onDone);
	done.current = onDone;

	const report = useCallback(async () => {
		try {
			const status = await api.status(seen.current);
			const run = finishedRun(last.current, status);
			dispatch({ type: "reported", status });
			if (run) done.current(run);
		} catch (error) {
			dispatch({ type: "unanswered", error: (error as Error).message });
		}
	}, []);

	// Pick up a job that was already running when the page opened.
	useEffect(() => {
		void report();
	}, [report]);

	// One timer per report while running; any other state schedules nothing.
	useEffect(() => {
		if (job.state !== "running") return;
		const timer = setTimeout(report, POLL_MS);
		return () => clearTimeout(timer);
	}, [job, report]);

	const start = useCallback(
		async (request: RunRequest) => {
			try {
				await api.start(request);
				dispatch({ type: "started" });
			} catch (error) {
				dispatch({ type: "unanswered", error: (error as Error).message });
				void report(); // "already running" is a refusal too: show what the backend is really doing
			}
		},
		[report],
	);

	/** A job was started some other way (not through `start`): begin showing it. */
	const follow = useCallback(() => {
		dispatch({ type: "started" });
		void report();
	}, [report]);

	return { job, start, follow };
}
