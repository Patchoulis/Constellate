"use client";

/**
 * A paper's Weaknesses section in the reader (engine analysis/critique.rs): what the reviewers raised first, in
 * their own words, each point opening on its place in the review; and, apart and only on request, the model's
 * reading of the paper, every statement of it with the paper's words it rests on. There is no score and no verdict
 * anywhere in it.
 */
import { useState } from "react";
import { Loader2, RotateCcw, Sparkles, Square } from "lucide-react";
import type { Critique, CritiqueModelPart, CritiqueReviewer, CritiqueReviewers, ModelWeakness, PointReading } from "@rp/contracts";
import { EvidenceLink, EvidenceLinks } from "@/components/reader/evidence-link";
import { Button, Heading } from "@/components/ui/button";
import { useConfirm } from "@/components/ui/confirm";
import { api } from "@/lib/api";
import {
	CONCERN_NAME,
	KIND_NAME,
	MODEL_NOTE,
	REVIEWERS_NOTE,
	absenceLine,
	attachmentNote,
	estimateLine,
	estimateTitle,
	leftOutLines,
	pointInSection,
	readingsByPoint,
	reviewerName,
	scoreLine,
	scoreTitle,
} from "@/lib/features/critique/critique";
import { choiceKey, isFor, useChosenModel } from "@/lib/chosen-model";
import { accepting, shownModel } from "@/lib/cost";
import { ready, useAsync } from "@/lib/use-async";
import { elsewhereLine, useFollowedJob } from "@/lib/use-followed-job";
import { count, knownMoney } from "@/lib/units";
import { cn } from "@/lib/utils";

/** The reviewer's own words: shown as said; a click shows them in their section of the review, marked. */
function ReviewWords({ reviewer, point }: { reviewer: CritiqueReviewer; point: CritiqueReviewer["points"][number] }) {
	const [open, setOpen] = useState(false);
	const section = reviewer.sections.find((one) => one.index === point.section);
	const [before, marked, after] = section ? pointInSection(section, point) : ["", point.text, ""];
	return (
		<span className="flex flex-col gap-1">
			<button
				type="button"
				className="text-left font-sans text-sm whitespace-pre-line text-foreground hover:underline"
				title={`${reviewerName(reviewer)}'s own words${section ? `, ${section.name}` : ""}: characters ${point.span.start.toLocaleString()} to ${point.span.end.toLocaleString()} of the kept reviews page. Click to see them in their section.`}
				onClick={() => setOpen(!open)}
			>
				{point.text}
			</button>
			{open && section && (
				<span className="block max-h-64 overflow-y-auto rounded-md border bg-background/40 p-2 font-sans text-xs whitespace-pre-line text-muted-foreground">
					<span className="mb-1 block font-ui text-foreground">
						{reviewerName(reviewer)}, {section.name}
					</span>
					{before}
					<mark className="rounded-sm bg-primary/20 px-0.5 text-foreground">{marked}</mark>
					{after}
				</span>
			)}
		</span>
	);
}

/** The model's reading of one reviewer's point, beside it: where it applies, and what the paper says. */
function PointNote({ reading }: { reading: PointReading }) {
	return (
		<span className="flex flex-col gap-0.5 border-l border-primary/40 pl-2 text-xs text-muted-foreground" title={MODEL_NOTE}>
			<span className="text-primary/80">The model&apos;s reading, not the reviewer&apos;s:</span>
			{reading.location?.kind === "passage" && (
				<span>
					It is about: “{reading.location.evidence.quote}” <EvidenceLink evidence={reading.location.evidence} />
				</span>
			)}
			{reading.location?.kind === "no_passage" && (
				<span className="text-warning">The paper has no passage on this, as the model reads it. Absence: check yourself.</span>
			)}
			{reading.paper_says && (
				<span>
					The paper says: “{reading.paper_says.quote}” <EvidenceLink evidence={reading.paper_says} />
				</span>
			)}
		</span>
	);
}

