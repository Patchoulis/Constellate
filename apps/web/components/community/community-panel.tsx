"use client";

import { useState } from "react";
import {
	ArrowDownWideNarrow,
	ArrowUpNarrowWide,
	ChartNetwork,
	ChevronLeft,
	Eye,
	EyeOff,
	ListTree,
	Loader2,
	Square,
	Star,
	Tags,
	Telescope,
	Trash2,
	X,
} from "lucide-react";
import type { Community, GraphNode, RunCitations, RunResult } from "@rp/contracts";
import { CommunityAsk } from "@/components/features/answers/ask";
import { useAskNaming } from "@/components/shell/run-dialogs";
import { EvidenceLink } from "@/components/reader/evidence-link";
import { Block, ListLabel, SubBlock } from "@/components/community/block";
import { ClosestLine, ClosestList, useClosest } from "@/components/community/closest-papers";
import { CITED_LABEL, citedIn, quotesTitle, statementOf } from "@/lib/features/writing/closest";
import { api } from "@/lib/net/api";
import { isPlain, leftOutWords, namingRequest, plainOf, sectionsOf, unnamedOf } from "@/lib/features/writing/naming";
import { elsewhereLine, useFollowedJob } from "@/lib/shell/use-followed-job";
import { AcceptanceTag, AiTag } from "@/components/brand";
import { MathText } from "@/components/math-text";
import { CommunityReviews } from "@/components/community/review-summary";
import { CommunityStances } from "@/components/features/stances/stances";
import { ExtractionTable } from "@/components/features/extraction/extraction-table";
import { CommunityCitationKinds } from "@/components/features/citation_contexts/citation-kinds";
import { Button, RowButton, Segmented } from "@/components/ui/button";
import { ResizeHandle } from "@/components/ui/resize-handle";
import {
	CITATIONS_NOTE,
	CITED_BY_TITLE,
	CITES_TITLE,
	NOT_COUNTABLE,
	citedByFigure,
	citedByOf,
	citesFigure,
	citesOf,
} from "@/lib/reader/citations";
import { CommunityCitationFigures, RunCitationFigures } from "@/components/community/citation-figures";
import { communityLabel, type CommunityNode } from "@/lib/community/community";
import { markedListLine, type MarksOf } from "@/lib/community/marks";
import { useResizable } from "@/lib/ui/use-resizable";
import { cn, hsl } from "@/lib/utils";
import { UNIQUENESS_NOTE, uniqueness } from "@/lib/community/community";
import { count, distance, percent, similarity, SIMILARITY_NOTE } from "@/lib/units";

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
		value: "uniqueness",
		label: "Uniqueness",
		title: UNIQUENESS_NOTE,
		of: (c: Community) => uniqueness(c),
		show: (c: Community) => distance(uniqueness(c), 3),
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
		title: `${CITED_BY_TITLE} ${NOT_COUNTABLE}`,
		of: citedByOf,
		show: citedByFigure,
	},
	{
		value: "cites",
		label: "Cites",
		title: CITES_TITLE,
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

/** A marked paper's "on the graph" switch: a graph, struck through when the paper is hidden there. */
function GraphSwitchIcon({ on }: { on: boolean }) {
	return (
		<span className="relative inline-flex">
			<ChartNetwork />
			{!on && <span aria-hidden className="absolute top-1/2 left-[-2px] h-px w-[calc(100%+4px)] -rotate-45 bg-current" />}
		</span>
	);
}

const SHOWN_TITLE =
	"Shown on the graph: its ring, its links and its name. Click to hide it there; it stays marked, listed here, and in the Table and Agree / disagree, which use every marked paper, hidden or not.";
const HIDDEN_TITLE =
	"Hidden on the graph: drawn there as an ordinary paper. It stays marked, listed here, and in the Table and Agree / disagree. Click to show it again.";

/** The run's marked papers, newest first: open one, show or hide it on the graph, draw its links or not, unmark one,
 *  or unmark them all after saying so twice. */
function MarkedList({ papers, marks, onOpen }: { papers: MarkedPaper[]; marks: MarksOf; onOpen: (id: string) => void }) {
	const [confirming, setConfirming] = useState(false);
	const all = papers.map(({ node }) => node.id);
	const hidden = all.filter((id) => marks.hidden.has(id)).length;
	const linksOff = all.filter((id) => marks.noLinks.has(id)).length;
	return (
		<>
			<div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-1 gap-y-1 border-b px-3 py-1.5">
				{confirming ? (
					<>
						<span className="min-w-0 flex-1 text-xs">Unmark all {count(papers.length, "paper")}? This cannot be undone.</span>
						<Button
							variant="outline"
							onClick={() => {
								marks.clear();
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
						<span className="min-w-0 basis-full px-1 text-xs text-muted-foreground tabular-nums">
							{markedListLine(papers.length, hidden, linksOff)}
						</span>
						<Button
							variant="ghost"
							className="h-7 px-2"
							title="Show every marked paper on the graph"
							disabled={hidden === 0}
							onClick={() => marks.setShown(all, true)}
						>
							<GraphSwitchIcon on />
							Show all
						</Button>
						<Button
							variant="ghost"
							className="h-7 px-2"
							title="Hide every marked paper on the graph: they stay marked and listed"
							disabled={hidden === all.length}
							onClick={() => marks.setShown(all, false)}
						>
							<GraphSwitchIcon on={false} />
							Hide all
						</Button>
						<Button
							variant="ghost"
							className="h-7 px-2"
							title="Draw the links of every marked paper shown on the graph"
							disabled={linksOff === 0}
							onClick={() => marks.setLinks(all, true)}
						>
							<Eye />
							Links on
						</Button>
						<Button
							variant="ghost"
							className="h-7 px-2"
							title="Draw no marked paper's links: their rings and names stay"
							disabled={linksOff === all.length}
							onClick={() => marks.setLinks(all, false)}
						>
							<EyeOff />
							Links off
						</Button>
						<Button
							variant="ghost"
							className="h-7 px-2"
							title="Unmark every paper of this run (asks first)"
							onClick={() => setConfirming(true)}
						>
							<Trash2 />
							Clear all
						</Button>
					</>
				)}
			</div>
			<ul className="min-h-0 flex-1 overflow-y-auto p-2">
				{papers.map(({ node, community }) => {
					const shown = !marks.hidden.has(node.id);
					const links = !marks.noLinks.has(node.id);
					return (
						<li key={node.id} className={cn("flex items-start gap-1", !shown && "text-muted-foreground")}>
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
							<span className="mt-1 flex shrink-0 items-center">
								<Button
									variant="ghost"
									size="icon"
									aria-pressed={shown}
									aria-label={shown ? "Shown on the graph" : "Hidden on the graph"}
									title={shown ? SHOWN_TITLE : HIDDEN_TITLE}
									className={cn(shown ? "text-mark hover:text-mark" : "text-muted-foreground/60")}
									onClick={() => marks.setShown([node.id], !shown)}
								>
									<GraphSwitchIcon on={shown} />
								</Button>
								<Button
									variant="ghost"
									size="icon"
									aria-pressed={shown && links}
									aria-label={links ? "Its links are drawn" : "Its links are not drawn"}
									disabled={!shown}
									title={
										!shown
											? "Its links are drawn only while it is shown on the graph: show it first."
											: links
												? "Its links to related papers are drawn. Click to draw only its ring and name; hovering it still shows its links."
												: "Only its ring and name are drawn. Click to draw its links again."
									}
									className={cn(shown && links ? "text-foreground" : "text-muted-foreground/60")}
									onClick={() => marks.setLinks([node.id], !links)}
								>
									{links ? <Eye /> : <EyeOff />}
								</Button>
								<Button variant="ghost" size="icon" title="Unmark this paper" onClick={() => marks.remove(node.id)}>
									<X />
								</Button>
							</span>
						</li>
					);
				})}
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
		<div className="flex min-w-0 justify-between gap-3" title={title}>
			<dt className="min-w-0 text-muted-foreground [overflow-wrap:anywhere]">{label}</dt>
			<dd className="shrink-0 text-right">{children}</dd>
		</div>
	);
}

/** The figures of "At a glance", besides its venues and acceptance tiers (its citations have a block of their own). */
const GLANCE = ["uniqueness", "highlights"];

/**
 * A community's description in its sections: each a quieter sub-header with its own hairline, its text, then the
 * papers the model cited (each the paper's title, which opens the reader at the quoted words, and the quote), then
 * the papers closest to it in meaning, found with the embedding model (none cited above listed again).
 */
function Sections({ community, closest }: { community: Community; closest: ReturnType<typeof useClosest> }) {
	const sections = sectionsOf(community);
	const leftOut = leftOutWords(community.sections_left_out);
	return (
		<div className="flex min-w-0 flex-col gap-4">
			{sections?.map((section) => {
				const found = statementOf(closest.status, section.heading);
				return (
					<SubBlock key={section.heading} title={section.heading}>
						<p className="font-sans text-base text-foreground">{section.text}</p>
						<ListLabel>{CITED_LABEL}</ListLabel>
						<ul className="flex flex-col gap-1.5" aria-label={`Papers cited for ${section.heading}`}>
							{section.cites.map((cite, index) => (
								<li key={`${cite.evidence.paper}-${index}`} className="min-w-0 border-l-2 pl-2 [overflow-wrap:anywhere]">
									<EvidenceLink
										evidence={cite.evidence}
										label={cite.title.trim() || cite.evidence.paper}
										className="flex items-start leading-snug [&_button]:text-left [&>button:first-child]:min-w-0 [&>button:first-child]:flex-1"
									/>
									{!quotesTitle(cite) && (
										<p className="font-sans text-[0.8125rem] leading-snug text-muted-foreground italic">
											&ldquo;{cite.evidence.quote}&rdquo;
										</p>
									)}
								</li>
							))}
						</ul>
						{found && closest.status && <ClosestList papers={found.papers} floor={closest.status.floor} cited={citedIn(section)} />}
					</SubBlock>
				);
			})}
			{!sections && <p className="text-xs text-muted-foreground">None of its description&apos;s sections could be shown.</p>}
			{leftOut && <p className="text-xs text-muted-foreground">{leftOut}</p>}
			<ClosestLine line={closest.line} />
		</div>
	);
}

/**
 * A plain description, written before sections were asked for: once its closest papers are found, each sentence is
 * a short paragraph of its own with the papers closest to it under it; until then, the paragraph as written.
 */
function PlainDescription({ community, closest }: { community: Community; closest: ReturnType<typeof useClosest> }) {
	const statements = closest.status?.statements;
	return (
		<div className="flex min-w-0 flex-col gap-3">
			{statements && statements.length > 0 ? (
				statements.map((statement, index) => (
					<div key={`${index}-${statement.statement}`} className="min-w-0">
						<p className="font-sans text-base text-foreground">{statement.statement}</p>
						<ClosestList papers={statement.papers} floor={closest.status?.floor ?? 0} />
					</div>
				))
			) : (
				<p className="font-sans text-base text-foreground">{community.description}</p>
			)}
			<ClosestLine line={closest.line} />
		</div>
	);
}

/** What the run is, for what describing it again asks first. */
interface Described {
	run: string;
	/** the run's title, said in what is asked first */
	title: string;
	communities: Community[];
	/** the run's communities were described again: read them again */
	onNamed: () => void;
}

function Detail({
	node,
	marks,
	described,
	onSelect,
	onOverview,
	onOpenPaper,
}: {
	node: CommunityNode;
	marks: MarksOf;
	described: Described;
	onSelect: (node: CommunityNode) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	const { community, siblings, children } = node;
	const closest = useClosest(described.run, community.tree_path);
	const glance = SORTS.filter((sort) => GLANCE.includes(sort.value));
	return (
		<div className="flex flex-col gap-5 p-4">
			{/* in a narrow panel its words go onto a second line: never past the panel's edge */}
			<Button variant="outline" className="h-auto min-h-8 py-1.5 text-left whitespace-normal" onClick={() => onOverview(node)}>
				<Telescope />
				<span className="min-w-0">Overview: methods, findings, directions</span>
			</Button>
			<Block title="Description">
				{community.sections !== undefined ? (
					<Sections community={community} closest={closest} />
				) : community.description ? (
					<div className="flex flex-col items-start gap-2">
						<PlainDescription community={community} closest={closest} />
						{isPlain(community) && <RewriteSections {...described} compact />}
					</div>
				) : (
					<p className="text-xs text-muted-foreground">
						No written description yet. &ldquo;Name the communities&rdquo;, over the list of all communities, names and describes them with
						the chosen language model, and shows what that costs first. It needs that model&apos;s key (Settings, Language model, says which
						variable to set).
					</p>
				)}
			</Block>
			<Block title="Characteristic terms" note={count(community.keywords.length, "term")}>
				<div className="flex flex-wrap gap-1">
					{community.keywords.map((keyword) => (
						<span key={keyword} className="rounded-sm border px-1.5 py-0.5 text-xs text-muted-foreground [overflow-wrap:anywhere]">
							{keyword}
						</span>
					))}
				</div>
			</Block>
			<Block title="At a glance" note={count(community.size, "paper")}>
				<dl className="flex flex-col gap-1 text-xs tabular-nums">
					{Object.entries(community.mix).map(([tag, papers]) => (
						<Stat key={tag} label={tag}>
							{count(papers, "paper")}
						</Stat>
					))}
					{glance.map((sort) => (
						<Stat key={sort.value} label={sort.label} title={sort.title}>
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
			</Block>
			<Block title="Citations" noteTitle={CITATIONS_NOTE} note="from reference lists read">
				<dl className="flex flex-col gap-1 text-xs tabular-nums">
					<CommunityCitationFigures community={community} />
				</dl>
			</Block>
			{children.length > 0 && (
				<Block title="Sub-communities" note={count(children.length, "part")}>
					{children.map((child) => (
						<Line key={child.community.id} node={child} figure={count(child.community.size, "paper")} onClick={() => onSelect(child)} />
					))}
				</Block>
			)}
			<Block title={`Closest ${node.depth ? "sibling" : "communities"}`}>
				{community.closest_communities.map((other) => (
					<Line key={other.id} node={siblings[other.id]} figure={similarity(other.cosine)} onClick={() => onSelect(siblings[other.id])} />
				))}
			</Block>
			<Block title="Nearest to the mean embedding">
				{community.nearest_to_centroid.map((paper) => (
					<div key={paper.id} className="flex items-start gap-1">
						<RowButton className="flex-col items-start gap-0.5" onClick={() => onOpenPaper(paper.id)}>
							<span className="w-full min-w-0 font-sans text-base leading-snug [overflow-wrap:anywhere]">
								<MathText>{paper.title}</MathText>
							</span>
							<span className="text-xs text-muted-foreground tabular-nums" title={SIMILARITY_NOTE}>
								{similarity(paper.cosine)}
							</span>
						</RowButton>
						<MarkButton className="mt-0.5" marked={marks.has(paper.id)} onToggle={() => marks.toggle(paper.id)} />
					</div>
				))}
			</Block>
		</div>
	);
}

function List({ nodes, sort, onSelect }: { nodes: CommunityNode[]; sort: Sort; onSelect: (node: CommunityNode) => void }) {
	const values = nodes.map((node) => sort.by.of(node.community)).filter((value): value is number => value !== null);
	// Counts are bars from zero; uniqueness sits in a narrow band, so its bars start just under the lowest.
	const low = sort.by.value === "uniqueness" ? Math.min(...values) * 0.9 : 0;
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
	onNamed,
	title,
	citations = null,
}: {
	/** what the run cites and is cited, counted from every reference list of the library; null until counted */
	citations?: RunCitations | null;
	/** the run's communities were named: read them again */
	onNamed: () => void;
	/** the run's title, said in what is asked before naming its communities */
	title: string;
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
	const markedIds = markedPapers.map(({ node }) => node.id);
	// in one order whatever order they were marked in: what the stances section is keyed and remembered by
	const sortedMarked = [...markedIds].sort();
	// the eye beside the tabs: pressed while any mark is shown on the graph; a click hides them all, or shows them all
	const anyShown = markedIds.some((id) => !marks.hidden.has(id));
	return (
		<aside ref={ref} style={style} className="relative flex w-(--panel-size) shrink-0 flex-col border-l bg-card">
			<ResizeHandle {...handle} />
			<div className="flex shrink-0 items-center gap-1 border-b px-3 py-2">
				<Segmented className="min-w-0 flex-1" options={tabs} value={tab} onChange={setTab} />
				{/* the marks stay; this only hides or shows their rings, links and names on the graph */}
				<Button
					variant="ghost"
					size="icon"
					className="shrink-0"
					aria-pressed={anyShown}
					disabled={markedIds.length === 0}
					title={
						markedIds.length === 0
							? "No paper is marked in this run"
							: anyShown
								? "Marked papers are shown on the graph. Click to hide them all there: they stay marked and listed under Marked, where each can be shown again."
								: "Every marked paper is hidden on the graph. Click to show them all again."
					}
					onClick={() => marks.setShown(markedIds, !anyShown)}
				>
					{anyShown ? <Eye /> : <EyeOff />}
				</Button>
			</div>
			{tab === "marked" ? (
				markedPapers.length > 0 ? (
					<>
						<MarkedList papers={markedPapers} marks={marks} onOpen={onGoToPaper} />
						<div className="max-h-[60%] shrink-0 overflow-y-auto border-t pt-3">
							{/* mounted afresh when the marked papers change: what it shows is for exactly these papers */}
							<CommunityStances
								key={`${result.name}/marked/${sortedMarked.join(",")}`}
								run={result.name}
								scope={{ papers: sortedMarked }}
								onOpenPaper={onGoToPaper}
							/>
							<ExtractionTable run={result.name} scope={{ papers: markedIds }} onOpenPaper={onGoToPaper} />
						</div>
					</>
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
					onNamed={onNamed}
					title={title}
					citations={citations}
				/>
			)}
		</aside>
	);
}

/**
 * "Name the communities", over the list of a run that has unnamed ones (one made without names, or without a key):
 * what naming would cost is shown first (lib/features/writing/naming.ts), the click starts the job, which is followed with its
 * progress and can be stopped, and the run's communities are read again when it ends.
 */
function NameCommunities({
	run,
	title,
	communities,
	onNamed,
}: {
	run: string;
	title: string;
	communities: Community[];
	onNamed: () => void;
}) {
	const askNaming = useAskNaming();
	const [asking, setAsking] = useState(false);
	const [refused, setRefused] = useState<string | null>(null);
	const { job, elsewhere, start } = useFollowedJob(() => onNamed(), `describing:${run}`);
	const { communities: all, unnamed } = unnamedOf(communities);
	const running = job.state === "running" ? job : null;
	if (unnamed === 0 && !running && job.state !== "failed") return null;
	const name = async () => {
		setRefused(null);
		setAsking(true);
		try {
			const estimate = await api.describeEstimate(run, "name");
			if (!(await askNaming(estimate, "name", title))) return;
			await start(async () => (await api.describeRun(run, namingRequest(estimate, "name"))).id);
		} catch (error) {
			setRefused((error as Error).message);
		} finally {
			setAsking(false);
		}
	};
	return (
		<div className="flex flex-col gap-1.5 border-b px-3 py-2 text-xs">
			{running ? (
				<div className="flex items-center gap-2 text-muted-foreground" role="status">
					<Loader2 className="size-3.5 shrink-0 animate-spin" />
					<span className="flex-1">
						Naming the communities
						{running.progress
							? `: ${running.progress.done.toLocaleString()} of ${count(running.progress.of, "community", "communities")}`
							: "…"}
					</span>
					<Button
						variant="ghost"
						size="icon"
						title="Stop: what is under way is finished, nothing new is asked"
						onClick={() => void api.cancelJob(running.id).catch(() => undefined)}
					>
						<Square />
					</Button>
				</div>
			) : (
				<div className="flex items-center justify-between gap-2">
					<span className="text-muted-foreground">
						{unnamed === all
							? `None of the ${count(all, "community", "communities")} has a name.`
							: `${unnamed.toLocaleString()} of ${count(all, "community", "communities")} have no name.`}
					</span>
					<Button
						variant="outline"
						className="h-7"
						disabled={asking || job.state === "starting" || elsewhere !== null}
						title="Shows what naming them with the chosen language model would cost, before anything is asked"
						onClick={() => void name()}
					>
						{asking || job.state === "starting" ? <Loader2 className="animate-spin" /> : <Tags />}
						Name the communities
					</Button>
				</div>
			)}
			{job.state === "failed" && <p className="text-destructive">They were not all named: {job.error}</p>}
			{refused && <p className="text-destructive">{refused}</p>}
			{elsewhere && !running && <p className="text-muted-foreground">{elsewhereLine(elsewhere)}</p>}
		</div>
	);
}

/**
 * "Rewrite with sections and citations", for a run with communities described before sections were asked for: a
 * small button under such a description (`compact`), and a line over the list of all communities. What rewriting
 * them would cost is shown first (the naming dialog, lib/features/writing/naming.ts), the click starts the job,
 * which is followed with its progress and can be stopped, and the run's communities are read again when it ends.
 */
function RewriteSections({ run, title, communities, onNamed, compact = false }: Described & { compact?: boolean }) {
	const askNaming = useAskNaming();
	const [asking, setAsking] = useState(false);
	const [refused, setRefused] = useState<string | null>(null);
	const { job, elsewhere, start } = useFollowedJob(() => onNamed(), `sectioning:${run}`);
	const { communities: all, plain } = plainOf(communities);
	const running = job.state === "running" ? job : null;
	if (plain === 0 && !running && job.state !== "failed") return null;
	const rewrite = async () => {
		setRefused(null);
		setAsking(true);
		try {
			const estimate = await api.describeEstimate(run, "sections");
			if (!(await askNaming(estimate, "sections", title))) return;
			await start(async () => (await api.describeRun(run, namingRequest(estimate, "sections"))).id);
		} catch (error) {
			setRefused((error as Error).message);
		} finally {
			setAsking(false);
		}
	};
	const button = (
		<Button
			variant={compact ? "ghost" : "outline"}
			className="h-auto min-h-7 px-2 py-1 text-left text-xs whitespace-normal"
			disabled={asking || job.state === "starting" || elsewhere !== null}
			title={`Shows what rewriting the ${count(plain, "plain description")} of this run with sections and the language model's own checked citations would cost, before anything is asked`}
			onClick={() => void rewrite()}
		>
			{asking || job.state === "starting" ? <Loader2 className="animate-spin" /> : <ListTree />}
			Rewrite with sections and the model&apos;s own citations
		</Button>
	);
	return (
		<div className={cn("flex min-w-0 flex-col items-start gap-1.5 text-xs", !compact && "border-b px-3 py-2")}>
			{running ? (
				<div className="flex w-full items-center gap-2 text-muted-foreground" role="status">
					<Loader2 className="size-3.5 shrink-0 animate-spin" />
					<span className="min-w-0 flex-1">
						Rewriting with sections and citations
						{running.progress ? `: ${running.progress.done.toLocaleString()} of ${count(running.progress.of, "call")} done` : "…"}
					</span>
					<Button
						variant="ghost"
						size="icon"
						title="Stop: what is under way is finished, nothing new is asked; what was rewritten stays"
						onClick={() => void api.cancelJob(running.id).catch(() => undefined)}
					>
						<Square />
					</Button>
				</div>
			) : (
				<>
					{!compact && (
						<span className="text-muted-foreground">
							{plain.toLocaleString()} of {count(all, "community", "communities")} {plain === 1 ? "has" : "have"} a plain description,
							written before sections and citations.
						</span>
					)}
					{button}
				</>
			)}
			{job.state === "failed" && <p className="text-destructive">They were not all rewritten: {job.error}</p>}
			{refused && <p className="text-destructive">{refused}</p>}
			{elsewhere && !running && <p className="text-muted-foreground">{elsewhereLine(elsewhere)}</p>}
		</div>
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
	onNamed,
	title,
	citations,
}: {
	result: RunResult;
	roots: CommunityNode[];
	selected: CommunityNode | null;
	onNamed: () => void;
	title: string;
	citations: RunCitations | null;
	/** kept by the panel, so it survives a look at the Marked tab */
	sort: Sort;
	marks: MarksOf;
	onSort: (sort: Sort) => void;
	onSelect: (node: CommunityNode | null) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	const described: Described = { run: result.name, title, communities: result.communities, onNamed };
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
				// one community's sections start afresh for another: nothing of the last one is shown under its name
				<div key={`${result.name}/${selected.community.tree_path}`} className="min-h-0 flex-1 overflow-y-auto">
					<Detail
						node={selected}
						marks={marks}
						described={described}
						onSelect={onSelect}
						onOverview={onOverview}
						onOpenPaper={onOpenPaper}
					/>
					<CommunityReviews run={result.name} treePath={selected.community.tree_path} onOpenPaper={onOpenPaper} />
					<CommunityStances
						key={`${result.name}/${selected.community.tree_path}`}
						run={result.name}
						scope={{ community: selected.community.tree_path }}
						onOpenPaper={onOpenPaper}
					/>
					<ExtractionTable run={result.name} scope={{ community: selected.community.tree_path }} onOpenPaper={onOpenPaper} />
					<CommunityCitationKinds run={result.name} treePath={selected.community.tree_path} onOpenPaper={onOpenPaper} />
					<CommunityAsk run={result.name} treePath={selected.community.tree_path} papers={selected.community.size} />
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
					<NameCommunities run={result.name} title={title} communities={result.communities} onNamed={onNamed} />
					<RewriteSections {...described} />
					{(sort.by.value === "cited" || sort.by.value === "cites") && <RunCitationFigures citations={citations} />}
					<div className="min-h-0 flex-1 overflow-y-auto p-2">
						<List nodes={roots} sort={sort} onSelect={onSelect} />
					</div>
				</>
			)}
		</>
	);
}
