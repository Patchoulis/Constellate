"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { Catalog, Ratings, ReaderTarget } from "@rp/contracts";
import { RunAsk } from "@/components/ask";
import { CommunityPanel } from "@/components/community-panel";
import { EvidenceProvider, type EvidenceActions } from "@/components/evidence-link";
import { GraphView } from "@/components/graph-view";
import { OverviewPanel } from "@/components/overview";
import { PaperReader } from "@/components/paper-reader";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/api";
import { communityTree, type CommunityNode } from "@/lib/community";
import { useMarks } from "@/lib/marks";
import type { Job } from "@/lib/use-job";
import { ready, useAsync } from "@/lib/use-async";
import { useResizable } from "@/lib/use-resizable";
import { stepLine } from "@/lib/run-step";

/** The run's log, folded to its last line until opened. */
export function ActivityLog({ job }: { job: Job }) {
	const [open, setOpen] = useState(false);
	const { ref: box, style, handle } = useResizable<HTMLPreElement>("log"); // the height of its text, when open
	const running = job.state === "running";
	useEffect(() => setOpen(running), [running]);
	useEffect(() => {
		if (box.current) box.current.scrollTop = box.current.scrollHeight;
	}, [box, job.log, open]);
	if (job.log.length === 0) return null;
	return (
		<div className="enter-from-bottom absolute bottom-3 left-3 z-10 w-[min(34rem,calc(100%-4.5rem))] rounded-md border bg-card/95 shadow-lg">
			{open && <ResizeHandle {...handle} />}
			<button
				type="button"
				aria-expanded={open}
				title={open ? "Fold the log" : "Show the whole log"}
				onClick={() => setOpen((value) => !value)}
				className="flex h-8 w-full items-center gap-2 px-3 text-left text-xs"
			>
				<span className={running ? "size-1.5 animate-pulse rounded-full bg-success" : "size-1.5 rounded-full bg-muted-foreground"} />
				{/* while a run is made, its step and how far it has got (the model's download in MB); else the last line */}
				<span className="flex-1 truncate text-muted-foreground">
					{running && job.step ? stepLine(job.step) : job.log[job.log.length - 1].trim()}
				</span>
				{open ? <ChevronDown className="size-3.5" /> : <ChevronUp className="size-3.5" />}
			</button>
			{open && (
				<pre
					ref={box}
					style={style}
					className="h-(--panel-size) overflow-auto border-t px-3 py-2 text-xs whitespace-pre-wrap text-muted-foreground"
				>
					{job.log.join("\n")}
				</pre>
			)}
		</div>
	);
}

export function Notice({ children }: { children: React.ReactNode }) {
	return <div className="flex size-full items-center justify-center p-8 text-center text-sm text-muted-foreground">{children}</div>;
}

/** One run on screen: its graph, its communities, and the paper being read. Mount it with key={name}, so
 *  opening another run starts with nothing selected. `children` are laid over the graph. */