function Reviewer({ reviewer, readings }: { reviewer: CritiqueReviewer; readings: Map<string, PointReading> | null }) {
	const [reply, setReply] = useState<number | null>(null);
	return (
		<li className="flex flex-col gap-1.5">
			<div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs">
				<span className="text-foreground">{reviewerName(reviewer)}</span>
				{reviewer.rating && (
					<span className="text-muted-foreground tabular-nums" title={scoreTitle(reviewer.rating)}>
						{scoreLine(reviewer.rating)}
					</span>
				)}
				{reviewer.confidence && (
					<span className="text-muted-foreground tabular-nums" title={scoreTitle(reviewer.confidence)}>
						{scoreLine(reviewer.confidence)}
					</span>
				)}
			</div>
			{reviewer.points.length === 0 && reviewer.not_split.length === 0 && (
				<p className="text-xs text-muted-foreground">No weaknesses, questions or limitations sections with points in this review.</p>
			)}
			<ul className="flex flex-col gap-2">
				{reviewer.points.map((point) => {
					const reading = point.id && readings ? readings.get(point.id) : undefined;
					return (
						<li key={`${point.span.start}`} className="grid grid-cols-[5.5rem_1fr] gap-x-2">
							<span className="pt-0.5 text-xs text-muted-foreground">{CONCERN_NAME[point.concern]}</span>
							<span className="flex flex-col gap-1">
								<ReviewWords reviewer={reviewer} point={point} />
								{reading && <PointNote reading={reading} />}
							</span>
						</li>
					);
				})}
			</ul>
			{reviewer.not_split.map((name) => (
				<p key={name} className="text-xs text-muted-foreground">
					{name}: strengths and weaknesses together, not marked apart by the reviewer, so not listed here. See the review itself.
				</p>
			))}
			{reviewer.replies.map((one) => (
				<div key={one.note} className="text-xs text-muted-foreground">
					<button
						type="button"
						className="text-left hover:text-foreground hover:underline"
						title="Whether a reply settles a point is not something a program can tell: read it."
						onClick={() => setReply(reply === one.note ? null : one.note)}
					>
						The authors replied to this review ({attachmentNote(one.attachment)})
					</button>
					{reply === one.note && (
						<p className="mt-1 max-h-64 overflow-y-auto rounded-md border bg-background/40 p-2 font-sans whitespace-pre-line">
							{one.text ??
								"The reviews page is not kept, so the reply's words cannot be shown: open the paper's Reviews tab to read the page again."}
							{one.cut && " …"}
						</p>
					)}
				</div>
			))}
		</li>
	);
}

function ReviewersPart({
	part,
	readings,
	onReviews,
}: {
	part: CritiqueReviewers;
	readings: Map<string, PointReading> | null;
	onReviews?: () => void;
}) {
	if (part.state === "none_kept")
		return (
			<p className="text-xs text-muted-foreground">
				No reviews are kept for this paper.{" "}
				{onReviews ? (
					<button type="button" className="underline-offset-2 hover:underline" onClick={onReviews}>
						Open its Reviews tab once to read them.
					</button>
				) : (
					"Open its Reviews tab in the desktop app once to read them."
				)}
			</p>
		);
	return (
		<div className="flex flex-col gap-3">
			<Heading>What the reviewers raised</Heading>
			<p className="text-xs text-muted-foreground" title={REVIEWERS_NOTE}>
				{count(part.points, "point")} from {count(part.reviewers.length, "review")}, read from the reviews page on{" "}
				{part.read_at.slice(0, 10)}
			</p>
			<ul className="flex flex-col gap-4">
				{part.reviewers.map((reviewer) => (
					<Reviewer key={reviewer.note} reviewer={reviewer} readings={readings} />
				))}
			</ul>
		</div>
	);
}

function Weakness({ weakness }: { weakness: ModelWeakness }) {
	const absence = absenceLine(weakness.absence);
	return (
		<li className="flex flex-col gap-1">
			<span className="text-xs text-muted-foreground">{KIND_NAME[weakness.kind]}</span>
			<span className="font-sans text-sm text-foreground" title={MODEL_NOTE}>
				{weakness.weakness}
			</span>
			<span className="text-xs text-muted-foreground">
				About: “{weakness.about.quote}” <EvidenceLink evidence={weakness.about} />
			</span>
			{absence && <span className="text-xs text-warning">{absence}</span>}
			{weakness.absence.kind === "but_see" && <EvidenceLinks evidence={weakness.absence.found} />}
		</li>
	);
}

