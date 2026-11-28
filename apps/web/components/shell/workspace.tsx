"use client";

import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { Catalog, Ratings, ReaderTarget } from "@rp/contracts";
import { RunAsk } from "@/components/features/answers/ask";
import { CommunityPanel } from "@/components/community/community-panel";
import { EvidenceProvider, type EvidenceActions } from "@/components/reader/evidence-link";
import { GraphView } from "@/components/graph/graph-view";
import { OverviewPanel } from "@/components/community/overview";
import { PaperReader } from "@/components/reader/paper-reader";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/net/api";
import { communityTree, nodeAt, type CommunityNode } from "@/lib/community/community";
import { withCounted } from "@/lib/reader/citations";
import { linkedPapers } from "@/lib/graph/related";
import { ANSWER_OWNER, GraphColouringContext, colouringStep, type GraphColouring, type PaperColouring } from "@/lib/graph/graph-colouring";
import { useMarks } from "@/lib/community/marks";
import type { Job } from "@/lib/shell/use-job";
import { ready, useAsync } from "@/lib/use-async";
import { setPrefs, usePrefs } from "@/lib/ui/prefs";
import { useResizable } from "@/lib/ui/use-resizable";
import { stepLine } from "@/lib/shell/run-step";

/** The run's log, folded to its last line until opened. */
export function ActivityLog({ job }: { job: Job }) {
	const [open, setOpen] = useState(false);
	const { ref: box, style, handle } = useResizable<HTMLPreElement>("log"); // the height of its text, when open
	const wide = useResizable<HTMLDivElement>("logWidth"); // its width, open or folded
	const running = job.state === "running";
	useEffect(() => setOpen(running), [running]);
	useEffect(() => {
		if (box.current) box.current.scrollTop = box.current.scrollHeight;
	}, [box, job.log, open]);
	if (job.log.length === 0) return null;
	return (
		<div
			ref={wide.ref}
			style={wide.style}
			className="enter-from-bottom absolute bottom-3 left-3 z-10 w-[min(var(--panel-size),calc(100%-4.5rem))] rounded-md border bg-card/95 shadow-lg"
		>
			{open && <ResizeHandle {...handle} />}
			<ResizeHandle {...wide.handle} />
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
	title,
	catalog,
	model,
	children,
}: {
	name: string;
	/** the run's title, as the list shows it */
	title?: string;
	catalog: Catalog | undefined;
	/** the language model chosen in Settings, once known */
	model?: string;
	children?: React.ReactNode;
}) {
	const run = useAsync(() => Promise.all([api.run(name), api.graph(name)]), [name], { keep: false });
	// Which community is selected and whose overview is open are kept by their place in the tree, not as the tree's
	// own objects: the tree is made again whenever the run's figures come (citations counted, communities named), and
	// an object of the old tree handed to the graph for even one render is not in the new layout, which stopped the
	// whole page. The community is found in the tree as it is now, every render.
	const [selectedPath, setSelectedPath] = useState<string | null>(null);
	const [paper, setPaper] = useState<string | null>(null);
	const [overviewPath, setOverviewPath] = useState<string | null>(null); // the community whose overview is open
	const [saved, graph] = ready(run) ?? [];
	// the citation figures counted now, from every reference list of the library read so far, in place of any
	// saved with the run; until they come (or if they cannot be counted), the saved ones say "not counted yet"
	const counted = ready(useAsync(() => api.runCitations(name), [name], { keep: false }));
	const result = useMemo(() => saved && { ...saved, communities: withCounted(saved.communities, counted) }, [saved, counted]);
	const roots = useMemo(() => (result ? communityTree(result.communities) : []), [result]);
	const selected = useMemo(() => (selectedPath === null ? null : nodeAt(roots, selectedPath)), [roots, selectedPath]);
	const overview = useMemo(() => (overviewPath === null ? null : nodeAt(roots, overviewPath)), [roots, overviewPath]);
	const setSelected = useCallback((node: CommunityNode | null) => setSelectedPath(node ? node.community.tree_path : null), []);
	const setOverview = useCallback((node: CommunityNode | null) => setOverviewPath(node ? node.community.tree_path : null), []);
	// reviewers' scores: those read before, and those read from a Reviews tab opened since
	const kept = ready(useAsync(() => api.ratings(), [name], { keep: false }));
	const [fresh, setFresh] = useState<Record<string, Ratings>>({});
	const ratings = useMemo(() => ({ ...kept, ...fresh }), [kept, fresh]);
	const onRatings = useCallback((id: string, read: Ratings) => setFresh((before) => ({ ...before, [id]: read })), []);
	// the papers marked in this run; none are shown until the graph says which papers the run has
	const known = useMemo(() => (graph ? new Set(graph.nodes.map((node) => node.id)) : null), [graph]);
	const stored = useMarks(name, known);
	const prefs = usePrefs();
	// Marking while the marks are hidden shows them again, where the person chose that (Settings).
	const { showMarksOnMark } = prefs;
	const marks = useMemo(() => {
		const show = () => showMarksOnMark && setPrefs({ marksShown: true });
		return {
			...stored,
			toggle: (id: string) => {
				if (!stored.has(id)) show();
				stored.toggle(id);
			},
			addMany: (ids: readonly string[]) => {
				show();
				stored.addMany(ids);
			},
		};
	}, [stored, showMarksOnMark]);
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
	// the graph coloured by a feature's groups of papers (lib/graph/graph-colouring.ts): none for a run just opened
	const [colouring, colour] = useReducer(colouringStep, null);
	// the same functions for as long as the run is open: a feature may clear its colouring when it goes away
	const setColouring = useCallback((one: PaperColouring) => colour({ kind: "set", colouring: one }), []);
	const clearColouring = useCallback((owner: string) => colour({ kind: "clear", owner }), []);
	const colourings = useMemo<GraphColouring>(
		() => ({ colouring, set: setColouring, clear: clearColouring }),
		[colouring, setColouring, clearColouring],
	);
	const onCitedPapers = useCallback(
		(papers: string[]) =>
			colour({
				kind: "offer", // never in place of a colouring the user chose
				colouring: { owner: ANSWER_OWNER, title: "The papers the answer cites", groups: [{ name: "cited", hue: 48, papers }] },
			}),
		[],
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
	// The open paper may not be in the run as it is now (read again since it was opened): it is still read, with no
	// community and no linked papers.
	const paperNode = paper === null ? undefined : graph.nodes.find((node) => node.id === paper);
	// The papers the open one is linked to, closest first.
	const related = linkedPapers(graph, paper);
	return (
		<EvidenceProvider value={evidence}>
			<GraphColouringContext value={colourings}>
				<main className="relative min-w-0 flex-1">
					<GraphView
						graph={graph}
						roots={roots}
						focus={selected}
						selectedPaper={paper}
						ratings={ratings}
						marked={marks.marked}
						marksShown={prefs.marksShown}
						selectKey={prefs.selectKey}
						goTo={goTo}
						onPickPaper={openPaper}
						onMarkPaper={marks.toggle}
						onMarkPapers={marks.addMany}
					/>
					{children}
					<RunAsk run={name} papers={graph.nodes.length} onCitedPapers={onCitedPapers} />
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
					marksShown={prefs.marksShown}
					onMarksShown={(marksShown) => setPrefs({ marksShown })}
					onGoToPaper={goToPaper}
					onNamed={run.reload}
					title={title ?? name}
					citations={counted ?? null}
				/>
			</GraphColouringContext>
		</EvidenceProvider>
	);
}
