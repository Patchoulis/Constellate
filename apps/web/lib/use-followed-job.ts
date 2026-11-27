"use client";

/**
 * A job of the engine that a feature started, followed until it ends (engine jobs.rs). Each feature's view of its
 * job is in exactly one of these states, and `jobStep` is the one place it changes:
 *
 *     idle | done | failed --start------> starting      (the user's click; refused while starting or running)
 *     starting --started(id)------------> running
 *     starting --not_started------------> idle          (nothing needed asking: the answer was kept)
 *     starting --refused(why)-----------> failed        (the engine refused: busy, the budget, the model changed)
 *     idle | done | failed --follow(id)-> running       (a job of this window's, found under way again)
 *     running --seen(it ran on)---------> running
 *     running --seen(it finished)-------> done (with what it made and how far it got)
 *     running --seen(it failed)---------> failed (with why and how far it got)
 *     running --seen(another job)-------> done (it ended between two looks: nothing known of what it made)
 *     running --unanswered--------------> running (the engine did not answer; it is looked at again, later)
 *     any but starting --reset----------> idle
 *
 * The engine runs one job at a time and `/api/status` describes it, so one poller (below) asks it for every
 * feature on the page: two sections following the same job see the same answers, nothing is asked while no
 * section follows a running job, and an engine that does not answer is asked less and less often.
 *
 * A job keeps being followed when its section is closed and opened again (another reader tab, a collapsed
 * section, another community and back): the job this window started for a subject is remembered, and a section
 * mounted for that subject follows it at once. A job running that is not this subject's (another section's,
 * another window's) is told as `elsewhere`, so that nothing is offered that the engine would refuse as busy.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type { JobOutcome, JobPapers, JobProgress, Status } from "@rp/contracts";
import { api } from "@/lib/api";
import { withSignal } from "@/lib/request";
import { count } from "@/lib/units";

export type FollowedJob =
	| { state: "idle" }
	| { state: "starting" }
	| {
			state: "running";
			id: number;
			/** what the job is about, as its starter named it (a question's id): kept to show its answer */
			about: string | null;
			progress: JobProgress | null;
			papers: JobPapers | null;
			log: string[];
			/** the engine did not answer the last look: the job may still be running, and is looked at again */
			unanswered: string | null;
	  }
	| { state: "done"; id: number; about: string | null; outcome: JobOutcome | null; progress: JobProgress | null; log: string[] }
	| { state: "failed"; id: number | null; about: string | null; error: string; progress: JobProgress | null; log: string[] };

export type Ended = Extract<FollowedJob, { state: "done" | "failed" }>;

export type JobEvent =
	| { kind: "start" }
	| { kind: "started"; id: number; about?: string | null }
	| { kind: "not_started" }
	| { kind: "refused"; error: string }
	| { kind: "follow"; id: number; about?: string | null }
	/** the engine's job now, with its whole log */
	| { kind: "seen"; status: Status }
	| { kind: "unanswered"; error: string }
	| { kind: "reset" };

export const POLL_MS = 1200;
/** The longest wait between two looks at an engine that does not answer. */
export const MOST_POLL_MS = 20_000;

/** A job just started: running, nothing seen of it yet. */
export function started(id: number, about: string | null = null): FollowedJob {
	return { state: "running", id, about, progress: null, papers: null, log: [], unanswered: null };
}

/** What `status` (with the job's whole log) says of the running job `job`: how far it is, or how it ended. */
export function step(job: Extract<FollowedJob, { state: "running" }>, status: Status): FollowedJob {
	const { id, about, progress } = job;
	if (status.id !== undefined && status.id !== id) return { state: "done", id, about, outcome: null, progress, log: job.log };
	const log = status.log;
	const reached = status.progress ?? progress;
	switch (status.state) {
		case "running":
		case "idle":
			return { ...job, log, progress: status.progress ?? null, papers: status.papers ?? null, unanswered: null };
		case "done":
			return { state: "done", id, about, outcome: status.outcome ?? null, progress: reached, log };
		case "failed":
			return { state: "failed", id, about, error: status.error ?? "The job failed.", progress: reached, log };
	}
}