export function Workspace({
	name,
	catalog,
	model,
	children,
}: {
	name: string;
	catalog: Catalog | undefined;
	/** the language model chosen in Settings, once known */
	model?: string;
	children?: React.ReactNode;
}) {
	const run = useAsync(() => Promise.all([api.run(name), api.graph(name)]), [name], { keep: false });
	const [selected, setSelected] = useState<CommunityNode | null>(null);
	const [paper, setPaper] = useState<string | null>(null);
	const [overview, setOverview] = useState<CommunityNode | null>(null); // the community whose overview is open
	const [result, graph] = ready(run) ?? [];
	const roots = useMemo(() => (result ? communityTree(result.communities) : []), [result]);
	// reviewers' scores: those read before, and those read from a Reviews tab opened since
	const kept = ready(useAsync(() => api.ratings(), [name], { keep: false }));
	const [fresh, setFresh] = useState<Record<string, Ratings>>({});
	const ratings = useMemo(() => ({ ...kept, ...fresh }), [kept, fresh]);
	const onRatings = useCallback((id: string, read: Ratings) => setFresh((before) => ({ ...before, [id]: read })), []);
	// the papers marked in this run; none are shown until the graph says which papers the run has
	const known = useMemo(() => (graph ? new Set(graph.nodes.map((node) => node.id)) : null), [graph]);
	const marks = useMarks(name, known);
	const [goTo, setGoTo] = useState<{ id: string } | null>(null);
	// where in the open paper a citation asked the reader to land; any other way of opening a paper clears it
	const [place, setPlace] = useState<ReaderTarget | null>(null);
	const openPaper = useCallback((id: string | null) => {
		setPlace(null);
		setPaper(id);
	}, []);
	const goToPaper = useCallback(
		(id: string) => {
			openPaper(id);
			setGoTo({ id });
		},
		[openPaper],
	);
	const evidence = useMemo<EvidenceActions>(
		() => ({
			open: (target) => {
				setPlace(target);
				setPaper(target.paper);
			},
			inRun: (id) => known?.has(id) ?? false,
			locate: goToPaper,
		}),
		[known, goToPaper],
	);
	const markedPapers = useMemo(() => {
		const byId = new Map(graph?.nodes.map((node) => [node.id, node]));
		return marks.marked.flatMap((id) => {
			const node = byId.get(id);
			return node ? [{ node, community: roots[node.community] }] : [];
		});
	}, [graph, roots, marks.marked]);

	if (!result || !graph) {
		return (
			<main className="relative min-w-0 flex-1">
				<Notice>{run.status === "failed" ? run.error : "Loading the run…"}</Notice>
				{children}
			</main>
		);
	}
	if (graph.nodes.length === 0) {
		// nothing to draw, lay out or pick from
		return (
			<main className="relative min-w-0 flex-1">
				<Notice>This run has no papers to show. Pick another range or conference, or change the settings, and run it again.</Notice>
				{children}
			</main>
		);
	}
	const paperIndex = paper === null ? -1 : graph.nodes.findIndex((node) => node.id === paper);
	const paperNode = graph.nodes[paperIndex];
	// The papers the open one is linked to, closest first.
	const related = graph.edges
		.filter(([a, b]) => a === paperIndex || b === paperIndex)
		.map(([a, b, similarity]) => ({ ...graph.nodes[a === paperIndex ? b : a], similarity }))
		.sort((a, b) => b.similarity - a.similarity);
	return (
		<EvidenceProvider value={evidence}>
			<main className="relative min-w-0 flex-1">
				<GraphView
					graph={graph}
					roots={roots}
					focus={selected}
					selectedPaper={paper}
					ratings={ratings}
					marked={marks.marked}
					goTo={goTo}
					onPickPaper={openPaper}
					onMarkPaper={marks.toggle}
				/>
				{children}
				<RunAsk run={name} papers={graph.nodes.length} />
				{overview && (
					<OverviewPanel
						key={overview.community.tree_path}
						run={name}
						node={overview}
						model={model}
						onOpenPaper={openPaper}
						onClose={() => setOverview(null)}
					/>
				)}
				{paper && (
					<PaperReader
						key={paper}
						id={paper}
						community={paperNode && roots[paperNode.community]}
						related={related.map((other) => ({ ...other, community: roots[other.community] }))}
						catalog={catalog}
						marked={marks.has(paper)}
						onToggleMark={() => marks.toggle(paper)}
						run={name}
						at={place}
						onRatings={onRatings}
						onOpen={openPaper}
						onClose={() => openPaper(null)}
					/>
				)}
			</main>
			<CommunityPanel
				result={result}
				roots={roots}
				selected={selected}
				onSelect={setSelected}
				onOverview={setOverview}
				onOpenPaper={openPaper}
				marks={marks}
				markedPapers={markedPapers}
				onGoToPaper={goToPaper}
			/>
		</EvidenceProvider>
	);
}
