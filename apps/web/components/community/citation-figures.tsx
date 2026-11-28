"use client";

/**
 * What a community and a run cite and are cited, with where the citations come from (lib/reader/citations.ts): the
 * rows of a community's figures, and the run's own line over the list of communities.
 */
import type { Community, RunCitations } from "@rp/contracts";
import {
	CITATIONS_NOTE,
	CITED_BY_TITLE,
	CITES_TITLE,
	NOT_COUNTABLE,
	citationsNote,
	citedByFigure,
	citedBySplit,
	citesFigure,
	citesSplit,
	runCitationsLine,
	type SplitPart,
} from "@/lib/reader/citations";

function Row({ label, title, children }: { label: string; title?: string; children: React.ReactNode }) {
	return (
		<div className="flex justify-between gap-2" title={title}>
			<dt className="text-muted-foreground">{label}</dt>
			<dd className="text-right">{children}</dd>
		</div>
	);
}

/** One part of a split, under its total. */
function Part({ part }: { part: SplitPart }) {
	return (
		<div className="flex justify-between gap-2 pl-3 text-muted-foreground">
			<dt>{part.label}</dt>
			<dd className="text-right">
				{part.value.toLocaleString()}
				{part.detail && <span className="block">{part.detail}</span>}
			</dd>
		</div>
	);
}

/** A community's "Cited by" and "Cites", each with its parts, and what cannot be counted. For a `<dl>`. */
export function CommunityCitationFigures({ community }: { community: Community }) {
	const note = citationsNote(community);
	const citedBy = citedBySplit(community);
	const cites = citesSplit(community);
	return (
		<>
			<Row label="Cited by" title={`${CITED_BY_TITLE} ${note}`}>
				{citedByFigure(community)}
			</Row>
			{citedBy?.map((part) => (
				<Part key={part.label} part={part} />
			))}
			<div className="pl-3 text-muted-foreground" title={CITATIONS_NOTE}>
				{NOT_COUNTABLE}
			</div>
			<Row label="Cites" title={`${CITES_TITLE} ${note}`}>
				{citesFigure(community)}
			</Row>
			{cites?.map((part) => (
				<Part key={part.label} part={part} />
			))}
		</>
	);
}

/** The run's own figures, over the list of communities when it is ordered by a citation figure. */
export function RunCitationFigures({ citations }: { citations: RunCitations | null }) {
	return (
		<p className="border-b px-4 py-2 text-xs leading-relaxed text-muted-foreground" title={CITATIONS_NOTE}>
			{citations ? `${runCitationsLine(citations)} ${CITATIONS_NOTE} ${NOT_COUNTABLE}` : "The run's citations are not counted yet."}
		</p>
	);
}
