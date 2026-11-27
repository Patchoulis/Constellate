"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, PenLine, X } from "lucide-react";
import type { OverviewPoint, OverviewRequest, OverviewStatus } from "@rp/contracts";
import { CommunityDot } from "@/components/community/community-panel";
import { EvidenceLink } from "@/components/reader/evidence-link";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/api";
import { communityLabel, type CommunityNode } from "@/lib/community/community";
import { accepting, costTitle, costWords, shownModel } from "@/lib/cost";
import { useFocusReturn } from "@/lib/use-focus-return";
import { useResizable } from "@/lib/use-resizable";

const POLL_MS = 3000;
const CITED_AT_FIRST = 4;

/**
 * Where a community's overview stands, mirroring the engine's job states:
 *
 *     idle --write--> running --> done | failed --write--> running
 *
 * It is asked for once when shown, again whenever the language model chosen in Settings changes (the overview
 * kept is the model's: another model's is not this one's), and every few seconds while (and only while) it is
 * being written. An answer to a question asked before the model changed is dropped.
 */
export function useOverview(run: string, treePath: string, model: string | undefined) {
	const [status, setStatus] = useState<OverviewStatus | null>(null);
	const [unreachable, setUnreachable] = useState<string | null>(null);
	const asking = useRef(0); // which question is the latest: only its answer is shown
	const ask = useCallback(
		(write?: OverviewRequest | true) => {
			const question = ++asking.current;
			return api.overview(run, treePath, write).then(
				(next) => {
					if (question !== asking.current) return;
					setStatus(next);
					setUnreachable(null);
				},
				(error: Error) => {
					if (question !== asking.current) return;
					setUnreachable(error.message);
					// a new object, so the effect below runs again: one failed poll must not end the polling
					setStatus((last) => last && { ...last });
				},
			);
		},
		// the model is not sent: the engine writes with the one in its settings, which is this one
		// eslint-disable-next-line react-hooks/exhaustive-deps
		[run, treePath, model],
	);
	useEffect(() => {
		setStatus(null);
		setUnreachable(null);
		void ask();
	}, [ask]);
	useEffect(() => {
		if (status?.state !== "running") return;
		const timer = setTimeout(ask, POLL_MS);
		return () => clearTimeout(timer);
	}, [status, ask]);
	// the engine writes only within the most of the estimate it showed
	const estimate = status?.estimate ?? null;
	const write = () => ask(estimate ? { ...shownModel(estimate), ...accepting(estimate) } : true);
	return { status, error: unreachable ?? status?.error ?? null, write };
}

/** One claim about the community, and the papers that share it: the citations are the evidence. */
function Point({ point, onOpenPaper }: { point: OverviewPoint; onOpenPaper: (id: string) => void }) {
	const [all, setAll] = useState(false);
	const shown = all ? point.papers : point.papers.slice(0, CITED_AT_FIRST);
	// the quote of each paper, as the paper words it (the engine only)
	const quoteOf = (id: string) => point.evidence?.find((piece) => piece.paper === id);
	return (
		<li className="flex flex-col gap-1.5">
			<h3 className="text-lg leading-snug font-semibold text-foreground">{point.title}</h3>
			<p className="text-base text-foreground/90">{point.insight}</p>
			<div className="font-ui text-xs text-muted-foreground">Shared by {point.papers.length} papers</div>
			<div className="flex flex-col border-l pl-1">
				{shown.map((paper) => {
					const quote = quoteOf(paper.id);
					return (
						<div key={paper.id} className="flex flex-col">
							<RowButton className="py-1 text-sm text-muted-foreground hover:text-foreground" onClick={() => onOpenPaper(paper.id)}>
								<span>
									<MathText>{paper.title}</MathText>
								</span>
							</RowButton>
							{quote && (
								<p className="pb-1 pl-2 text-xs text-muted-foreground">
									<span className="italic">“{quote.quote}”</span> <EvidenceLink evidence={quote} />
								</p>
							)}
						</div>
					);
				})}
				{point.papers.length > CITED_AT_FIRST && (
					<RowButton className="py-1 font-ui text-xs text-muted-foreground" onClick={() => setAll(!all)}>
						{all ? "Show fewer" : `Show all ${point.papers.length}`}
					</RowButton>
				)}
			</div>
		</li>
	);
}

