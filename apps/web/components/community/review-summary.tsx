"use client";

/**
 * What reviewers said, taken together (engine analysis/reviews.rs): a community's Reviews section, and the figures
 * of one paper's reviews shown above its Reviews tab. Every figure is on its venue's own scale and names it; a
 * figure that compares venues says it is normalised. All of it is a program's reading of the OpenReview pages the
 * user opened, and says so on hover.
 */
import { useState } from "react";
import { ChevronDown, ChevronRight, Star } from "lucide-react";
import type {
	FieldFigures,
	PaperReviews,
	PaperTheme,
	RankedPaper,
	ReviewTheme,
	Scale,
	Signer,
	ThemeSource,
	VenueRatings,
} from "@rp/contracts";
import { MathText } from "@/components/math-text";
import { Heading, RowButton } from "@/components/ui/button";
import { commonPoints, commonPointsLine, firstPoints, reviewersSharing, SHOWN_POINTS } from "@/lib/community/common-points";
import { api } from "@/lib/net/api";
import { ready, useAsync } from "@/lib/use-async";
import { count, normalised, onScale, percent, scaleName, spreadPoints, starsOf } from "@/lib/units";

/** Said wherever reviewers' figures are shown. */
export const REVIEWS_NOTE =
	"Read by a program from the OpenReview pages you opened in the reader: an estimate of what the reviews say. Check the page before relying on it.";

const COVERAGE_NOTE =
	"Reviews are known only for papers whose OpenReview page you opened in the reader's Reviews tab: the figures cover those papers, not the whole community.";

function scaleNote(scale: Scale): string {
	return scale.source === "assumed"
		? `On ${scaleName(scale)}, assumed: Constellate does not know this venue's form for that year, so the scale is an estimate.`
		: `On ${scaleName(scale)}, as the venue published it for that year.`;
}

function signerName(signer: Signer): string {
	if (signer.role === "reviewer") return `Reviewer ${signer.id}`;
	return {
		authors: "The authors",
		area_chair: "Area chair",
		senior_area_chair: "Senior area chair",
		program_chairs: "Program chairs",
		ethics_reviewer: "Ethics reviewer",
		public: "A public comment",
		not_shown: "A reviewer",
	}[signer.role];
}

/** A score as stars out of its scale's points: grey outlines, and over them as much of the row filled as the score. */
function Stars({ value, scale, label }: { value: number; scale: Scale; label: string }) {
	const stars = starsOf(value, scale);
	if (!stars) return null; // a scale too long for stars: the words beside it say the score
	const row = (filled: boolean) =>
		Array.from({ length: stars.total }, (_, index) => (
			<Star key={index} className="size-3.5 shrink-0" fill={filled ? "currentColor" : "none"} />
		));
	return (
		<span role="img" aria-label={label} className="relative inline-flex shrink-0 align-middle">
			<span className="flex text-muted-foreground/40">{row(false)}</span>
			<span
				className="absolute inset-y-0 left-0 flex overflow-hidden text-mark"
				style={{ width: `${(100 * stars.filled) / stars.total}%` }}
			>
				{row(true)}
			</span>
		</span>
	);
}

/**
 * One field of one paper: its name, its mean as stars out of the scale's points, and the mean in words ("4 of 5").
 * The range, the number of reviews and the confidence-weighted mean are said on hover: the row itself is the mean.
 */
function FieldLine({ figures }: { figures: FieldFigures }) {
	const what = figures.field.toLowerCase();
	const weighted = figures.confidence_weighted;
	const mean = onScale(figures.mean, figures.scale, `mean ${what}`);
	const more = [
		`${mean}, range ${figures.minimum} to ${figures.maximum}, ${count(figures.reviews, "review")}.`,
		...(weighted
			? [
					`${onScale(weighted.mean, figures.scale, "weighted by the reviewers' confidence")} (${count(weighted.reviews, "review")} gave one).`,
				]
			: []),
		scaleNote(figures.scale),
		REVIEWS_NOTE,
	].join(" ");
	return (
		<div className="flex flex-wrap items-center gap-x-2 gap-y-0.5" title={more}>
			<span className="min-w-0 flex-1 text-foreground">{figures.field}</span>
			<Stars value={figures.mean} scale={figures.scale} label={mean} />
			<span className="w-16 shrink-0 text-right">{onScale(figures.mean, figures.scale, "").trim()}</span>
		</div>
	);
}

