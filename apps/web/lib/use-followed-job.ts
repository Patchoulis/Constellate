"use client";

/**
 * A job of the engine that a feature started, followed until it ends (engine jobs.rs), as an explicit state
 * machine:
 *
 *     idle --follow--> running --the job ran on--> running
 *                              --it finished-----> done (with what it made)
 *                              --it failed-------> failed (with why)
 *     done | failed --follow--> running
 *
 * While running, `/api/status` is asked every POLL_MS for the log lines after those already seen, and nothing
 * else: a feature never re-reads the whole log. A job replaced by another (it ended between two looks) is done,
 * with nothing known of what it made. The one hook every feature that polls goes through.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type { JobOutcome, JobPapers, JobProgress, Status } from "@rp/contracts";
import { api } from "@/lib/api";

export type FollowedJob =
	| { state: "idle" }
	| {
			state: "running";
			id: number;
			progress: JobProgress | null;
			papers: JobPapers | null;
			log: string[];
			/** the engine did not answer the last look: the job may still be running, and is looked at again */
			unanswered: string | null;
	  }
	| { state: "done"; id: number; outcome: JobOutcome | null; log: string[] }
	| { state: "failed"; id: number | null; error: string; log: string[] };

export const POLL_MS = 1200;

/** A job just started: running, nothing seen of it yet. */
export function started(id: number): FollowedJob {
	return { state: "running", id, progress: null, papers: null, log: [], unanswered: null };
}

/** What `status` says of the running job `job`: its new log lines added, or how it ended. */
export function step(job: Extract<FollowedJob, { state: "running" }>, status: Status): FollowedJob {
	if (status.id !== undefined && status.id !== job.id) return { state: "done", id: job.id, outcome: null, log: job.log };
	const log = [...job.log, ...status.log];
	switch (status.state) {
		case "running":
		case "idle":
			return { ...job, log, progress: status.progress ?? null, papers: status.papers ?? null, unanswered: null };
		case "done":
			return { state: "done", id: job.id, outcome: status.outcome ?? null, log };
		case "failed":
			return { state: "failed", id: job.id, error: status.error ?? "The job failed.", log };
	}
}

/**
 * Follow a job: `follow(id)` once it is started (or `start(begin)`, which starts it and follows the number it
 * answers). `onEnd` is called once when it has finished or failed.
 */
export function useFollowedJob(onEnd?: (job: Extract<FollowedJob, { state: "done" | "failed" }>) => void) {
	const [job, setJob] = useState<FollowedJob>({ state: "idle" });
	const ended = useRef(onEnd);
	ended.current = onEnd;

	const follow = useCallback((id: number) => setJob(started(id)), []);
	const start = useCallback(async (begin: () => Promise<number | null>) => {
		try {
			const id = await begin();
			if (id !== null) setJob(started(id));
		} catch (error) {
			setJob({ state: "failed", id: null, error: (error as Error).message, log: [] });
		}
	}, []);
	const reset = useCallback(() => setJob({ state: "idle" }), []);

	useEffect(() => {
		if (job.state !== "running") return;
		const timer = setTimeout(async () => {
			try {
				// only the lines not seen yet
				const next = step(job, await api.status(job.log.length));
				setJob(next);
				if (next.state === "done" || next.state === "failed") ended.current?.(next);
			} catch (error) {
				setJob({ ...job, unanswered: (error as Error).message });
			}
		}, POLL_MS);
		return () => clearTimeout(timer);
	}, [job]);

	return { job, follow, start, reset };
}
