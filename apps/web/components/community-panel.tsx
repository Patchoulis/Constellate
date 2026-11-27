"use client";

import { useState } from "react";
import { ArrowDownWideNarrow, ArrowUpNarrowWide, ChevronLeft, Sparkles } from "lucide-react";
import type { Community, RunResult } from "@rp/contracts";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { communityLabel, type CommunityNode } from "@/lib/community";
import { hsl } from "@/lib/utils";

/** What a list of communities can be ordered by. */
const SORTS = [
	{
		value: "papers",
		label: "Papers",
		title: "Number of papers",
		of: (c: Community) => c.size,
		show: (c: Community) => String(c.size),
	},
	{
		value: "saturation",
		label: "Saturation",
		title: "How densely packed it is: the mean similarity between two of its papers",
		of: (c: Community) => c.mean_inner_similarity,
		show: (c: Community) => c.mean_inner_similarity.toFixed(3),
	},
	{
		value: "tightness",
		label: "Tightness",
		title: "Mean similarity of its papers to its mean embedding",
		of: (c: Community) => c.similarity_to_mean,
		show: (c: Community) => c.similarity_to_mean.toFixed(3),
	},
	{
		value: "highlights",
		label: "Highlights",
		title: "Share of its papers accepted as orals or spotlights",
		of: (c: Community) => ((c.acceptance.oral ?? 0) + (c.acceptance.spotlight ?? 0)) / c.size,
		show: (c: Community) => `${Math.round((100 * ((c.acceptance.oral ?? 0) + (c.acceptance.spotlight ?? 0))) / c.size)}%`,
	},
] as const;
type Sort = { by: (typeof SORTS)[number]; descending: boolean };

export function CommunityDot({ hue }: { hue: number }) {
	return (
		<span
			className="size-2.5 shrink-0 rounded-full"
			style={{ background: hsl(hue), boxShadow: `0 0 8px 1px ${hsl(hue, 60, 0.7)}` }}
		/>
	);
}

/** A community as one line: dot, title, a figure. */
function Line({ node, figure, onClick }: { node: CommunityNode; figure: string; onClick: () => void }) {
	return (
		<RowButton className="items-center gap-2" onClick={onClick}>
			<CommunityDot hue={node.hue} />
			<span className="min-w-0 flex-1 truncate text-sm">{communityLabel(node.community)}</span>
			<span className="text-xs text-muted-foreground tabular-nums">{figure}</span>
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
	onSelect,
	onOverview,
	onOpenPaper,
}: {
	node: CommunityNode;
	onSelect: (node: CommunityNode) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	const { community, siblings, children } = node;
	return (
		<div className="flex flex-col gap-4 p-4">
			<Button variant="outline" onClick={() => onOverview(node)}>
				<Sparkles />
				Overview: methods, findings, directions
			</Button>
			{community.description ? (
				<p className="font-sans text-base text-foreground">{community.description}</p>
			) : (
				<p className="text-xs text-muted-foreground">
					No written description yet: the backend has no Claude API key. Set ANTHROPIC_API_KEY and run again; the
					download and the embeddings are reused.
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
				{Object.entries(community.mix).map(([tag, count]) => (
					<Stat key={tag} label={tag}>
						{count}
					</Stat>
				))}
				{SORTS.slice(1, 3).map((sort) => (
					<Stat key={sort.value} label={sort.label} title={sort.title}>
						{sort.show(community)}
					</Stat>
				))}
				{(["oral", "spotlight"] as const).map(
					(tier) =>
						community.acceptance[tier] !== undefined && (
							<Stat key={tier} label={tier === "oral" ? "Orals" : "Spotlights"}>
								{community.acceptance[tier]}
							</Stat>
						),
				)}
				<Stat label="Tagged AI detected" title="Papers the statistical AI-text detector flagged: an estimate">
					{community.ai_flagged}
				</Stat>
			</dl>
			{children.length > 0 && (
				<section>
					<Heading className="mb-1.5">Sub-communities</Heading>
					{children.map((child) => (
						<Line key={child.community.id} node={child} figure={String(child.community.size)} onClick={() => onSelect(child)} />
					))}
				</section>
			)}
			<section>
				<Heading className="mb-1.5">Closest {node.depth ? "sibling" : "communities"}</Heading>
				{community.closest_communities.map((other) => (
					<Line
						key={other.id}
						node={siblings[other.id]}
						figure={other.cosine.toFixed(2)}
						onClick={() => onSelect(siblings[other.id])}
					/>
				))}
			</section>
			<section>
				<Heading className="mb-1.5">Nearest to the mean embedding</Heading>
				{community.nearest_to_centroid.map((paper) => (
					<RowButton key={paper.id} className="items-baseline gap-2" onClick={() => onOpenPaper(paper.id)}>
						<span className="flex-1 font-sans text-base leading-snug">
							<MathText>{paper.title}</MathText>
						</span>
						<span className="text-xs text-muted-foreground tabular-nums">{paper.cosine.toFixed(2)}</span>
					</RowButton>
				))}
			</section>
		</div>
	);
}

function List({ nodes, sort, onSelect }: { nodes: CommunityNode[]; sort: Sort; onSelect: (node: CommunityNode) => void }) {
	const values = nodes.map((node) => sort.by.of(node.community));
	// Sizes are bars from zero; similarities sit in a narrow band, so their bars start just under the lowest.
	const low = sort.by.value === "papers" || sort.by.value === "highlights" ? 0 : Math.min(...values) * 0.9;
	const span = Math.max(...values) - low || 1;
	const ordered = [...nodes].sort(
		(a, b) => (sort.descending ? -1 : 1) * (sort.by.of(a.community) - sort.by.of(b.community)),
	);
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
									width: `${(100 * (sort.by.of(node.community) - low)) / span}%`,
									background: hsl(node.hue, 60, 0.8),
								}}
							/>
						</span>
						<span className="w-full truncate text-xs text-muted-foreground">
							{node.community.keywords.slice(2, 7).join(", ")}
						</span>
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
}: {
	result: RunResult;
	roots: CommunityNode[];
	selected: CommunityNode | null;
	onSelect: (node: CommunityNode | null) => void;
	onOverview: (node: CommunityNode) => void;
	onOpenPaper: (id: string) => void;
}) {
	const [sort, setSort] = useState<Sort>({ by: SORTS[0], descending: true });
	return (
		<aside className="flex w-88 shrink-0 flex-col border-l bg-card">
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
						<span className="text-xs text-muted-foreground tabular-nums">{selected.community.size}</span>
					</>
				) : (
					<>
						<span className="flex-1 px-1 text-sm font-semibold">Communities</span>
						<span className="text-xs text-muted-foreground tabular-nums">
							{roots.length} · modularity {result.modularity.toFixed(2)}
						</span>
					</>
				)}
			</div>
			{selected ? (
				<div className="min-h-0 flex-1 overflow-y-auto">
					<Detail node={selected} onSelect={onSelect} onOverview={onOverview} onOpenPaper={onOpenPaper} />
				</div>
			) : (
				<>
					<div className="flex items-center gap-2 border-b px-3 py-2">
						<Segmented
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
		</aside>
	);
}