/** The figures of one paper's reviews, for the strip above its Reviews tab. `version` changes when a page was just read. */
export function PaperReviewFigures({ id, version }: { id: string; version?: unknown }) {
	const kept = ready(useAsync(() => api.paperReviews(id), [id, version]));
	if (!kept || kept.figures.reviews === 0) return null;
	const { figures } = kept;
	return (
		<div className="flex shrink-0 flex-col gap-1 border-b px-3 py-2 font-ui text-xs text-muted-foreground tabular-nums">
			{figures.fields
				.filter((field) => field.field !== "Confidence")
				.map((field) => (
					<FieldLine key={field.field} figures={field} />
				))}
			{/* keyed by the paper the reviews are of: another paper's start collapsed, however this one was left */}
			<CommonPoints key={kept.paper_id} reviews={kept} />
		</div>
	);
}

/** The per-paper mean rating of one venue and year, as bars on that venue's scale. */
function Distribution({ group }: { group: VenueRatings }) {
	const tallest = Math.max(1, ...group.bins.map((bin) => bin.papers));
	const what = `mean ${group.field.toLowerCase()}`;
	return (
		<div className="flex flex-col gap-1" title={`${scaleNote(group.scale)} ${REVIEWS_NOTE}`}>
			<div className="flex justify-between gap-2 text-xs">
				<span className="text-foreground">
					{group.venue.conference} {group.venue.year} · {what} per paper, {scaleName(group.scale)}
				</span>
				<span className="shrink-0 text-muted-foreground tabular-nums">{count(group.papers, "paper")}</span>
			</div>
			<div className="flex h-10 items-end gap-px">
				{group.bins.map((bin) => (
					<div
						key={bin.from}
						className="flex-1 rounded-t-sm bg-primary/60"
						style={{ height: `${(100 * bin.papers) / tallest}%`, minHeight: bin.papers ? 2 : 0 }}
						title={`${count(bin.papers, "paper")} with a ${what} from ${bin.from} to ${onScale(bin.to, group.scale, "").trim()}`}
					/>
				))}
			</div>
			<div className="flex justify-between text-[0.65rem] text-muted-foreground tabular-nums">
				<span>{group.scale.minimum}</span>
				<span>
					{onScale(group.mean, group.scale, "average of the papers' means")}, median {group.median.toFixed(1)}
				</span>
				<span>{group.scale.maximum}</span>
			</div>
		</div>
	);
}

function Ranked({
	papers,
	titles,
	figure,
	onOpenPaper,
}: {
	papers: RankedPaper[];
	titles: Record<string, string>;
	figure: (paper: RankedPaper) => string;
	onOpenPaper: (id: string) => void;
}) {
	return (
		<>
			{papers.map((paper) => (
				<RowButton key={paper.paper_id} className="items-baseline gap-2" onClick={() => onOpenPaper(paper.paper_id)}>
					<span className="flex-1 font-sans text-base leading-snug">
						<MathText>{titles[paper.paper_id] ?? paper.paper_id}</MathText>
					</span>
					<span
						className="shrink-0 text-xs text-muted-foreground tabular-nums"
						title={`${scaleNote(paper.overall.scale)} Ranked by ${normalised(paper.normalised)}, so that venues compare. ${REVIEWS_NOTE}`}
					>
						{figure(paper)}
					</span>
				</RowButton>
			))}
		</>
	);
}

/**
 * A term reviewers use when they praise or criticise, and how many use it; opened, the sentences they use it in,
 * each in the reviewer's own words with who said it. `caption` names where a sentence is; `onSource`, when given,
 * makes each sentence a button.
 */
