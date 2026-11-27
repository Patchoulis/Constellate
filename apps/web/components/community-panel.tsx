"use client";

import { useState } from "react";
import { ArrowDownWideNarrow, ArrowUpNarrowWide, ChevronLeft, Star, Telescope, Trash2, X } from "lucide-react";
import type { Community, GraphNode, RunResult } from "@rp/contracts";
import { AcceptanceTag, AiTag } from "@/components/brand";
import { MathText } from "@/components/math-text";
import { CommunityReviews } from "@/components/review-summary";
import { CommunityStances } from "@/components/stances";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { CITATIONS_NOTE, citationsNote, citedByFigure, citedByOf, citesFigure, citesOf } from "@/lib/citations";
import { communityLabel, type CommunityNode } from "@/lib/community";
import type { MarksOf } from "@/lib/marks";
import { useResizable } from "@/lib/use-resizable";
import { cn, hsl } from "@/lib/utils";
import { count, percent, similarity, SIMILARITY_NOTE } from "@/lib/units";

/** How a user marks a paper, said wherever marking is offered. */
export const MARK_HINT = "Right-click a paper to mark it";

/** What a list of communities can be ordered by. A figure that is not known yet (`of` gives null) sorts last. */
const SORTS: readonly {
	value: string;
	label: string;
	title: string;
	of: (c: Community) => number | null;
	show: (c: Community) => string;
}[] = [
	{
		value: "papers",
		label: "Papers",
		title: "Number of papers",
		of: (c: Community) => c.size,
		show: (c: Community) => count(c.size, "paper"),
	},
	{
		value: "saturation",
		label: "Saturation",
		title: `How densely packed it is: the mean cosine similarity between two of its papers. ${SIMILARITY_NOTE}`,
		of: (c: Community) => c.mean_inner_similarity,
		show: (c: Community) => similarity(c.mean_inner_similarity, 3),
	},
	{
		value: "tightness",
		label: "Tightness",
		title: `Mean cosine similarity of its papers to its mean embedding. ${SIMILARITY_NOTE}`,
		of: (c: Community) => c.similarity_to_mean,
		show: (c: Community) => similarity(c.similarity_to_mean, 3),
	},
	{
		value: "highlights",
		label: "Highlights",
		title: "Share of its papers accepted as orals or spotlights",
		of: (c: Community) => ((c.acceptance.oral ?? 0) + (c.acceptance.spotlight ?? 0)) / c.size,
		show: (c: Community) => `${percent(((c.acceptance.oral ?? 0) + (c.acceptance.spotlight ?? 0)) / c.size)} of its papers`,
	},
	{
		value: "cited",
		label: "Cited by",
		title: `How many times its papers are cited by the run's papers, from inside it and outside it. ${CITATIONS_NOTE}`,
		of: citedByOf,
		show: citedByFigure,
	},
	{
		value: "cites",
		label: "Cites",
		title: `How many different works its papers cite, found in your library or not. ${CITATIONS_NOTE}`,
		of: citesOf,
		show: citesFigure,
	},
];
type Sort = { by: (typeof SORTS)[number]; descending: boolean };

export function CommunityDot({ hue }: { hue: number }) {
	return (
		<span className="size-2.5 shrink-0 rounded-full" style={{ background: hsl(hue), boxShadow: `0 0 8px 1px ${hsl(hue, 60, 0.7)}` }} />
	);
}

/** Mark or unmark one paper. Marks are kept per run, in this browser. */
export function MarkButton({ marked, onToggle, className }: { marked: boolean; onToggle: () => void; className?: string }) {
	return (
		<Button
			variant="ghost"
			size="icon"
			aria-pressed={marked}
			title={
				marked
					? "Marked in this run. Click to unmark it."
					: "Mark this paper: it stays marked in this run, listed under Marked, with its links always shown on the graph."
			}
			className={cn(marked && "text-mark hover:text-mark", className)}
			onClick={onToggle}
		>
			<Star fill={marked ? "currentColor" : "none"} />
		</Button>
	);
}

/** A marked paper as the Marked tab lists it. */
export interface MarkedPaper {
	node: GraphNode;
	community?: CommunityNode;
}

