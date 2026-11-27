"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { Catalog, Ratings } from "@rp/contracts";
import { CommunityPanel } from "@/components/community-panel";
import { GraphView } from "@/components/graph-view";
import { OverviewPanel } from "@/components/overview";
import { PaperReader } from "@/components/paper-reader";
import { api } from "@/lib/api";
import { communityTree, type CommunityNode } from "@/lib/community";
import type { Job } from "@/lib/use-job";
import { useAsync } from "@/lib/use-async";

/** The run's log, folded to its last line until opened. */
export function ActivityLog({ job }: { job: Job }) {
	const [open, setOpen] = useState(false);
	const box = useRef<HTMLPreElement>(null);
	const running = job.state === "running";
	useEffect(() => setOpen(running), [running]);
	useEffect(() => {
		if (box.current) box.current.scrollTop = box.current.scrollHeight;
	}, [job.log, open]);
	if (job.log.length === 0) return null;
	return (
		<div className="absolute bottom-3 left-3 z-10 w-[min(34rem,calc(100%-4.5rem))] rounded-md border bg-card/95 shadow-lg">
			<button onClick={() => setOpen((value) => !value)} className="flex h-8 w-full items-center gap-2 px-3 text-left text-xs">
				<span className={running ? "size-1.5 animate-pulse rounded-full bg-success" : "size-1.5 rounded-full bg-muted-foreground"} />
				<span className="flex-1 truncate text-muted-foreground">{job.log[job.log.length - 1].trim()}</span>
				{open ? <ChevronDown className="size-3.5" /> : <ChevronUp className="size-3.5" />}
			</button>
			{open && (
				<pre ref={box} className="max-h-56 overflow-auto border-t px-3 py-2 text-xs whitespace-pre-wrap text-muted-foreground">
					{job.log.join("\n")}
				</pre>
			)}
		</div>
	);
}

export function Notice({ children }: { children: React.ReactNode }) {
	return (
		<div className="flex size-full items-center justify-center p-8 text-center text-sm text-muted-foreground">
			{children}
		</div>
	);
}

/** One run on screen: its graph, its communities, and the paper being read. Mount it with key={name}, so
 *  opening another run starts with nothing selected. `children` are laid over the graph. */
export function Workspace({
	name,
	catalog,
	children,
}: {
	name: string;
	catalog: Catalog | undefined;
	children?: React.ReactNode;
}) {
	const run = useAsync(() => Promise.all([api.run(name), api.graph(name)]), [name]);
	const [selected, setSelected] = useState<CommunityNode | null>(null);
	const [paper, setPaper] = useState<string | null>(null);
	const [overview, setOverview] = useState<CommunityNode | null>(null); // the community whose overview is open
	const [result, graph] = run.status === "ready" ? run.data : [];
	const roots = useMemo(() => (result ? communityTree(result.communities) : []), [result]);
	// reviewers' scores: those read before, and those read from a Reviews tab opened since
	const kept = useAsync(() => api.ratings(), [name]);
	const [fresh, setFresh] = useState<Record<string, Ratings>>({});
	const ratings = useMemo(() => ({ ...(kept.status === "ready" ? kept.data : {}), ...fresh }), [kept, fresh]);
	const onRatings = useCallback((id: string, read: Ratings) => setFresh((before) => ({ ...before, [id]: read })), []);

	if (!result || !graph) {
		return (
			<main className="relative min-w-0 flex-1">
				<Notice>{run.status === "failed" ? run.error : "Loading the run…"}</Notice>
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
		<>
			<main className="relative min-w-0 flex-1">
				<GraphView graph={graph} roots={roots} focus={selected} selectedPaper={paper} ratings={ratings} onPickPaper={setPaper} />
				{children}
				{overview && (
					<OverviewPanel
						key={overview.community.tree_path}
						run={name}
						node={overview}
						onOpenPaper={setPaper}
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
						onRatings={onRatings}
						onOpen={setPaper}
						onClose={() => setPaper(null)}
					/>
				)}
			</main>
			<CommunityPanel
				result={result}
				roots={roots}
				selected={selected}
				onSelect={setSelected}
				onOverview={setOverview}
				onOpenPaper={setPaper}
			/>
		</>
	);
}