function ModelPart({
	part,
	current,
	shown,
	onShow,
	onChanged,
	id,
}: {
	part: CritiqueModelPart;
	/** the estimate is answered for the model chosen now: false while it is asked for again */
	current: boolean;
	shown: boolean;
	onShow: (shown: boolean) => void;
	onChanged: () => void;
	id: string;
}) {
	// the job is followed until it ends, also when the reader's tab was left and opened again; then what is kept is
	// read again
	const { job, elsewhere, start } = useFollowedJob(() => onChanged(), `critique:${id}`, { feature: "critique", subject: { paper: id } });
	const ask = (asked: Extract<CritiqueModelPart, { state: "not_asked" }>) =>
		void start(async () => {
			// the model and the most shown go with the request: the engine refuses if either changed
			const started = await api.readCritique(id, shownModel(asked), accepting(asked.estimate));
			if (started.outcome === "started") return started.job;
			onChanged(); // read before: nothing was asked
			return null;
		});
	const asking = { running: job.state === "running", starting: job.state === "starting" };
	const [forgetError, setForgetError] = useState<string | null>(null);
	const confirm = useConfirm();

	const heading = <Heading title={MODEL_NOTE}>The model&apos;s reading, not a review</Heading>;
	if (part.state === "no_pdf")
		return (
			<p className="text-xs text-muted-foreground">
				The model&apos;s reading needs the paper&apos;s PDF, which the library does not have: only the reviewers&apos; points are shown.
			</p>
		);
	if (part.state === "unreadable")
		return <p className="text-xs text-muted-foreground">The paper&apos;s PDF could not be read, so the model cannot be asked about it.</p>;
	if (part.state === "not_asked") {
		const cost = estimateLine(part.estimate);
		const running = asking.running || asking.starting;
		const spent =
			job.state === "running" && job.progress
				? knownMoney(job.progress.usd > 0 ? job.progress.usd : null, { code: "USD", rate: 1, date: null })
				: null;
		return (
			<div className="flex flex-col items-start gap-2">
				{heading}
				<p className="text-xs text-muted-foreground">
					On request, the model reads the paper: where in it each reviewer&apos;s point applies, and weaknesses the reviewers did not raise,
					each with the paper&apos;s words it is about. It gives no score and no verdict.
					{part.points_asked > 0 && ` It would be asked about ${count(part.points_asked, "reviewer's point", "reviewers' points")}.`}
					{part.cut !== null &&
						` The paper is too long to send whole: only its first ${count(part.cut, "character")} (as sent, with page marks) would be read.`}
				</p>
				{job.state === "running" ? (
					<div className="flex items-center gap-2 text-xs text-muted-foreground">
						<Loader2 className="size-3.5 animate-spin" />
						The model is reading the paper{spent ? ` (${spent} spent so far)` : ""}
						{job.unanswered && ` (${job.unanswered}: looking again.)`}
						<Button variant="ghost" size="icon" title="Stop" onClick={() => void api.cancelJob(job.id).catch(() => undefined)}>
							<Square />
						</Button>
					</div>
				) : !current ? (
					<p className="text-xs text-muted-foreground">Working out the cost for the model chosen now…</p>
				) : (
					<Button
						variant="outline"
						disabled={!part.api_key_set || running || elsewhere !== null}
						title={estimateTitle(part.estimate)}
						onClick={() => ask(part)}
					>
						{asking.starting ? <Loader2 className="animate-spin" /> : <Sparkles />}
						{`Ask the model (${part.model}): ${cost}, an estimate`}
					</Button>
				)}
				{!part.api_key_set && (
					<p className="text-xs text-muted-foreground">The chosen provider&apos;s key is not set, so the model cannot be asked.</p>
				)}
				{elsewhere && !running && <p className="text-xs text-muted-foreground">{elsewhereLine(elsewhere)}</p>}
				{job.state === "failed" && <p className="text-xs text-destructive">{job.id === null ? `Not asked: ${job.error}` : job.error}</p>}
			</div>
		);
	}
	const { reading } = part;
	const left = leftOutLines(reading.left_out);
	return (
		<div className="flex flex-col gap-2">
			{heading}
			{!shown ? (
				<Button variant="outline" className="self-start" onClick={() => onShow(true)} title={MODEL_NOTE}>
					Show the model&apos;s reading ({count(reading.weaknesses.length, "weakness", "weaknesses")} the reviewers did not raise)
				</Button>
			) : (
				<>
					<p className="text-xs text-muted-foreground" title={MODEL_NOTE}>
						Written by {part.model} on {part.kept_at.slice(0, 10)} from the paper&apos;s text. Its reading of each reviewer&apos;s point is
						shown beside the point above; below are weaknesses it reads that the reviewers did not raise. Each rests on the quoted words;
						none is a verdict.
					</p>
					{reading.weaknesses.length === 0 ? (
						<p className="text-xs text-muted-foreground">The model raised no weakness beyond the reviewers&apos; points.</p>
					) : (
						<ul className="flex flex-col gap-3">
							{reading.weaknesses.map((weakness, index) => (
								<Weakness key={`${weakness.kind}-${index}`} weakness={weakness} />
							))}
						</ul>
					)}
					{left.map((line) => (
						<p key={line} className="text-xs text-muted-foreground">
							{line}
						</p>
					))}
					<div className="flex gap-2">
						<Button variant="ghost" onClick={() => onShow(false)}>
							Hide the model&apos;s reading
						</Button>
						<Button
							variant="ghost"
							title="Forget this reading: the model can then be asked again, at the cost shown then."
							onClick={async () => {
								const sure = await confirm({
									title: "Forget the model's reading of this paper's weaknesses?",
									body: <p>It cannot be undone, and asking again costs again. The cost is shown before anything is sent.</p>,
									confirm: "Forget it",
									destructive: true,
								});
								if (!sure) return;
								setForgetError(null);
								void api.forgetCritique(id).then(onChanged, (error: Error) => setForgetError(error.message));
							}}
						>
							<RotateCcw />
							Forget it
						</Button>
					</div>
					{forgetError && <p className="text-xs text-destructive">{forgetError}</p>}
				</>
			)}
		</div>
	);
}