/** The one place a feature's job changes; an event that does not apply to the state leaves it as it is. */
export function jobStep(job: FollowedJob, event: JobEvent): FollowedJob {
	switch (event.kind) {
		case "start":
			return job.state === "starting" || job.state === "running" ? job : { state: "starting" };
		case "started":
			return job.state === "starting" ? started(event.id, event.about ?? null) : job;
		case "not_started":
			return job.state === "starting" ? { state: "idle" } : job;
		case "refused":
			return job.state === "starting" ? { state: "failed", id: null, about: null, error: event.error, progress: null, log: [] } : job;
		case "follow":
			if (job.state === "starting" || (job.state === "running" && job.id === event.id)) return job;
			return started(event.id, event.about ?? null);
		case "seen":
			return job.state === "running" ? step(job, event.status) : job;
		case "unanswered":
			return job.state === "running" ? { ...job, unanswered: event.error } : job;
		case "reset":
			return job.state === "starting" ? job : { state: "idle" };
	}
}

/** "6 of 10 papers were asked; the other 4 were not." for a job that ended before its end, or null. */
export function notAskedLine(job: FollowedJob, noun: string, nouns = `${noun}s`): string | null {
	if ((job.state !== "done" && job.state !== "failed") || !job.progress) return null;
	const { done, of } = job.progress;
	if (done >= of) return null;
	return `${done.toLocaleString()} of ${count(of, noun, nouns)} ${done === 1 ? "was" : "were"} asked; the other ${(of - done).toLocaleString()} ${of - done === 1 ? "was" : "were"} not.`;
}

// ---- the one poller of /api/status ----

/** A look at the engine's job: what it is now (its log whole), or why the engine did not answer. */
export type Look = { seen: Status } | { unanswered: string };

/** How long to wait before the next look, after `failures` looks in a row the engine did not answer. */
export function pollDelay(failures: number): number {
	return Math.min(POLL_MS * 2 ** Math.min(failures, 16), MOST_POLL_MS);
}

export interface Poller {
	/** be told every look while `watch`ing; the engine is looked at only while someone watches */
	watch: (listener: (look: Look) => void) => () => void;
	/** one look now, or the last one when it is newer than a poll's interval */
	probe: () => Promise<Look>;
	/**
	 * A job was just started: what was seen before is another job's. The log is asked for from its first line, and
	 * a look already under way (asked before the start) is asked again rather than told, so that nobody takes the
	 * last job's end for the new one's.
	 */
	restart: () => void;
}

/** A poller asking `status(since)`; `now` and `wait` are the clock (the tests' own). */
export function createPoller(
	status: (since: number) => Promise<Status>,
	now: () => number = Date.now,
	wait: (run: () => void, ms: number) => () => void = (run, ms) => {
		const timer = setTimeout(run, ms);
		return () => clearTimeout(timer);
	},
): Poller {
	const listeners = new Set<(look: Look) => void>();
	let known: { id: number | undefined; log: string[] } = { id: undefined, log: [] };
	let last: { at: number; look: Look } | null = null;
	let asking: Promise<Look> | null = null;
	let cancel: (() => void) | null = null;
	let failures = 0;
	/** bumped by `restart`: a look asked under an older one is not told */
	let generation = 0;

	const ask = async (asked: number): Promise<Look> => {
		try {
			const before = known;
			let told = await status(before.log.length);
			// another job since the last look: its log from its first line
			if (told.id !== before.id && before.log.length > 0) told = await status(0);
			const log = told.id === before.id ? [...before.log, ...told.log] : told.log;
			if (asked === generation) known = { id: told.id, log }; // else a restart came: this look is not kept
			failures = 0;
			return { seen: { ...told, log } };
		} catch (error) {
			failures += 1;
			return { unanswered: (error as Error).message };
		}
	};
	const schedule = () => {
		cancel?.();
		cancel = null;
		if (listeners.size > 0 && asking === null) cancel = wait(() => void look(), pollDelay(failures));
	};
	const look = (): Promise<Look> => {
		const asked = generation;
		asking ??= ask(asked).then((answer) => {
			asking = null;
			if (asked !== generation) return look(); // asked before a job was started: asked again
			last = { at: now(), look: answer };
			for (const listener of [...listeners]) listener(answer);
			schedule();
			return answer;
		});
		return asking;
	};
	return {
		watch(listener) {
			listeners.add(listener);
			if (cancel === null && asking === null) schedule();
			return () => {
				listeners.delete(listener);
				if (listeners.size === 0) {
					cancel?.();
					cancel = null;
				}
			};
		},
		probe() {
			if (last && "seen" in last.look && now() - last.at < POLL_MS) return Promise.resolve(last.look);
			return look();
		},
		restart() {
			generation += 1;
			known = { id: undefined, log: [] };
			last = null;
		},
	};
}

