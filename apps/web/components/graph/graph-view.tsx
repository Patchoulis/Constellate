"use client";

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { LocateFixed, Star, X } from "lucide-react";
import type { GraphData, Ratings } from "@rp/contracts";
import { AcceptanceTag, AiTag, RatingTags } from "@/components/brand";
import { CommunityDot, MARK_HINT } from "@/components/community/community-panel";
import { GraphCanvasRenderer, type CanvasData, type CanvasLevel } from "@/components/graph/graph-canvas";
import { MathText } from "@/components/math-text";
import { Button } from "@/components/ui/button";
import { communityLabel, groupsAt, treeDepth, type CommunityNode } from "@/lib/community/community";
import { pickMarkedLabels, placeLabels, type MarkedPlace, type PaperPlace } from "@/lib/graph/labels";
import type { Marks } from "@/lib/community/marks";
import { useSelectKeyName, type SelectKey } from "@/lib/ui/prefs";
import { panelSize } from "@/lib/ui/use-resizable";
import { count } from "@/lib/units";
import { countKinds, kindNotes, PAPER_KINDS, shownPapers, type PaperKind } from "@/lib/graph/paper-filter";
import { canvasColouring, legendNote, useGraphColouring } from "@/lib/graph/graph-colouring";
import { cn, hsl } from "@/lib/utils";
import { boxOf, layoutGraph, type Layout } from "@/lib/graph/layout";

/** A paper's halo shrinks with the level, so the glows of smaller groups stay separate. */
const GLOW = [48, 34, 24, 17];
const LEVEL_NAMES = ["Communities", "Sub-communities", "Sub-sub-communities"];
/** At most this many marked papers, and papers linked to them, are named on the graph at once: with many marks,
 *  more would cover the graph. Their rings and links are always drawn; a label that finds no free place is
 *  left out rather than laid over another. */
const MOST_LABELS = { marked: 12, linked: 24 };

/** A label on the graph, and whose it is: the hovered paper, a paper linked to it, a marked paper, or a paper
 *  linked to a marked one. They are placed in this order, so the first get the best places. */
type Labelled = PaperPlace & { kind: "hovered" | "linked" | "marked" | "markedLinked" };
const LABEL_PLACING: Record<Labelled["kind"], { gap: number; above: boolean; optional: boolean }> = {
	hovered: { gap: 14, above: false, optional: false },
	linked: { gap: 8, above: true, optional: false },
	marked: { gap: 10, above: false, optional: true },
	markedLinked: { gap: 8, above: true, optional: true },
};

/** What the renderer draws: the papers' positions and one grouping per level of detail. */
function canvasData(graph: GraphData, roots: CommunityNode[], layout: Layout): CanvasData {
	const levels: CanvasLevel[] = [];
	for (let level = 0; level <= treeDepth(roots); level++) {
		const nodes = groupsAt(roots, level);
		const of = new Int32Array(graph.nodes.length);
		nodes.forEach((node, group) => {
			for (const index of node.community.members) of[index] = group;
		});
		levels.push({
			of,
			glow: GLOW[Math.min(level, GLOW.length - 1)],
			groups: nodes.map((node) => ({
				hue: node.hue,
				label: communityLabel(node.community),
				size: node.community.size,
				...layout.discs.get(node)!,
			})),
		});
	}
	// orals and spotlights are drawn larger: they are the papers a conference singled out
	const sizes = Float32Array.from(graph.nodes, (node) => ({ oral: 1.9, spotlight: 1.45, poster: 1 })[node.acceptance ?? "poster"] ?? 1); // a tier this version does not know: usual size
	return { points: layout.points, sizes, links: graph.edges, levels };
}

/** A paper's title with its formulas set. Kept between renders: a label follows its paper as the camera moves,
 *  many times a second, and setting formulas again each time would be wasted work. */
const Title = memo(function Title({ text }: { text: string }) {
	return <MathText>{text}</MathText>;
});