/** The run's marked papers, newest first: open one, unmark one, or unmark them all after saying so twice. */
function MarkedList({
	papers,
	onOpen,
	onRemove,
	onClear,
}: {
	papers: MarkedPaper[];
	onOpen: (id: string) => void;
	onRemove: (id: string) => void;
	onClear: () => void;
}) {
	const [confirming, setConfirming] = useState(false);
	return (
		<>
			<div className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
				{confirming ? (
					<>
						<span className="min-w-0 flex-1 text-xs">Unmark all {count(papers.length, "paper")}? This cannot be undone.</span>
						<Button
							variant="outline"
							onClick={() => {
								onClear();
								setConfirming(false);
							}}
						>
							Unmark all
						</Button>
						<Button variant="ghost" onClick={() => setConfirming(false)}>
							Keep
						</Button>
					</>
				) : (
					<>
						<span className="min-w-0 flex-1 px-1 text-xs text-muted-foreground tabular-nums">
							{count(papers.length, "paper")} marked in this run, newest first
						</span>
						<Button variant="ghost" title="Unmark every paper of this run (asks first)" onClick={() => setConfirming(true)}>
							<Trash2 />
							Clear all
						</Button>
					</>
				)}
			</div>
			<ul className="min-h-0 flex-1 overflow-y-auto p-2">
				{papers.map(({ node, community }) => (
					<li key={node.id} className="flex items-start gap-1">
						<RowButton className="flex-col gap-1" title="Open it in the reader and show it on the graph" onClick={() => onOpen(node.id)}>
							<span className="font-sans text-base leading-snug">
								<MathText>{node.title}</MathText>
							</span>
							{community && (
								<span className="flex w-full min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
									<CommunityDot hue={community.hue} />
									<span className="truncate">{communityLabel(community.community)}</span>
								</span>
							)}
							<span className="flex flex-wrap items-center gap-1 empty:hidden">
								<AcceptanceTag state={node.acceptance} term={node.acceptance_term} small />
								{node.ai && <AiTag small />}
							</span>
						</RowButton>
						<Button variant="ghost" size="icon" className="mt-1" title="Unmark this paper" onClick={() => onRemove(node.id)}>
							<X />
						</Button>
					</li>
				))}
			</ul>
		</>
	);
}

/** A community as one line: dot, title, a figure. */
function Line({ node, figure, onClick }: { node: CommunityNode; figure: string; onClick: () => void }) {
	return (
		<RowButton className="items-center gap-2" onClick={onClick}>
			<CommunityDot hue={node.hue} />
			<span className="min-w-0 flex-1 truncate text-sm">{communityLabel(node.community)}</span>
			<span className="shrink-0 text-xs text-muted-foreground tabular-nums">{figure}</span>
		</RowButton>
	);
}

function Stat({ label, title, children }: { label: string; title?: string; children: React.ReactNode }) {
	return (
		<div className="flex justify-between" title={title}>
			<dt className="text-muted-foreground">{label}</dt>
			<dd>{children}</dd>
		</div>
	);
}

