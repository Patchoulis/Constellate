"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { ClosestPaper, ClosestPapers } from "@rp/contracts";
import { ListLabel } from "@/components/community/block";
import { EvidenceLink } from "@/components/reader/evidence-link";
import { api } from "@/lib/net/api";
import { CLOSEST_NOTE, closeness, closestLine, noneCloseWords, quotesTitle, shownPapers } from "@/lib/features/writing/closest";
import { useFollowedJob } from "@/lib/shell/use-followed-job";
import { cn } from "@/lib/utils";

/** How often a community whose closest papers another window is finding is looked at again. */
const LOOK_AGAIN_MS = 1500;

/**
 * A community's closest papers: asked for when it is shown; when they were not found before, finding them is
 * started at once (it is free and sends nothing anywhere), followed as a job, and asked for again when it ends.
 * Nothing waits on it: the rest of the panel is shown meanwhile.
 */
export function useClosest(run: string, treePath: string) {
	const [status, setStatus] = useState<ClosestPapers | null>(null);
	const [unreachable, setUnreachable] = useState<string | null>(null);
	const [version, setVersion] = useState(0);
	const asking = useRef(0);
	const reload = useCallback(() => setVersion((last) => last + 1), []);
	const { job, elsewhere, start, reset } = useFollowedJob(reload, `closest:${run}/${treePath}`, {
		feature: "closest_papers",
		subject: { community: { run, community: treePath } },
	});
	useEffect(() => {
		const question = ++asking.current;
		api.closest(run, treePath).then(
			(next) => {
				if (question !== asking.current) return;
				setStatus(next);
				setUnreachable(null);
			},
			(error: Error) => question === asking.current && setUnreachable(error.message),
		);
	}, [run, treePath, version]);
	// not found yet, and nothing else running: find them now
	const idle = status?.state === "idle" && !status.unavailable;
	useEffect(() => {
		if (!idle || elsewhere !== null || job.state !== "idle") return;
		void start(async () => {
			const started = await api.findClosest(run, treePath);
			if (started.id === null) reload(); // found before, after all
			return started.id;
		});
	}, [idle, elsewhere, job.state, start, run, treePath, reload]);
	// being found by a job this window does not follow: looked at again
	useEffect(() => {
		if (status?.state !== "running" || job.state === "running") return;
		const timer = setTimeout(reload, LOOK_AGAIN_MS);
		return () => clearTimeout(timer);
	}, [status, job.state, reload]);
	// refused because another job was running: once it ends, started again
	const refusedAsBusy = job.state === "failed" && job.id === null;
	useEffect(() => {
		if (elsewhere === null && refusedAsBusy) reset();
	}, [elsewhere, refusedAsBusy, reset]);
	const line = closestLine(
		status,
		{ state: job.state, error: job.state === "failed" ? job.error : undefined },
		elsewhere !== null,
		unreachable,
	);
	return { status, line };
}

/** The quiet line under a description while its closest papers are found, or why they cannot be. */
export function ClosestLine({ line }: { line: ReturnType<typeof useClosest>["line"] }) {
	if (!line) return null;
	return <p className={cn("text-xs", line.tone === "problem" ? "text-destructive" : "text-muted-foreground")}>{line.text}</p>;
}

/**
 * A statement's closest papers: up to three, the rest on "show all"; each the paper's title (it opens the reader at
 * the supporting sentence), its cosine similarity with the statement, and that sentence in the paper's words.
 * Papers in `cited` (the model's own citations of the same section) are not listed again.
 */
export function ClosestList({ papers, floor, cited = new Set() }: { papers: ClosestPaper[]; floor: number; cited?: Set<string> }) {
	const [all, setAll] = useState(false);
	const { shown, more, listed } = shownPapers(papers, cited, all);
	return (
		<div className="min-w-0">
			<ListLabel title={CLOSEST_NOTE}>Closest papers</ListLabel>
			{listed === 0 ? (
				<p className="text-xs text-muted-foreground" title={CLOSEST_NOTE}>
					{papers.length === 0 ? noneCloseWords(floor) : "The closest papers are the ones cited above."}
				</p>
			) : (
				<ul className="flex flex-col gap-1.5" aria-label="Closest papers">
					{shown.map((paper) => (
						<li key={paper.id} className="min-w-0 border-l-2 border-border/60 pl-2 [overflow-wrap:anywhere]">
							<EvidenceLink
								evidence={paper.evidence}
								label={paper.title.trim() || paper.id}
								className="flex items-start leading-snug [&_button]:text-left [&>button:first-child]:min-w-0 [&>button:first-child]:flex-1"
							/>
							<div className="text-[0.6875rem] text-muted-foreground tabular-nums" title={CLOSEST_NOTE}>
								{closeness(paper)}
							</div>
							{!quotesTitle(paper) && (
								<p className="font-sans text-[0.8125rem] leading-snug text-muted-foreground">&ldquo;{paper.evidence.quote}&rdquo;</p>
							)}
						</li>
					))}
				</ul>
			)}
			{(more > 0 || (all && listed > 3)) && (
				<button type="button" className="mt-1 font-ui text-xs text-primary underline-offset-2 hover:underline" onClick={() => setAll(!all)}>
					{all ? "show fewer" : `show all ${listed}`}
				</button>
			)}
		</div>
	);
}