export function GraphView({
	graph,
	roots,
	focus,
	selectedPaper,
	ratings,
	marked,
	marksShown,
	selectKey,
	goTo,
	onPickPaper,
	onMarkPaper,
	onMarkPapers,
}: {
	graph: GraphData;
	/** the run's communities, indexed like GraphNode.community */
	roots: CommunityNode[];
	/** the community to show in full and move the camera to; everything else is dimmed */
	focus: CommunityNode | null;
	selectedPaper: string | null;
	/** reviewers' scores of the papers that have them, by paper id */
	ratings: Record<string, Ratings>;
	/** the papers the user marked, newest first */
	marked: Marks;
	/** whether the marked papers are shown as marked on the graph */
	marksShown: boolean;
	/** the key that, held, makes a click mark a paper and a drag draw a lasso */
	selectKey: SelectKey;
	/** a paper to move the camera to; each new request moves it again, even to the same paper */
	goTo: { id: string } | null;
	onPickPaper: (id: string | null) => void;
	/** mark the paper, or unmark it */
	onMarkPaper: (id: string) => void;
	/** mark these papers: those a lasso went round */
	onMarkPapers: (ids: string[]) => void;
}) {
	const canvasRef = useRef<HTMLCanvasElement>(null);
	const rendererRef = useRef<GraphCanvasRenderer | null>(null);
	const pickRef = useRef(onPickPaper);
	pickRef.current = onPickPaper;
	const markRef = useRef(onMarkPaper);
	markRef.current = onMarkPaper;
	const markManyRef = useRef(onMarkPapers);
	markManyRef.current = onMarkPapers;
	// The hovered paper and the marked papers' places are told by the renderer as indexes into the graph it was given,
	// and kept with that graph: a run's graph read again (its papers renamed, the run made again) has other papers at
	// those indexes, or fewer, and for the render before the new renderer tells anything they would name papers that
	// are not there.
	const [hoverHeld, setHover] = useState<{ graph: GraphData; index: number; x: number; y: number; linked: PaperPlace[] } | null>(null);
	const hover = hoverHeld?.graph === graph ? hoverHeld : null;
	// where the marked papers are on screen, and how large the screen is
	const [markedHeld, setMarkedView] = useState<{
		graph: GraphData | null;
		places: MarkedPlace[];
		frame: { width: number; height: number };
	}>({
		graph: null,
		places: [],
		frame: { width: 0, height: 0 },
	});
	const markedView = markedHeld.graph === graph ? markedHeld : { places: [], frame: markedHeld.frame };
	const [level, setLevel] = useState(0);
	const keyName = useSelectKeyName(selectKey);
	const layout = useMemo(() => layoutGraph(graph, roots), [graph, roots]);
	const indexOf = useMemo(() => new Map(graph.nodes.map((node, index) => [node.id, index])), [graph]);

	// The renderer lives as long as this graph does.
	useEffect(() => {
		const canvas = canvasRef.current;
		if (!canvas) return;
		const renderer = new GraphCanvasRenderer(canvas, {
			onHover: (index, x, y, linked) => setHover(index === null ? null : { graph, index, x, y, linked }),
			onPick: (index) => pickRef.current(index === null ? null : graph.nodes[index].id),
			onMark: (index) => markRef.current(graph.nodes[index].id),
			onMarkMany: (indexes) => markManyRef.current(indexes.map((index) => graph.nodes[index].id)),
			// told on every camera move: with nothing marked, nothing changes, so nothing is rendered again
			onMarked: (places) =>
				setMarkedView((before) =>
					places.length === 0 && before.places.length === 0
						? before
						: { graph, places, frame: { width: canvas.clientWidth, height: canvas.clientHeight } },
				),
			onLevel: setLevel,
		});
		rendererRef.current = renderer;
		renderer.setData(canvasData(graph, roots, layout));
		renderer.fit(layout.bounds, true, true); // a graph's first view: there is nowhere to glide from
		return () => {
			renderer.destroy();
			rendererRef.current = null;
		};
	}, [graph, roots, layout]);

	// Focusing a community dims the rest and brings it into view; clearing the focus shows everything again.
	useEffect(() => {
		const renderer = rendererRef.current;
		if (!renderer) return;
		// a community that is not in this layout (the tree was made again a moment ago) is no focus: the whole graph
		const disc = focus ? layout.discs.get(focus) : undefined;
		renderer.fit(disc ? boxOf(disc) : layout.bounds);
	}, [layout, focus]);

	// What is shown in full: the focused community's papers (or all), minus the kinds switched off above the graph.
	const [hidden, setHidden] = useState<ReadonlySet<PaperKind>>(new Set());
	const counts = useMemo(() => countKinds(graph.nodes), [graph]);
	const notes = useMemo(() => kindNotes(graph.nodes), [graph]);
	useEffect(() => {
		rendererRef.current?.setFocus(shownPapers(graph.nodes, hidden, focus ? focus.community.members : null));
	}, [graph, layout, focus, hidden]);
	const toggle = (kind: PaperKind) =>
		setHidden((before) => {
			const next = new Set(before);
			if (!next.delete(kind)) next.add(kind);
			return next;
		});

	useEffect(() => {
		const index = selectedPaper === null ? -1 : graph.nodes.findIndex((node) => node.id === selectedPaper);
		rendererRef.current?.setSelected(index < 0 ? null : index);
	}, [graph, layout, selectedPaper]);

	// The marked papers: ringed and linked by the renderer. With a community in focus, only its own marked papers.
	useEffect(() => {
		rendererRef.current?.setMarked(marked.flatMap((id) => indexOf.get(id) ?? []));
	}, [layout, indexOf, marked]);
	useEffect(() => {
		rendererRef.current?.setMarksShown(marksShown);
	}, [layout, marksShown]);
	useEffect(() => {
		rendererRef.current?.setSelectKey(selectKey);
	}, [layout, selectKey]);

	// A feature's groups of papers, drawn in their hues instead of the communities' (lib/graph/graph-colouring.ts).
	const colourings = useGraphColouring();
	const colouring = colourings?.colouring ?? null;
	const painted = useMemo(() => (colouring ? canvasColouring(colouring, indexOf, graph.nodes.length) : null), [colouring, indexOf, graph]);
	useEffect(() => {
		rendererRef.current?.setColouring(painted);
	}, [layout, painted]);

	// Bring a paper into the middle of what the reader, which it opens in, leaves of the graph.
	// Only a new request moves the camera: a new graph is fitted, not moved to an old request.
	const indexRef = useRef(indexOf);
	indexRef.current = indexOf;
	useEffect(() => {
		const index = goTo ? indexRef.current.get(goTo.id) : undefined;
		if (index === undefined) return;
		rendererRef.current?.moveTo(index, panelSize("reader")); // as wide as the user has made it
	}, [goTo]);

	// Where "Recenter" goes: the open paper if there is one, else the focused community, else the whole graph.
	const openIndex = selectedPaper === null ? undefined : indexOf.get(selectedPaper);
	const recentre =
		openIndex !== undefined
			? {
					title: "Bring the open paper back to the middle of the graph",
					go: () => rendererRef.current?.moveTo(openIndex, panelSize("reader")),
				}
			: focus
				? {
						title: `Bring ${communityLabel(focus.community)} back into view`,
						go: () => {
							const disc = layout.discs.get(focus);
							rendererRef.current?.fit(disc ? boxOf(disc) : layout.bounds, !disc);
						},
					}
				: { title: "Bring the whole graph back into view", go: () => rendererRef.current?.fit(layout.bounds, true) };

	// Every label on the graph, most important first: the hovered paper and its linked papers, then the marked
	// papers and theirs (those not named already, within MOST_LABELS).
	const skip = new Set(hover ? [hover.index, ...hover.linked.map((other) => other.index)] : []);
	const picked = pickMarkedLabels(markedView.frame, markedView.places, skip, MOST_LABELS);
	const labelled: Labelled[] = [
		...(hover ? [{ ...hover, kind: "hovered" as const }] : []),
		...(hover?.linked ?? []).map((other) => ({ ...other, kind: "linked" as const })),
		...picked.marked.map((place) => ({ ...place, kind: "marked" as const })),
		...picked.linked.map((place) => ({ ...place, kind: "markedLinked" as const })),
	];

	// Place the labels once they have been measured and before the browser paints, together, so that each stays
	// inside the graph and none covers another; a marked paper's label that finds no free place is hidden.
	const frameRef = useRef<HTMLDivElement>(null);
	const labels = useRef<(HTMLDivElement | null)[]>([]);
	useLayoutEffect(() => {
		const frame = frameRef.current;
		if (!frame) return;
		const shown = labelled.flatMap((label, order) => {
			const box = labels.current[order];
			return box ? [{ box, ...label, ...LABEL_PLACING[label.kind] }] : [];
		});
		const places = placeLabels(
			{ width: frame.clientWidth, height: frame.clientHeight },
			shown.map(({ box, x, y, gap, above, optional }) => ({
				x,
				y,
				gap,
				above,
				optional,
				width: box.offsetWidth,
				height: box.offsetHeight,
			})),
		);
		shown.forEach(({ box }, order) => {
			box.style.left = `${places[order].left}px`;
			box.style.top = `${places[order].top}px`;
			box.style.visibility = places[order].hidden ? "hidden" : "";
		});
	});

	const hovered = hover ? graph.nodes[hover.index] : null;
	const hoveredCommunity = hovered ? roots[hovered.community] : undefined;
	const markedSet = new Set(marked);
	return (
		<div ref={frameRef} className="relative size-full overflow-hidden">
			<canvas ref={canvasRef} className="block size-full cursor-grab active:cursor-grabbing" />
			<div className="absolute top-3 right-3 left-3 flex flex-wrap items-center gap-1.5 text-xs">
				<div className="pointer-events-none rounded-md border bg-card/90 px-2.5 py-1 text-muted-foreground">
					{LEVEL_NAMES[Math.min(level, LEVEL_NAMES.length - 1)]} · zoom in to go deeper
				</div>
				{/* How to mark: plain to see until something is marked, quieter after. */}
				<div
					title={`Right-click a paper's dot to mark it, and again to unmark it. Or hold ${keyName}: a click marks a paper, and a drag draws a lasso that marks every paper inside it (the key can be changed in Settings). A marked paper gets a yellow ring, its links stay shown, and it is listed under Marked on the right, where the eye hides and shows the marks on the graph. While one community is in focus, only the marked papers inside it are shown. Marks are kept for this run, in this browser.`}
					className={cn(
						"flex items-center gap-1.5 rounded-md border px-2.5 py-1 tabular-nums",
						marked.length === 0 ? "border-mark/50 bg-card/90 text-foreground" : "bg-card/70 text-muted-foreground",
					)}
				>
					<Star className={cn("size-3 text-mark", marked.length > 0 && "fill-current")} />
					{MARK_HINT}, or {keyName} + drag a lasso
					{marked.length > 0 && !marksShown && <span>· marks hidden</span>}
					{marked.length > 0 && <span>· {count(marked.length, "paper")} marked</span>}
				</div>
				{/* Which kinds of paper are shown in full: click one to dim it, again to bring it back. */}
				{PAPER_KINDS.filter(({ kind }) => counts[kind] > 0).map(({ kind, label }) => (
					<button
						key={kind}
						type="button"
						aria-pressed={!hidden.has(kind)}
						title={`${notes[kind]}. Click to ${hidden.has(kind) ? "show" : "dim"} them.`}
						onClick={() => toggle(kind)}
						className={cn(
							"rounded-md border bg-card/90 px-2 py-1 tabular-nums transition-colors hover:border-ring",
							hidden.has(kind) ? "text-muted-foreground/50 line-through" : "text-foreground",
						)}
					>
						{label} <span className="text-muted-foreground">{count(counts[kind], "paper")}</span>
					</button>
				))}
			</div>
			{colouring && painted && (
				<div className="absolute bottom-14 left-3 z-[7] flex max-w-72 flex-col gap-1 rounded-md border bg-card/95 px-2.5 py-2 text-xs shadow-md">
					<div className="flex items-start gap-2">
						<span className="min-w-0 flex-1 font-medium text-foreground">Coloured by {colouring.title}</span>
						<Button
							variant="ghost"
							size="icon"
							className="size-5"
							title="Colour the graph by its communities again"
							onClick={() => colourings?.clear(colouring.owner)}
						>
							<X />
						</Button>
					</div>
					<ul className="flex flex-col gap-0.5 tabular-nums">
						{painted.legend.map((entry) => (
							<li key={entry.name} className="flex items-center gap-1.5 text-muted-foreground">
								<span
									className="size-2 shrink-0 rounded-full"
									style={{ background: entry.hue === null ? "rgba(190, 190, 190, 0.75)" : hsl(entry.hue, 74) }}
								/>
								<span className="min-w-0 flex-1 truncate text-foreground" title={entry.name}>
									{entry.name}
								</span>
								<span title={legendNote(entry)}>
									{count(entry.shown, "paper")}
									{entry.earlier > 0 && ` (+${entry.earlier.toLocaleString()} above)`}
								</span>
							</li>
						))}
					</ul>
					<span className="text-muted-foreground">Every other paper is dimmed.</span>
				</div>
			)}
			{/* Back to what is being looked at, after dragging or zooming away from it. */}
			<Button variant="outline" className="absolute right-3 bottom-3 gap-1.5 bg-card" title={recentre.title} onClick={recentre.go}>
				<LocateFixed />
				Recenter
			</Button>
			{/* The hovered paper's label, and in small the papers it is linked to, the marked papers and theirs. They are
			    placed together (above), so that each stays inside the graph and none covers another. */}
			{labelled.map((label, order) => {
				const node = graph.nodes[label.index];
				const ref = (box: HTMLDivElement | null) => void (labels.current[order] = box);
				if (label.kind === "hovered") return null; // drawn below, with more to say
				return (
					<div
						key={`${label.kind}:${label.index}`}
						ref={ref}
						className={cn(
							"enter-fade pointer-events-none absolute w-max max-w-56 rounded-sm border bg-popover/95 px-1.5 py-1 shadow-md",
							label.kind === "marked" ? "z-[6] border-mark/60" : "z-[5]",
						)}
					>
						<div className="font-sans text-xs leading-tight text-foreground/90">
							{label.kind === "marked" && (
								<Star className="mr-1 inline size-2.5 -translate-y-px fill-current text-mark" aria-label="Marked" />
							)}
							<Title text={node.title} />
						</div>
						<div className="mt-0.5 flex flex-wrap items-center gap-1 empty:hidden">
							<AcceptanceTag state={node.acceptance} term={node.acceptance_term} small />
							{node.ai && <AiTag small hedged />}
							<RatingTags ratings={ratings[node.id]} small most={1} />
						</div>
					</div>
				);
			})}
			{hover && hovered && (
				<div
					key="hovered"
					ref={(box) => void (labels.current[0] = box)}
					className="enter-fade pointer-events-none absolute z-10 w-max max-w-80 rounded-md border bg-popover px-2.5 py-1.5 shadow-lg"
				>
					<div className="font-sans text-base leading-snug text-foreground">
						<Title text={hovered.title} />
					</div>
					{hoveredCommunity && (
						<div className="mt-1 flex items-start gap-1.5 text-xs text-muted-foreground">
							<span className="translate-y-0.5">
								<CommunityDot hue={hoveredCommunity.hue} />
							</span>
							<span className="min-w-0 flex-1">{communityLabel(hoveredCommunity.community)}</span>
						</div>
					)}
					<div className="mt-1 flex flex-wrap items-center gap-1 empty:hidden">
						<AcceptanceTag state={hovered.acceptance} term={hovered.acceptance_term} />
						{hovered.ai && <AiTag hedged />}
						<RatingTags ratings={ratings[hovered.id]} />
					</div>
					<div className="mt-1 text-xs text-muted-foreground">
						{markedSet.has(hovered.id) ? (
							<>
								<Star className="mr-1 inline size-3 -translate-y-px fill-current text-mark" />
								Marked · right-click to unmark
							</>
						) : (
							"Right-click to mark it"
						)}
					</div>
				</div>
			)}
		</div>
	);
}
