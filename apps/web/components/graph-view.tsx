"use client";

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Maximize2 } from "lucide-react";
import type { GraphData, Ratings } from "@rp/contracts";
import { AcceptanceTag, AiTag, RatingTags } from "@/components/brand";
import { CommunityDot } from "@/components/community-panel";
import { GraphCanvasRenderer, type CanvasData, type CanvasLevel } from "@/components/graph-canvas";
import { MathText } from "@/components/math-text";
import { Button } from "@/components/ui/button";
import { communityLabel, groupsAt, treeDepth, type CommunityNode } from "@/lib/community";
import { placeLabels } from "@/lib/labels";
import { countKinds, PAPER_KINDS, shownPapers, type PaperKind } from "@/lib/paper-filter";
import { cn } from "@/lib/utils";
import { boxOf, layoutGraph, type Layout } from "@/lib/layout";

/** A paper's halo shrinks with the level, so the glows of smaller groups stay separate. */
const GLOW = [48, 34, 24, 17];
const LEVEL_NAMES = ["Communities", "Sub-communities", "Sub-sub-communities"];

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
	const sizes = Float32Array.from(graph.nodes, (node) => ({ oral: 1.9, spotlight: 1.45, poster: 1 })[node.acceptance ?? "poster"]);
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
	onPickPaper,
}: {
	graph: GraphData;
	/** the run's communities, indexed like GraphNode.community */
	roots: CommunityNode[];
	/** the community to show in full and move the camera to; everything else is dimmed */
	focus: CommunityNode | null;
	selectedPaper: string | null;
	/** reviewers' scores of the papers that have them, by paper id */
	ratings: Record<string, Ratings>;
	onPickPaper: (id: string | null) => void;
}) {
	const canvasRef = useRef<HTMLCanvasElement>(null);
	const rendererRef = useRef<GraphCanvasRenderer | null>(null);
	const pickRef = useRef(onPickPaper);
	pickRef.current = onPickPaper;
	const [hover, setHover] = useState<{ index: number; x: number; y: number; linked: { index: number; x: number; y: number }[] } | null>(null);
	const [level, setLevel] = useState(0);
	const layout = useMemo(() => layoutGraph(graph, roots), [graph, roots]);

	// The renderer lives as long as this graph does.
	useEffect(() => {
		const canvas = canvasRef.current;
		if (!canvas) return;
		const renderer = new GraphCanvasRenderer(canvas, {
			onHover: (index, x, y, linked) => setHover(index === null ? null : { index, x, y, linked }),
			onPick: (index) => pickRef.current(index === null ? null : graph.nodes[index].id),
			onLevel: setLevel,
		});
		rendererRef.current = renderer;
		renderer.setData(canvasData(graph, roots, layout));
		renderer.fit(layout.bounds, true);
		return () => {
			renderer.destroy();
			rendererRef.current = null;
		};
	}, [graph, roots, layout]);

	// Focusing a community dims the rest and brings it into view; clearing the focus shows everything again.
	useEffect(() => {
		const renderer = rendererRef.current;
		if (!renderer) return;
		renderer.fit(focus ? boxOf(layout.discs.get(focus)!) : layout.bounds);
	}, [layout, focus]);

	// What is shown in full: the focused community's papers (or all), minus the kinds switched off above the graph.
	const [hidden, setHidden] = useState<ReadonlySet<PaperKind>>(new Set());
	const counts = useMemo(() => countKinds(graph.nodes), [graph]);
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

	// Place the hover labels once they have been measured and before the browser paints: the hovered paper's
	// first (it gets the best spot), then its linked papers'.
	const frameRef = useRef<HTMLDivElement>(null);
	const labels = useRef<(HTMLDivElement | null)[]>([]);
	useLayoutEffect(() => {
		const frame = frameRef.current;
		if (!frame || !hover) return;
		const points = [{ ...hover, gap: 14, above: false }, ...hover.linked.map((other) => ({ ...other, gap: 8, above: true }))];
		const shown = points.flatMap((point, order) => {
			const box = labels.current[order];
			return box ? [{ box, ...point }] : [];
		});
		const places = placeLabels(
			{ width: frame.clientWidth, height: frame.clientHeight },
			shown.map(({ box, x, y, gap, above }) => ({ x, y, gap, above, width: box.offsetWidth, height: box.offsetHeight })),
		);
		shown.forEach(({ box }, order) => {
			box.style.left = `${places[order].left}px`;
			box.style.top = `${places[order].top}px`;
		});
	});

	const hovered = hover ? graph.nodes[hover.index] : null;
	const hoveredCommunity = hovered ? roots[hovered.community] : undefined;
	return (
		<div ref={frameRef} className="relative size-full overflow-hidden">
			<canvas ref={canvasRef} className="block size-full cursor-grab active:cursor-grabbing" />
			<div className="absolute top-3 right-3 left-3 flex flex-wrap items-center gap-1.5 text-xs">
				<div className="pointer-events-none rounded-md border bg-card/90 px-2.5 py-1 text-muted-foreground">
					{LEVEL_NAMES[Math.min(level, LEVEL_NAMES.length - 1)]} · zoom in to go deeper
				</div>
				{/* Which kinds of paper are shown in full: click one to dim it, again to bring it back. */}
				{PAPER_KINDS.filter(({ kind }) => counts[kind] > 0).map(({ kind, label, note }) => (
					<button
						key={kind}
						type="button"
						aria-pressed={!hidden.has(kind)}
						title={`${note}. Click to ${hidden.has(kind) ? "show" : "dim"} them.`}
						onClick={() => toggle(kind)}
						className={cn(
							"rounded-md border bg-card/90 px-2 py-1 tabular-nums transition-colors hover:border-ring",
							hidden.has(kind) ? "text-muted-foreground/50 line-through" : "text-foreground",
						)}
					>
						{label} <span className="text-muted-foreground">{counts[kind].toLocaleString()}</span>
					</button>
				))}
			</div>
			<Button
				variant="outline"
				size="icon"
				className="absolute right-3 bottom-3 bg-card"
				title="Fit the graph to the view"
				onClick={() => rendererRef.current?.fit(layout.bounds, true)}
			>
				<Maximize2 />
			</Button>
			{/* The hovered paper's label, and in small the papers it is linked to. They are placed together (below), so
			    that each stays inside the graph and none covers another. */}
			{hover && hovered && (
				<div
					key="hovered"
					ref={(box) => void (labels.current[0] = box)}
					className="pointer-events-none absolute z-10 w-max max-w-80 rounded-md border bg-popover px-2.5 py-1.5 shadow-lg"
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
						<AcceptanceTag state={hovered.acceptance} />
						{hovered.ai && <AiTag hedged />}
						<RatingTags ratings={ratings[hovered.id]} />
					</div>
				</div>
			)}
			{hover?.linked.map((other, order) => {
				const node = graph.nodes[other.index];
				return (
					<div
						key={other.index}
						ref={(box) => void (labels.current[order + 1] = box)}
						className="pointer-events-none absolute z-[5] w-max max-w-56 rounded-sm border bg-popover/95 px-1.5 py-1 shadow-md"
					>
						<div className="font-sans text-xs leading-tight text-foreground/90">
							<Title text={node.title} />
						</div>
						<div className="mt-0.5 flex flex-wrap items-center gap-1 empty:hidden">
							<AcceptanceTag state={node.acceptance} small />
							{node.ai && <AiTag small hedged />}
							<RatingTags ratings={ratings[node.id]} small most={1} />
						</div>
					</div>
				);
			})}
		</div>
	);
}