/** The Weaknesses section of a paper. `onReviews` opens the reader's Reviews tab, where reviews are read. */
export function Weaknesses({ id, onReviews, className }: { id: string; onReviews?: () => void; className?: string }) {
	const [shown, setShown] = useState(false);
	const chosen = useChosenModel();
	// read again when the model chosen changes (its estimate is the model's) or after asking; the reviewers' part
	// stays in view meanwhile, and the estimate cannot be accepted until it is the new one
	const loaded = useAsync(() => api.critique(id), [id, choiceKey(chosen)]);
	const critique = ready(loaded);
	if (loaded.status === "failed")
		return <p className="font-ui text-xs text-destructive">The weaknesses could not be read: {loaded.error}</p>;
	if (!critique) return null;
	const model = critique.model;
	const current = loaded.status === "ready" && (model.state !== "not_asked" || isFor(model, chosen));
	return (
		<WeaknessesOf
			critique={critique}
			current={current}
			shown={shown}
			onShow={setShown}
			onChanged={loaded.reload}
			onReviews={onReviews}
			className={className}
		/>
	);
}

/** The section drawn from what the engine answered (apart, so that it can be drawn without the engine). */
export function WeaknessesOf({
	critique,
	current = true,
	shown,
	onShow,
	onChanged,
	onReviews,
	className,
}: {
	critique: Critique;
	/** the estimate in it is answered for the model chosen now (false while it is asked for again) */
	current?: boolean;
	shown: boolean;
	onShow: (shown: boolean) => void;
	onChanged: () => void;
	onReviews?: () => void;
	className?: string;
}) {
	const readings = critique.model.state === "answered" && shown ? readingsByPoint(critique.model.reading) : null;
	return (
		<section className={cn("flex flex-col gap-4 font-ui", className)}>
			<Heading className="text-foreground">Weaknesses</Heading>
			<ReviewersPart part={critique.reviewers} readings={readings} onReviews={onReviews} />
			<ModelPart part={critique.model} current={current} shown={shown} onShow={onShow} onChanged={onChanged} id={critique.paper_id} />
		</section>
	);
}
