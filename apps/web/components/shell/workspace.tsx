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
import { ANSWER_OWNER, GraphColouringContext, colouringStep, type GraphColouring, type PaperColouring } from "@/lib/graph/graph-colouring";
import { useMarks } from "@/lib/community/marks";
import type { Job } from "@/lib/shell/use-job";
import { ready, useAsync } from "@/lib/use-async";
import { usePrefs } from "@/lib/ui/prefs";
import { useResizable } from "@/lib/ui/use-resizable";
import { stepLine } from "@/lib/shell/run-step";
import { SkyLoading } from "@/components/graph/sky-loading";
import { LoadingWords } from "@/components/ui/loading-dots";
import { prefersLessMotion } from "@/lib/graph/glide";
import { BEATS, INTRO_MS, introPlays, type Opening } from "@/lib/graph/intro";

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
	// positioned, so that it is drawn over a sky behind it (components/graph/sky-loading.tsx)
	return (
		<div className="relative flex size-full items-center justify-center p-8 text-center text-sm text-muted-foreground">{children}</div>
	);
}

/** How long the loading sky takes to fade once the graph's intro begins: as long as the intro's stars take to come. */
const SKY_HANDOVER_MS = Math.round(INTRO_MS * BEATS.stars[1]);

/** One run on screen: its graph, its communities, and the paper being read. Mount it with key={name}, so
 *  opening another run starts with nothing selected. `children` are laid over the graph. */
export function Workspace({
	name,
	title,
	catalog,
	model,
	opening = "opened",
	children,
}: {
	name: string;
	/** why the run is shown: made by a job just now, opened, or only shown again; the intro may play for the first two
	 *  (Settings, "Play the constellation intro") */
	opening?: Opening;
	/** the run's title, as the list shows it */
	title?: string;
	catalog: Catalog | undefined;
	/** the language model chosen in Settings, once known */
	model?: string;
	children?: React.ReactNode;
}) {
	const run = useAsync(() => Promise.all([api.run(name), api.graph(name)]), [name], { keep: false });
	const [selected, setSelected] = useState<CommunityNode | null>(null);
	const [paper, setPaper] = useState<string | null>(null);
	const [overview, setOverview] = useState<CommunityNode | null>(null); // the community whose overview is open
	const [saved, graph] = ready(run) ?? [];
	// the citation figures counted now, from every reference list of the library read so far, in place of any
	// saved with the run; until they come (or if they cannot be counted), the saved ones say "not counted yet"
	const counted = ready(useAsync(() => api.runCitations(name), [name], { keep: false }));
	const result = useMemo(() => saved && { ...saved, communities: withCounted(saved.communities, counted) }, [saved, counted]);
	const roots = useMemo(() => (result ? communityTree(result.communities) : []), [result]);
	// the tree made again (the figures came): the community selected is the same one, in the new tree
	useEffect(() => setSelected((before) => before && nodeAt(roots, before.community.tree_path)), [roots]);
	// reviewers' scores: those read before, and those read from a Reviews tab opened since
	const kept = ready(useAsync(() => api.ratings(), [name], { keep: false }));
	const [fresh, setFresh] = useState<Record<string, Ratings>>({});
	const ratings = useMemo(() => ({ ...kept, ...fresh }), [kept, fresh]);
	const onRatings = useCallback((id: string, read: Ratings) => setFresh((before) => ({ ...before, [id]: read })), []);
	// the papers marked in this run; none are shown until the graph says which papers the run has
	const known = useMemo(() => (graph ? new Set(graph.nodes.map((node) => node.id)) : null), [graph]);
	const stored = useMarks(name, known);
	const prefs = usePrefs();
	// Whether the graph comes up with the intro: decided once, as the run is opened.
	const [intro] = useState(() => introPlays(prefs.intro, opening, prefersLessMotion()));
	// the loading sky, fading out over the intro's first stars once the graph is there
	const [skyLeft, setSkyLeft] = useState(!intro);
	// A paper marked is always shown with its links (lib/community/marks.ts); where the person chose that (Settings),
	// marking one also shows every hidden mark again.
	const { showMarksOnMark } = prefs;
	const marks = useMemo(() => {
		const showAll = () => showMarksOnMark && stored.hidden.size > 0 && stored.setShown([...stored.hidden], true);
		return {
			...stored,
			toggle: (id: string) => {
				if (!stored.has(id)) showAll();
				stored.toggle(id);
			},
			addMany: (ids: readonly string[]) => {
				showAll();
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
		const failed = run.status === "failed";
		return (
			<main className="relative min-w-0 flex-1">
				{!failed && <SkyLoading />}
				<Notice>{failed ? run.error : <LoadingWords words="Loading the run" />}</Notice>
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
			<GraphColouringContext value={colourings}>
				<main className="relative min-w-0 flex-1">
					<GraphView
						graph={graph}
						roots={roots}
						focus={selected}
						selectedPaper={paper}
						ratings={ratings}
						marked={marks.marked}
						hiddenMarks={marks.hidden}
						marksWithoutLinks={marks.noLinks}
						selectKey={prefs.selectKey}
						goTo={goTo}
						intro={intro}
						stars={prefs.stars}
						regionFrom={prefs.regionFrom}
						onPickPaper={openPaper}
						onMarkPaper={marks.toggle}
						onMarkPapers={marks.addMany}
					/>
					{!skyLeft && (
						<div
							className="leave-fade pointer-events-none absolute inset-0"
							style={{ animationDuration: `${SKY_HANDOVER_MS}ms` }}
							onAnimationEnd={() => setSkyLeft(true)}
						>
							<SkyLoading />
						</div>
					)}
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
					onGoToPaper={goToPaper}
					onNamed={run.reload}
					title={title ?? name}
					citations={counted ?? null}
				/>
			</GraphColouringContext>
		</EvidenceProvider>
	);
}