function ThemeRow({
	term,
	figure,
	note,
	sources,
	more,
	caption,
	onSource,
	size = "text-sm",
}: {
	term: string;
	figure: string;
	note: string;
	sources: ThemeSource[];
	more: number;
	caption: (source: ThemeSource) => React.ReactNode;
	onSource?: (source: ThemeSource) => void;
	size?: string;
}) {
	const [open, setOpen] = useState(false);
	return (
		<li className="min-w-0">
			<button
				type="button"
				aria-expanded={open}
				className="flex w-full min-w-0 items-baseline gap-2 rounded-md px-2 py-1 text-left hover:bg-accent"
				onClick={() => setOpen(!open)}
				title={note}
			>
				<span className={`min-w-0 flex-1 font-sans wrap-anywhere text-foreground ${size}`}>{term}</span>
				<span className="shrink-0 text-xs text-muted-foreground tabular-nums">{figure}</span>
			</button>
			{open && (
				<ul className="ml-2 flex min-w-0 flex-col gap-1 border-l pl-2">
					{sources.map((source) => {
						const said = (
							<>
								<span className={`font-sans wrap-anywhere text-foreground ${size}`}>&ldquo;{source.excerpt}&rdquo;</span>
								<span className="text-xs wrap-anywhere text-muted-foreground">{caption(source)}</span>
							</>
						);
						return (
							<li key={`${source.paper_id}:${source.note}:${source.span.start}`} className="min-w-0">
								{onSource ? (
									<RowButton className="flex-col gap-0.5" onClick={() => onSource(source)} title="Open the paper">
										{said}
									</RowButton>
								) : (
									<div className="flex min-w-0 flex-col gap-0.5 px-2 py-1">{said}</div>
								)}
							</li>
						);
					})}
					{more > 0 && <li className="px-2 text-xs text-muted-foreground">and {count(more, "more review")}</li>}
				</ul>
			)}
		</li>
	);
}

function Theme({ theme, titles, onOpenPaper }: { theme: ReviewTheme; titles: Record<string, string>; onOpenPaper: (id: string) => void }) {
	return (
		<ThemeRow
			term={theme.term}
			figure={`${count(theme.sections, "section")} of ${count(theme.papers, "paper")}`}
			note={`In ${percent(theme.share)} of these sections, ${theme.distinctiveness.toFixed(1)} times as often as in the reviews' sections overall. Counted from the words of the reviews, without a language model. ${REVIEWS_NOTE}`}
			sources={theme.sources}
			more={theme.more_sources}
			caption={(source) => (
				<>
					{signerName(source.signer)}, {source.section}: <MathText>{titles[source.paper_id] ?? source.paper_id}</MathText>
				</>
			)}
			onSource={(source) => onOpenPaper(source.paper_id)}
		/>
	);
}

const COMMON_NOTE =
	"Counted by a program from the words of the reviews, not a model's reading: a term at least two of this paper's reviewers use where they praise (or criticise) it, most widely shared first.";

/** One tone of a paper's points in common: the first few, and a control to show the rest. */
function CommonTone({ heading, points }: { heading: string; points: PaperTheme[] }) {
	const [all, setAll] = useState(false);
	if (points.length === 0) return null;
	const { shown, hidden } = firstPoints(points, all);
	return (
		<div className="flex min-w-0 flex-col">
			<Heading className="px-2 pb-0.5">{heading}</Heading>
			<ul className="flex min-w-0 flex-col">
				{shown.map((point) => (
					<ThemeRow
						key={point.term}
						term={point.term}
						figure={reviewersSharing(point)}
						note={`${reviewersSharing(point)} use it here, ${point.distinctiveness.toFixed(1)} times as often as in this paper's review sections overall. ${COMMON_NOTE} ${REVIEWS_NOTE}`}
						sources={point.sources}
						more={point.more_sources}
						caption={(source) => `${signerName(source.signer)}, ${source.section}`}
						size="text-xs"
					/>
				))}
			</ul>
			{points.length > SHOWN_POINTS && (
				<button
					type="button"
					className="self-start px-2 py-0.5 text-xs text-muted-foreground underline-offset-2 hover:underline"
					onClick={() => setAll(!all)}
				>
					{all ? `Show the first ${SHOWN_POINTS}` : `Show all ${count(points.length, "point")} (${hidden} more)`}
				</button>
			)}
		</div>
	);
}

/**
 * What a paper's reviewers commonly praise and criticise, under its scores: collapsed behind a toggle (each paper
 * starts collapsed: its parent gives it the paper's id as key), or one muted line when there is nothing to show.
 */
