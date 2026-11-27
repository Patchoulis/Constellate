"use client";

import { useCallback, useEffect, useState } from "react";
import { Loader2, Sparkles, X } from "lucide-react";
import type { OverviewPoint, OverviewStatus } from "@rp/contracts";
import { CommunityDot } from "@/components/community-panel";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { api } from "@/lib/api";
import { communityLabel, type CommunityNode } from "@/lib/community";

const POLL_MS = 3000;
const CITED_AT_FIRST = 4;

/**
 * Where a community's overview stands, mirroring the backend's job states:
 *
 *     idle --write--> running --> done | failed --write--> running
 *
 * It is asked for once when shown, and again every few seconds while (and only while) it is being written.
 */
function useOverview(run: string, treePath: string) {
	const [status, setStatus] = useState<OverviewStatus | null>(null);
	const [unreachable, setUnreachable] = useState<string | null>(null);
	const ask = useCallback(
		(write = false) =>
			api.overview(run, treePath, write).then(
				(next) => {
					setStatus(next);
					setUnreachable(null);
				},
				(error: Error) => {
					setUnreachable(error.message);
					// a new object, so the effect below runs again: one failed poll must not end the polling
					setStatus((last) => last && { ...last });
				},
			),
		[run, treePath],
	);
	useEffect(() => {
		setStatus(null);
		void ask();
	}, [ask]);
	useEffect(() => {
		if (status?.state !== "running") return;
		const timer = setTimeout(ask, POLL_MS);
		return () => clearTimeout(timer);
	}, [status, ask]);
	return { status, error: unreachable ?? status?.error ?? null, write: () => ask(true) };
}

/** One claim about the community, and the papers that share it: the citations are the evidence. */
function Point({ point, onOpenPaper }: { point: OverviewPoint; onOpenPaper: (id: string) => void }) {
	const [all, setAll] = useState(false);
	const shown = all ? point.papers : point.papers.slice(0, CITED_AT_FIRST);
	return (
		<li className="flex flex-col gap-1.5">
			<h3 className="text-lg leading-snug font-semibold text-foreground">{point.title}</h3>
			<p className="text-base text-foreground/90">{point.insight}</p>
			<div className="font-ui text-xs text-muted-foreground">Shared by {point.papers.length} papers</div>
			<div className="flex flex-col border-l pl-1">
				{shown.map((paper) => (
					<RowButton
						key={paper.id}
						className="py-1 text-sm text-muted-foreground hover:text-foreground"
						onClick={() => onOpenPaper(paper.id)}
					>
						<span>
							<MathText>{paper.title}</MathText>
						</span>
					</RowButton>
				))}
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
	onOpenPaper,
	onClose,
}: {
	run: string;
	node: CommunityNode;
	onOpenPaper: (id: string) => void;
	onClose: () => void;
}) {
	const { community } = node;
	const { status, error, write } = useOverview(run, community.tree_path);
	const overview = status?.state === "done" ? status.overview : null;
	return (
		<div className="absolute inset-y-0 right-0 z-20 flex w-[min(52rem,100%)] flex-col border-l bg-card shadow-2xl">
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
							<p className="text-lg leading-relaxed text-foreground">{overview.summary}</p>
							<p className="font-ui text-xs text-muted-foreground">
								Written by {overview.model} from all {overview.papers} papers. Every point cites the papers that share it; click one to read
								it.
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
							An overview reads all {community.size} papers of this community and reports what they have in common: the methods that recur,
							the findings several papers agree on, the directions gaining momentum, and where they disagree. Each point lists the papers
							behind it.
						</p>
						{status?.state === "running" ? (
							<p className="flex items-center gap-2 text-sm text-muted-foreground">
								<Loader2 className="size-3.5 animate-spin" />
								Reading {community.size} papers. This takes a minute or two.
							</p>
						) : (
							<Button disabled={status === null && !error} onClick={write}>
								<Sparkles />
								Write the overview
							</Button>
						)}
						{error && <p className="text-sm text-destructive">{error}</p>}
						<p className="text-xs text-muted-foreground">
							It is written with the Claude model chosen in Settings and kept, so it is only written once.
						</p>
					</div>
				)}
			</div>
		</div>
	);
}
