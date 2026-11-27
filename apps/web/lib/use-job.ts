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
 */
export interface Job {
	state: JobState;
	log: string[];
	error: string | null;
}

type Event =
	| { type: "started" }
	| { type: "reported"; status: Status }
	// the backend did not answer, or refused to start a run: what the job is doing is unchanged
	| { type: "unanswered"; error: string };

function reduce(job: Job, event: Event): Job {
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
	const done = useRef(onDone);
	done.current = onDone;

	const report = useCallback(async () => {
		try {
			const status = await api.status(seen.current);
			dispatch({ type: "reported", status });
			if (status.state === "done" && status.result) done.current(status.result);
		} catch (problem) {
			dispatch({ type: "unanswered", error: (problem as Error).message });
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

	const start = useCallback(async (request: RunRequest) => {
		try {
			await api.start(request);
			dispatch({ type: "started" });
		} catch (problem) {
			dispatch({ type: "unanswered", error: (problem as Error).message });
			void report(); // "already running" is a refusal too: show what the backend is really doing
		}
	}, [report]);

	/** A job was started some other way (not through `start`): begin showing it. */
	const follow = useCallback(() => {
		dispatch({ type: "started" });
		void report();
	}, [report]);

	return { job, start, follow };
}