function CommonPoints({ reviews }: { reviews: PaperReviews }) {
	const [open, setOpen] = useState(false);
	const points = commonPoints(reviews.figures.reviews, reviews.praise, reviews.criticism);
	if (points.kind === "no_reviews") return null;
	const line = commonPointsLine(points);
	const note = `${COMMON_NOTE} ${REVIEWS_NOTE}`;
	if (points.kind !== "some")
		return (
			<p className="pt-1 text-muted-foreground/80 italic" title={note}>
				{line}
			</p>
		);
	return (
		<div className="flex min-w-0 flex-col pt-1">
			<button
				type="button"
				aria-expanded={open}
				className="-mx-1 flex min-w-0 items-start gap-1 rounded-md px-1 py-0.5 text-left hover:bg-accent"
				onClick={() => setOpen(!open)}
				title={note}
			>
				{open ? <ChevronDown className="mt-px size-3 shrink-0" /> : <ChevronRight className="mt-px size-3 shrink-0" />}
				<span className="min-w-0 flex-1 wrap-anywhere">{line}</span>
			</button>
			{open && (
				<div className="-mx-2 flex max-h-72 min-w-0 flex-col gap-2 overflow-x-hidden overflow-y-auto pt-1">
					<CommonTone heading="Common praise" points={reviews.praise} />
					<CommonTone heading="Common criticism" points={reviews.criticism} />
				</div>
			)}
		</div>
	);
}

/** A community's Reviews section: how many of its papers have reviews read, how they are rated, where reviewers
 *  disagreed, and what they praise and criticise. */
export function CommunityReviews({ run, treePath, onOpenPaper }: { run: string; treePath: string; onOpenPaper: (id: string) => void }) {
	// one community's figures are never shown under another's name while its own are asked for
	const loaded = useAsync(() => api.reviewSummary(run, treePath), [run, treePath], { keep: false });
	const summary = ready(loaded);
	if (loaded.status === "failed")
		return <p className="px-4 pb-4 font-ui text-xs leading-relaxed text-destructive">The reviews could not be read: {loaded.error}</p>;
	if (!summary) return null;
	const disagreed = summary.most_disagreed.filter((paper) => paper.overall.spread > 0);
	return (
		<section className="flex flex-col gap-3 px-4 pb-4">
			<Heading>Reviews</Heading>
			<p className="text-xs text-muted-foreground" title={COVERAGE_NOTE}>
				Reviews read for {summary.papers_with_reviews} of {count(summary.papers, "paper")}
				{summary.reviews > 0 && ` · ${count(summary.reviews, "review")}`}
				{summary.papers_with_reviews === 0 && ". Open a paper's Reviews tab in the reader to read its reviews."}
			</p>
			{summary.ratings.map((group) => (
				<Distribution key={`${group.venue.conference} ${group.venue.year} ${group.field}`} group={group} />
			))}
			{summary.papers_without_venue > 0 && (
				<p className="text-xs text-muted-foreground">
					{count(summary.papers_without_venue, "paper")} with reviews of an unknown venue, left out: their scale is not known.
				</p>
			)}
			{disagreed.length > 0 && (
				<div>
					<Heading className="mb-1.5">Reviewers disagreed most</Heading>
					<Ranked
						papers={disagreed}
						titles={summary.titles}
						onOpenPaper={onOpenPaper}
						figure={(paper) =>
							`${spreadPoints(paper.overall.spread)} of ${paper.overall.scale.maximum}, ${count(paper.overall.reviews, "review")}`
						}
					/>
				</div>
			)}
			{summary.highest_rated.length > 0 && (
				<div>
					<Heading className="mb-1.5">Highest rated</Heading>
					<Ranked
						papers={summary.highest_rated}
						titles={summary.titles}
						onOpenPaper={onOpenPaper}
						figure={(paper) => onScale(paper.overall.mean, paper.overall.scale, `mean ${paper.overall.field.toLowerCase()}`)}
					/>
				</div>
			)}
			{(["praise", "criticism"] as const).map(
				(tone) =>
					summary[tone].length > 0 && (
						<div key={tone}>
							<Heading className="mb-1.5">{tone === "praise" ? "What reviewers praise" : "What reviewers criticise"}</Heading>
							<ul className="flex flex-col">
								{summary[tone].map((theme) => (
									<Theme key={theme.term} theme={theme} titles={summary.titles} onOpenPaper={onOpenPaper} />
								))}
							</ul>
						</div>
					),
			)}
		</section>
	);
}