/** A community's overview, in a panel over the graph. */
export function OverviewPanel({
	run,
	node,
	model,
	onOpenPaper,
	onClose,
}: {
	run: string;
	node: CommunityNode;
	/** the language model chosen in Settings, once known: the overview shown is the one it wrote */
	model: string | undefined;
	onOpenPaper: (id: string) => void;
	onClose: () => void;
}) {
	const { community } = node;
	const { status, error, write } = useOverview(run, community.tree_path, model);
	const overview = status?.state === "done" ? status.overview : null;
	const { ref, style, handle } = useResizable<HTMLDivElement>("overview");
	useFocusReturn(ref);
	return (
		<div
			ref={ref}
			style={style}
			className="enter-from-right absolute inset-y-0 right-0 z-20 flex w-[min(var(--panel-size),100%)] flex-col border-l bg-card shadow-2xl"
		>
			<ResizeHandle {...handle} />
			<div className="flex h-11 shrink-0 items-center gap-2 border-b px-4">
				<CommunityDot hue={node.hue} />
				<span className="min-w-0 flex-1 truncate text-sm font-semibold">Overview · {communityLabel(community)}</span>
				<Button variant="ghost" size="icon" title="Close" onClick={onClose}>
					<X />
				</Button>
			</div>
			<div className="min-h-0 flex-1 overflow-y-auto px-8 py-6">
				{overview ? (
					<article className="mx-auto flex max-w-[68ch] flex-col gap-6 font-sans">
						<header className="flex flex-col gap-3">
							<h1 className="text-2xl leading-tight font-semibold text-balance">{overview.title}</h1>
							{overview.summary && <p className="text-lg leading-relaxed text-foreground">{overview.summary}</p>}
							<p className="font-ui text-xs text-muted-foreground">
								{overview.papers_given === undefined || overview.papers_given === overview.papers
									? `Written by ${overview.model} from all ${overview.papers} papers.`
									: `Written by ${overview.model} from the ${overview.papers_given} most central of the ${overview.papers} papers.`}{" "}
								{overview.counts
									? "Each point is the model's reading; it is shown only with quotes found in at least 3 of the papers it names, in the papers' own words."
									: "Every point cites the papers that share it; click one to read it."}
								{overview.counts && overview.counts.not_found + overview.counts.malformed > 0 && (
									<>
										{" "}
										{overview.counts.not_found + overview.counts.malformed} of the {overview.counts.written} points written were left out:
										their quotes were not found{overview.counts.summary_left_out ? ", nor were the summary's" : ""}.
									</>
								)}
							</p>
						</header>
						{overview.sections.map((section) => (
							<section key={section.heading} className="flex flex-col gap-4">
								<Heading className="border-b pb-1.5 font-ui">{section.heading}</Heading>
								<ol className="flex flex-col gap-6">
									{section.points.map((point) => (
										<Point key={point.title} point={point} onOpenPaper={onOpenPaper} />
									))}
								</ol>
							</section>
						))}
					</article>
				) : (
					<div className="mx-auto flex max-w-[52ch] flex-col items-start gap-4 pt-10">
						<p className="font-sans text-lg text-foreground">
							An overview reads the {community.size} papers of this community (the most central, when there are very many) and reports what
							they have in common: the methods that recur, the findings several papers agree on, the directions gaining momentum, and where
							they disagree. Each point lists the papers behind it.
						</p>
						{status?.state === "running" ? (
							<p className="flex items-center gap-2 text-sm text-muted-foreground">
								<Loader2 className="size-3.5 animate-spin" />
								Reading {community.size} papers. This takes a minute or two.
							</p>
						) : (
							<div className="flex flex-col items-start gap-1">
								<Button disabled={(status === null && !error) || status?.api_key_set === false} onClick={write}>
									<PenLine />
									Write the overview
								</Button>
								{status?.estimate && (
									<p className="text-xs text-muted-foreground" title={costTitle(status.estimate)}>
										With {status.estimate.model}: {costWords(status.estimate)} (an estimate).
										{status.api_key_set === false && " No key is set for its provider."}
									</p>
								)}
							</div>
						)}
						{error && <p className="text-sm text-destructive">{error}</p>}
						<p className="text-xs text-muted-foreground">
							It is written with the language model chosen in Settings and kept, so it is only written once.
						</p>
					</div>
				)}
			</div>
		</div>
	);
}