function Detail({
	node,
	marks,
	onSelect,
	onOverview,
	onOpenPaper,
}: {
	node: CommunityNode;
	marks: MarksOf;
	onSelect: (node: CommunityNode) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	const { community, siblings, children } = node;
	return (
		<div className="flex flex-col gap-4 p-4">
			<Button variant="outline" onClick={() => onOverview(node)}>
				<Telescope />
				Overview: methods, findings, directions
			</Button>
			{community.description ? (
				<p className="font-sans text-base text-foreground">{community.description}</p>
			) : (
				<p className="text-xs text-muted-foreground">
					No written description yet: the backend has no Claude API key. Set ANTHROPIC_API_KEY and run again; the download and the
					embeddings are reused.
				</p>
			)}
			<div className="flex flex-wrap gap-1">
				{community.keywords.map((keyword) => (
					<span key={keyword} className="rounded-sm border px-1.5 py-0.5 text-xs text-muted-foreground">
						{keyword}
					</span>
				))}
			</div>
			<dl className="flex flex-col gap-1 text-xs tabular-nums">
				{Object.entries(community.mix).map(([tag, papers]) => (
					<Stat key={tag} label={tag}>
						{count(papers, "paper")}
					</Stat>
				))}
				{SORTS.slice(1, 3).map((sort) => (
					<Stat key={sort.value} label={sort.label} title={sort.title}>
						{sort.show(community)}
					</Stat>
				))}
				{SORTS.slice(4).map((sort) => (
					<Stat key={sort.value} label={sort.label} title={`${sort.title} ${citationsNote(community)}`}>
						{sort.show(community)}
					</Stat>
				))}
				{(["oral", "spotlight"] as const).map(
					(tier) =>
						community.acceptance[tier] !== undefined && (
							<Stat key={tier} label={tier === "oral" ? "Orals" : "Spotlights"}>
								{count(community.acceptance[tier] ?? 0, "paper")}
							</Stat>
						),
				)}
				<Stat label="Tagged AI detected" title="Papers the statistical AI-text detector flagged: an estimate">
					{count(community.ai_flagged, "paper")}
				</Stat>
			</dl>
			{children.length > 0 && (
				<section>
					<Heading className="mb-1.5">Sub-communities</Heading>
					{children.map((child) => (
						<Line key={child.community.id} node={child} figure={count(child.community.size, "paper")} onClick={() => onSelect(child)} />
					))}
				</section>
			)}
			<section>
				<Heading className="mb-1.5">Closest {node.depth ? "sibling" : "communities"}</Heading>
				{community.closest_communities.map((other) => (
					<Line key={other.id} node={siblings[other.id]} figure={similarity(other.cosine)} onClick={() => onSelect(siblings[other.id])} />
				))}
			</section>
			<section>
				<Heading className="mb-1.5">Nearest to the mean embedding</Heading>
				{community.nearest_to_centroid.map((paper) => (
					<div key={paper.id} className="flex items-start gap-1">
						<RowButton className="items-baseline gap-2" onClick={() => onOpenPaper(paper.id)}>
							<span className="flex-1 font-sans text-base leading-snug">
								<MathText>{paper.title}</MathText>
							</span>
							<span className="shrink-0 text-xs text-muted-foreground tabular-nums" title={SIMILARITY_NOTE}>
								{similarity(paper.cosine)}
							</span>
						</RowButton>
						<MarkButton className="mt-0.5" marked={marks.has(paper.id)} onToggle={() => marks.toggle(paper.id)} />
					</div>
				))}
			</section>
		</div>
	);
}

function List({ nodes, sort, onSelect }: { nodes: CommunityNode[]; sort: Sort; onSelect: (node: CommunityNode) => void }) {
	const values = nodes.map((node) => sort.by.of(node.community)).filter((value): value is number => value !== null);
	// Counts are bars from zero; similarities sit in a narrow band, so their bars start just under the lowest.
	const low = sort.by.value === "saturation" || sort.by.value === "tightness" ? Math.min(...values) * 0.9 : 0;
	const span = Math.max(...values) - low || 1;
	// a figure not known yet goes last whichever way the list is ordered
	const ordered = [...nodes].sort((a, b) => {
		const [x, y] = [sort.by.of(a.community), sort.by.of(b.community)];
		if (x === null || y === null) return (x === null ? 1 : 0) - (y === null ? 1 : 0);
		return (sort.descending ? -1 : 1) * (x - y);
	});
	return (
		<ul className="flex flex-col">
			{ordered.map((node) => (
				<li key={node.community.tree_path}>
					<RowButton className="flex-col gap-1.5 py-2" onClick={() => onSelect(node)}>
						<span className="flex w-full items-center gap-2">
							<CommunityDot hue={node.hue} />
							<span className="min-w-0 flex-1 truncate text-sm">{communityLabel(node.community)}</span>
							<span className="text-xs text-muted-foreground tabular-nums">{sort.by.show(node.community)}</span>
						</span>
						<span className="block h-0.5 w-full rounded-full bg-accent">
							<span
								className="block h-full rounded-full"
								style={{
									width: `${(100 * ((sort.by.of(node.community) ?? low) - low)) / span}%`,
									background: hsl(node.hue, 60, 0.8),
								}}
							/>
						</span>
						<span className="w-full truncate text-xs text-muted-foreground">{node.community.keywords.slice(2, 7).join(", ")}</span>
					</RowButton>
				</li>
			))}
		</ul>
	);
}

export function CommunityPanel({
	result,
	roots,
	selected,
	onSelect,
	onOverview,
	onOpenPaper,
	marks,
	markedPapers,
	onGoToPaper,
}: {
	result: RunResult;
	roots: CommunityNode[];
	selected: CommunityNode | null;
	onSelect: (node: CommunityNode | null) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
	marks: MarksOf;
	/** the run's marked papers, newest first */
	markedPapers: MarkedPaper[];
	/** open a paper and move the graph to it */
	onGoToPaper: (id: string) => void;
}) {
	const [sort, setSort] = useState<Sort>({ by: SORTS[0], descending: true });
	const [tab, setTab] = useState<"communities" | "marked">("communities");
	const tabs = [
		{ value: "communities" as const, label: "Communities", title: "The run's communities and what is in them" },
		{
			value: "marked" as const,
			label: `Marked · ${count(markedPapers.length, "paper")}`,
			title: "The papers you marked in this run, newest first",
			icon: <Star />,
		},
	];
	const { ref, style, handle } = useResizable("communities");
	return (
		<aside ref={ref} style={style} className="relative flex w-(--panel-size) shrink-0 flex-col border-l bg-card">
			<ResizeHandle {...handle} />
			<div className="flex shrink-0 border-b px-3 py-2">
				<Segmented className="w-full" options={tabs} value={tab} onChange={setTab} />
			</div>
			{tab === "marked" ? (
				markedPapers.length > 0 ? (
					<MarkedList papers={markedPapers} onOpen={onGoToPaper} onRemove={marks.remove} onClear={marks.clear} />
				) : (
					<p className="p-6 text-center text-xs leading-relaxed text-muted-foreground">
						No marked papers in this run yet. {MARK_HINT} on the graph, or use the star beside a paper here and in the reader. Marked papers
						keep their links on the graph and are listed here.
					</p>
				)
			) : (
				<Communities
					result={result}
					roots={roots}
					selected={selected}
					sort={sort}
					marks={marks}
					onSort={setSort}
					onSelect={onSelect}
					onOverview={onOverview}
					onOpenPaper={onOpenPaper}
				/>
			)}
		</aside>
	);
}

/** The Communities tab: the list of communities, or one community in detail. */
function Communities({
	result,
	roots,
	selected,
	sort,
	marks,
	onSort: setSort,
	onSelect,
	onOverview,
	onOpenPaper,
}: {
	result: RunResult;
	roots: CommunityNode[];
	selected: CommunityNode | null;
	/** kept by the panel, so it survives a look at the Marked tab */
	sort: Sort;
	marks: MarksOf;
	onSort: (sort: Sort) => void;
	onSelect: (node: CommunityNode | null) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	return (
		<>
			<div className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
				{selected ? (
					<>
						<Button
							variant="ghost"
							size="icon"
							title={selected.parent ? `Back to ${communityLabel(selected.parent.community)}` : "All communities"}
							onClick={() => onSelect(selected.parent)}
						>
							<ChevronLeft />
						</Button>
						<CommunityDot hue={selected.hue} />
						<span className="min-w-0 flex-1 truncate text-sm font-semibold">{communityLabel(selected.community)}</span>
						<span className="shrink-0 text-xs text-muted-foreground tabular-nums">{count(selected.community.size, "paper")}</span>
					</>
				) : (
					<>
						<span className="flex-1 px-1 text-sm font-semibold">All communities</span>
						<span className="text-xs text-muted-foreground tabular-nums">
							{count(roots.length, "community", "communities")} · modularity {result.modularity.toFixed(2)} of 1
						</span>
					</>
				)}
			</div>
			{selected ? (
				<div className="min-h-0 flex-1 overflow-y-auto">
					<Detail node={selected} marks={marks} onSelect={onSelect} onOverview={onOverview} onOpenPaper={onOpenPaper} />
					<CommunityReviews run={result.name} treePath={selected.community.tree_path} onOpenPaper={onOpenPaper} />
					<CommunityStances
						key={`${result.name}/${selected.community.tree_path}`}
						run={result.name}
						scope={{ community: selected.community.tree_path }}
						onOpenPaper={onOpenPaper}
					/>
				</div>
			) : (
				<>
					<div className="flex items-center gap-2 border-b px-3 py-2">
						<Segmented
							className="min-w-0 flex-1 flex-wrap"
							options={SORTS}
							value={sort.by.value}
							onChange={(value) => setSort({ ...sort, by: SORTS.find((option) => option.value === value)! })}
						/>
						<Button
							variant="ghost"
							size="icon"
							className="ml-auto"
							title={sort.descending ? "Highest first" : "Lowest first"}
							onClick={() => setSort({ ...sort, descending: !sort.descending })}
						>
							{sort.descending ? <ArrowDownWideNarrow /> : <ArrowUpNarrowWide />}
						</Button>
					</div>
					<div className="min-h-0 flex-1 overflow-y-auto p-2">
						<List nodes={roots} sort={sort} onSelect={onSelect} />
					</div>
				</>
			)}
		</>
	);
}