/** The one poller of `/api/status` of this window: every feature's section and the run's job (lib/use-job.ts)
 *  look through it. Shared by all of them: never called off by one. */
export const poller = createPoller((since) => withSignal(null, () => api.status(since)));

/** The jobs this window started, by what they are about (a paper's weaknesses, a community's table…). */
const startedFor = new Map<string, { id: number; about: string | null }>();
/** What starting a job answers: its number (and what it is about), or null when nothing needed starting. */
export type Begun = number | { id: number; about: string | null } | null;

/**
 * Follow the job of `subject` (a key of what it is about: "critique:<paper>"): `start(begin)` on the user's click
 * starts it once (a second click while it starts or runs does nothing) and follows the number it answers;
 * `follow(id)` follows one started otherwise. `onEnd` is called once when it has finished or failed.
 * `elsewhere` is a job running that is not this one: nothing should be started until it ends.
 */
export function useFollowedJob(onEnd?: (job: Ended) => void, subject?: string) {
	const [job, setJob] = useState<FollowedJob>(() => {
		const kept = subject === undefined ? undefined : startedFor.get(subject);
		return kept ? started(kept.id, kept.about) : { state: "idle" };
	});
	const [elsewhere, setElsewhere] = useState<{ id: number; kind: string } | null>(null);
	const current = useRef(job);
	const ended = useRef(onEnd);
	ended.current = onEnd;

	const transition = useCallback(
		(event: JobEvent): boolean => {
			const before = current.current;
			const next = jobStep(before, event);
			if (next === before) return false;
			current.current = next;
			setJob(next);
			if (subject !== undefined) {
				if (next.state === "running") startedFor.set(subject, { id: next.id, about: next.about });
				else if (before.state === "running") startedFor.delete(subject);
			}
			if (before.state === "running" && (next.state === "done" || next.state === "failed")) ended.current?.(next);
			return true;
		},
		[subject],
	);

	/** What a look says of a job running that is not ours. */
	const notice = useCallback((look: Look) => {
		if (!("seen" in look)) return;
		const status = look.seen;
		const ours = current.current.state === "running" && current.current.id === status.id;
		const other = status.state === "running" && status.id !== undefined && !ours;
		setElsewhere((before) => (other ? (before?.id === status.id ? before : { id: status.id!, kind: status.kind ?? "another job" }) : null));
	}, []);

	const listen = useCallback(
		(look: Look) => {
			if ("seen" in look) transition({ kind: "seen", status: look.seen });
			else transition({ kind: "unanswered", error: look.unanswered });
			notice(look);
		},
		[transition, notice],
	);

	// what is running when this is shown: another section's or window's job is said, and not started over
	useEffect(() => {
		let gone = false;
		void poller.probe().then((look) => gone || notice(look));
		return () => {
			gone = true;
		};
	}, [notice]);

	// looked at while our job runs, or another one does; not at all otherwise, nor once this is gone
	const watching = job.state === "running" || elsewhere !== null;
	useEffect(() => (watching ? poller.watch(listen) : undefined), [watching, listen]);

	const follow = useCallback((id: number, about: string | null = null) => void transition({ kind: "follow", id, about }), [transition]);
	const start = useCallback(
		async (begin: () => Promise<Begun>) => {
			if (!transition({ kind: "start" })) return;
			try {
				const begun = await begin();
				// a look asked before the job began would tell the last job's end as this one's
				if (begun !== null) poller.restart();
				if (begun === null) transition({ kind: "not_started" });
				else if (typeof begun === "number") transition({ kind: "started", id: begun });
				else transition({ kind: "started", id: begun.id, about: begun.about });
			} catch (error) {
				transition({ kind: "refused", error: (error as Error).message });
				void poller.probe().then(notice); // refused as busy: say what is running
			}
		},
		[transition, notice],
	);
	const reset = useCallback(() => void transition({ kind: "reset" }), [transition]);

	return { job, elsewhere, follow, start, reset };
}

/** "Another job is in progress (reading citation kinds): wait for it to end." */
export function elsewhereLine(elsewhere: { kind: string }): string {
	return `Something else is being done (${elsewhere.kind}): nothing new can be asked until it ends.`;
}
