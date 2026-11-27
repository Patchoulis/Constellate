"use client";

import { createContext, useContext } from "react";
import { Crosshair } from "lucide-react";
import type { Evidence, ReaderTarget } from "@rp/contracts";
import { api } from "@/lib/api";
import { withSignal } from "@/lib/request";
import { evidenceTitle, readerTarget, shortLabel } from "@/lib/reader/evidence";
import { ready, useAsync } from "@/lib/use-async";
import { cn } from "@/lib/utils";

/** What a citation can do where it is shown: open the reader at a place, and find a paper on the run's graph. */
export interface EvidenceActions {
	open: (target: ReaderTarget) => void;
	/** is this paper one of the open run's? */
	inRun: (paper: string) => boolean;
	/** show the paper on the graph */
	locate: (paper: string) => void;
}

const Actions = createContext<EvidenceActions | null>(null);

/** Wraps whatever shows evidence; the workspace provides it (components/workspace.tsx). */
export const EvidenceProvider = Actions.Provider;

/** Labels of papers already asked for, by id: a list of citations asks for each paper once. */
const labels = new Map<string, Promise<string>>();

function labelOf(paper: string): Promise<string> {
	let label = labels.get(paper);
	if (!label) {
		// shared by every citation of the paper: not called off when the one that asked first goes away
		label = withSignal(null, () => api.paper(paper)).then(shortLabel, () => {
			labels.delete(paper); // shown by its id this time; asked again by the next citation of it
			return paper;
		});
		labels.set(paper, label);
	}
	return label;
}

/**
 * A citation of a place in a paper: the paper's short label ("Vaswani et al., 2017"); on hover, the paper's own
 * words there and how they were found; on click, the reader opens the paper at that place with those words
 * highlighted. When the paper is one of the open run's, a second button finds it on the graph.
 *
 * Every feature shows its evidence with this, and only evidence the engine checked (engine evidence.rs) reaches
 * it. `label` saves asking for the paper when the caller already knows it.
 */
export function EvidenceLink({ evidence, label, className }: { evidence: Evidence; label?: string; className?: string }) {
	const actions = useContext(Actions);
	// another paper's label is never shown for this one, not even while this one's is asked for
	const fetched = useAsync(label ? null : () => labelOf(evidence.paper), [evidence.paper, label], { keep: false });
	const shown = label ?? ready(fetched) ?? evidence.paper;
	const fromAbstract = evidence.established.kind === "abstract";
	return (
		<span className={cn("inline-flex items-baseline gap-0.5 font-ui text-xs", className)}>
			<button
				type="button"
				title={evidenceTitle(evidence, shown)}
				disabled={!actions}
				onClick={() => actions?.open(readerTarget(evidence))}
				className="text-primary underline-offset-2 hover:underline disabled:text-muted-foreground disabled:no-underline"
			>
				{shown}
				{fromAbstract && <span className="text-muted-foreground"> (abstract)</span>}
			</button>
			{actions?.inRun(evidence.paper) && (
				<button
					type="button"
					title="Show this paper on the graph"
					onClick={() => actions.locate(evidence.paper)}
					className="self-center text-muted-foreground hover:text-foreground"
				>
					<Crosshair className="size-3" />
				</button>
			)}
		</span>
	);
}

/** A statement's citations, one after the other: "(Vaswani et al., 2017; Devlin et al., 2019)". */
export function EvidenceLinks({ evidence, className }: { evidence: Evidence[]; className?: string }) {
	if (evidence.length === 0) return null;
	return (
		<span className={cn("font-ui text-xs text-muted-foreground", className)}>
			(
			{evidence.map((piece, index) => (
				<span key={`${piece.paper}-${index}`}>
					{index > 0 && "; "}
					<EvidenceLink evidence={piece} />
				</span>
			))}
			)
		</span>
	);
}
