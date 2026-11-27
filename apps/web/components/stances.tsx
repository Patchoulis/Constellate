"use client";

/**
 * A community's "Agree / disagree" section (engine api/stances.rs): ask a yes/no question of its papers, or start
 * from a term its papers keep using or a question the model proposed; see what it costs before anything is sent;
 * ask the most central papers first and the rest on a second click; then see the papers placed in groups, each
 * with its own words, the counts with what they are counts of, by acceptance tier and by year. Never one verdict.
 * Nothing is shown when the engine has no such route (the Python backend).
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { RotateCcw, Sparkles, Square } from "lucide-react";
import type { JobProgress, StancePlan, StanceProposalPlan, StanceProposals, StanceResult, StanceRow, StanceScope } from "@rp/contracts";
import { AcceptanceTag } from "@/components/brand";
import { EvidenceLinks } from "@/components/evidence-link";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { api } from "@/lib/api";
import {
	abstractOnlyLine,
	APART,
	breakdown,
	costLine,
	costNote,
	COUNTED,
	countsLine,
	coverageLine,
	GROUP_HUE,
	GROUP_LABEL,
	GROUP_NOTE,
	rowsOf,
	skippedLine,
	STANCE_NOTE,
	starter,
	tally,
} from "@/lib/stances";
import { ready, useAsync } from "@/lib/use-async";
import { cn, hsl } from "@/lib/utils";
import { count, currencyOf, money, type Currency } from "@/lib/units";

const DOLLAR: Currency = { code: "USD", rate: 1, date: null };
const POLL_MS = 1200;

/** Where asking a question stands: one of these at a time. */
type Step =
	| { kind: "choosing" }
	| { kind: "estimating"; question: string; includeUnrelated: boolean }
	| { kind: "estimated"; plan: StancePlan; includeUnrelated: boolean }
	| { kind: "asking"; job: number; id: string }
	| { kind: "failed"; error: string };

/** Where proposing questions stands. */
type Proposing =
	| { kind: "idle" }
	| { kind: "estimating" }
	| { kind: "estimated"; plan: StanceProposalPlan; fresh: boolean }
	| { kind: "asking"; job: number }
	| { kind: "failed"; error: string };

/** The user's currency, at the rate the engine knows; US dollars until it answers. */
function useCurrency(): { currency: Currency; locale?: string } {
	const locale = typeof navigator === "undefined" ? undefined : navigator.language;
	const usage = ready(useAsync(() => api.usage(currencyOf(locale)), [locale]));
	return { currency: usage?.currency ?? DOLLAR, locale };
}

/** Follow job `job` until it is no longer running, then call `onEnd` once. Its progress meanwhile. */
function useJobProgress(job: number | null, onEnd: (error: string | null) => void): JobProgress | null {
	const [progress, setProgress] = useState<JobProgress | null>(null);
	const ended = useRef(onEnd);
	ended.current = onEnd;
	useEffect(() => {
		if (job === null) return;
		let stopped = false;
		let timer: ReturnType<typeof setTimeout>;
		const poll = async () => {
			try {
				const status = await api.status(Number.MAX_SAFE_INTEGER);
				if (stopped) return;
				if (status.id !== job || status.state !== "running") {
					ended.current(status.id === job && status.state === "failed" ? status.error : null);
					return;
				}
				setProgress(status.progress ?? null);
			} catch (error) {
				if (stopped) return;
			}
			timer = setTimeout(poll, POLL_MS);
		};
		void poll();
		return () => {
			stopped = true;
			clearTimeout(timer);
		};
	}, [job]);
	return progress;
}

function Running({ job, progress, what, onDone }: { job: number; progress: JobProgress | null; what: string; onDone?: () => void }) {
	const { currency, locale } = useCurrency();
	return (
		<div className="flex items-center gap-2 text-xs text-muted-foreground tabular-nums">
			<span className="flex-1">
				{progress
					? `${progress.done} of ${count(progress.of, what)} done · ${money(progress.usd, currency, locale)} spent so far`
					: "Starting…"}
			</span>
			<Button
				variant="outline"
				title="Stop: nothing new is sent, what is under way finishes, and every answer so far is kept"
				onClick={() => void api.cancelJob(job).then(onDone, onDone)}
			>
				<Square />
				Stop
			</Button>
		</div>
	);
}

function Dot({ hue }: { hue: number | null }) {
	return <span className="size-2 shrink-0 rounded-full" style={{ background: hue === null ? "var(--muted-foreground)" : hsl(hue, 60) }} />;
}

/** One placed paper: its title (opens it), its tier, how it was read, the condition for a mixed one, its quotes. */
function PlacedPaper({ row, onOpenPaper }: { row: StanceRow; onOpenPaper: (id: string) => void }) {
	const { placed } = row;
	return (
		<li className="flex flex-col gap-0.5">
			<RowButton className="flex-col gap-0.5 py-1" title="Open it in the reader" onClick={() => onOpenPaper(row.paper)}>
				<span className="font-sans text-sm leading-snug">
					<MathText>{row.title}</MathText>
				</span>
				<span className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground empty:hidden">
					<AcceptanceTag state={row.tier} small />
					{row.year !== null && <span>{row.year}</span>}
					{row.read === "abstract" && (
						<span title="The library has no PDF of this paper: the model read its title and abstract only. A weaker reading.">
							from the abstract only
						</span>
					)}
				</span>
			</RowButton>
			{placed.group === "mixed" && (
				<p className="px-2 text-xs text-foreground" title="The condition, in the model's words: the quotes are the paper's.">
					Yes when: {placed.condition}
				</p>
			)}
			{"evidence" in placed && <EvidenceLinks className="px-2" evidence={placed.evidence} />}
		</li>
	);
}

function Groups({ result, onOpenPaper }: { result: StanceResult; onOpenPaper: (id: string) => void }) {
	return (
		<div className="flex flex-col gap-3">
			{COUNTED.filter((group) => group !== "not_addressed").map((group) => {
				const rows = rowsOf(result.rows, group);
				if (rows.length === 0) return null;
				return (
					<div key={group}>
						<Heading className="mb-1 flex items-center gap-1.5" title={GROUP_NOTE[group]}>
							<Dot hue={GROUP_HUE[group]} />
							{GROUP_LABEL[group]} · {count(rows.length, "paper")}
						</Heading>
						<ul className="flex flex-col gap-1.5">
							{rows.map((row) => (
								<PlacedPaper key={row.paper} row={row} onOpenPaper={onOpenPaper} />
							))}
						</ul>
					</div>
				);
			})}
			{(["not_addressed", ...APART] as const).map((group) => {
				const rows = rowsOf(result.rows, group);
				if (rows.length === 0) return null;
				return (
					<details key={group}>
						<summary className="cursor-pointer text-xs text-muted-foreground" title={GROUP_NOTE[group]}>
							{GROUP_LABEL[group]} · {count(rows.length, "paper")}
						</summary>
						<ul className="mt-1 flex flex-col">
							{rows.map((row) => (
								<PlacedPaper key={row.paper} row={row} onOpenPaper={onOpenPaper} />
							))}
						</ul>
					</details>
				);
			})}
		</div>
	);
}

/** What is known about one question: coverage, counts, the breakdowns, the groups. */
function ResultView({
	run,
	scope,
	id,
	version,
	busy,
	onAskRest,
	onRedo,
	onOpenPaper,
}: {
	run: string;
	scope: StanceScope;
	id: string;
	version: number;
	/** a job is running: offer nothing that starts another */
	busy: boolean;
	onAskRest: (result: StanceResult) => void;
	onRedo: (result: StanceResult) => void;
	onOpenPaper: (id: string) => void;
}) {
	const loaded = useAsync(() => api.stances.result(run, scope, id), [run, JSON.stringify(scope), id, version]);
	const [confirming, setConfirming] = useState(false);
	if (loaded.status === "failed") return <p className="text-xs text-destructive">{loaded.error}</p>;
	const result = ready(loaded);
	if (!result) return <p className="text-xs text-muted-foreground">Loading…</p>;
	const counts = tally(result.rows);
	const skipped = skippedLine(result.filter);
	const fromAbstract = abstractOnlyLine(result.rows);
	return (
		<div className="flex flex-col gap-2">
			<p className="font-sans text-base text-foreground">{result.question}</p>
			<p className="text-xs text-muted-foreground tabular-nums">
				{coverageLine(result)} {skipped}
			</p>
			<p className="text-xs text-foreground tabular-nums" title={STANCE_NOTE}>
				{countsLine(counts)}
			</p>
			<p className="text-xs text-muted-foreground">
				The model&apos;s reading ({result.model}), not the papers&apos; verdict: read the quotes. {fromAbstract}
			</p>
			{result.rows.length > 0 && (
				<details>
					<summary className="cursor-pointer text-xs text-muted-foreground">By acceptance tier and by year</summary>
					<ul className="mt-1 flex flex-col gap-1 text-xs text-muted-foreground tabular-nums">
						{[...breakdown(result.rows, "tier"), ...breakdown(result.rows, "year")].map((line) => (
							<li key={line.key} title={STANCE_NOTE}>
								<span className="text-foreground">{line.label}</span> ({count(line.papers, "paper")}): {countsLine(line.counts)}
							</li>
						))}
					</ul>
				</details>
			)}
			<Groups result={result} onOpenPaper={onOpenPaper} />
			{/* nothing new is started while a job runs: it is followed until it ends */}
			<div className={cn("flex flex-wrap gap-2", busy && "hidden")}>
				{result.answered < result.to_ask && (
					<Button variant="outline" onClick={() => onAskRest(result)}>
						Ask the other {count(result.to_ask - result.answered, "paper")}…
					</Button>
				)}
				{confirming ? (
					<>
						<Button
							variant="outline"
							onClick={() => {
								setConfirming(false);
								onRedo(result);
							}}
						>
							Redo: ask again and pay again
						</Button>
						<Button variant="ghost" onClick={() => setConfirming(false)}>
							Keep
						</Button>
					</>
				) : (
					<Button
						variant="ghost"
						title="Ask this question again from scratch: these answers are no longer shown, and asking again costs again (the cost is shown first)"
						onClick={() => setConfirming(true)}
					>
						<RotateCcw />
						Redo
					</Button>
				)}
			</div>
		</div>
	);
}

/** The cost of asking, before anything is sent, and the clicks that start it. */
function PlanView({
	plan,
	includeUnrelated,
	onInclude,
	onAsk,
	onCancel,
}: {
	plan: StancePlan;
	includeUnrelated: boolean;
	onInclude: (include: boolean) => void;
	onAsk: (part: "sample" | "all") => void;
	onCancel: () => void;
}) {
	const { currency, locale } = useCurrency();
	const skipped = skippedLine(plan.filter);
	const rest = plan.all.estimate.to_ask;
	const sample = plan.sample.estimate.to_ask;
	return (
		<div className="flex flex-col gap-2 text-xs">
			<p className="font-sans text-base text-foreground">{plan.question}</p>
			{skipped && <p className="text-muted-foreground tabular-nums">{skipped}</p>}
			{plan.filter.unrelated > 0 && (
				<label className="flex items-center gap-2 text-muted-foreground">
					<input type="checkbox" checked={includeUnrelated} onChange={(event) => onInclude(event.target.checked)} />
					Include the {count(plan.filter.unrelated, "skipped paper")} too
				</label>
			)}
			<p className="text-muted-foreground tabular-nums">
				{count(plan.all.papers, "paper")} to ask with {plan.model}: {plan.all.with_pdf} read from their PDF, {plan.all.abstract_only} from
				the abstract only. One call per paper.
			</p>
			<div className="flex flex-col gap-1.5">
				<Button disabled={plan.sample.papers === 0} title={costNote(plan.sample.estimate)} onClick={() => onAsk("sample")}>
					Ask the {count(plan.sample.papers, "most central paper")} first: {costLine(plan.sample.estimate, currency, locale)}
				</Button>
				{plan.all.papers > plan.sample.papers && (
					<Button variant="outline" title={costNote(plan.all.estimate)} onClick={() => onAsk("all")}>
						Ask all {count(plan.all.papers, "paper")}: {costLine(plan.all.estimate, currency, locale)}
					</Button>
				)}
				<Button variant="ghost" onClick={onCancel}>
					Not now
				</Button>
			</div>
			{sample === 0 && rest === 0 && <p className="text-muted-foreground">Every answer is kept: asking costs nothing.</p>}
		</div>
	);
}

/** Questions the model may propose: their cost first, then the job, then each with the quotes that show papers address it. */
function Proposals({
	run,
	community,
	proposals,
	onPick,
	onChanged,
}: {
	run: string;
	community: string;
	proposals: StanceProposals | null;
	onPick: (question: string) => void;
	onChanged: () => void;
}) {
	const [step, setStep] = useState<Proposing>({ kind: "idle" });
	const { currency, locale } = useCurrency();
	const end = useCallback(
		(error: string | null) => {
			setStep(error ? { kind: "failed", error } : { kind: "idle" });
			onChanged();
		},
		[onChanged],
	);
	const progress = useJobProgress(step.kind === "asking" ? step.job : null, end);
	// with proposals shown, proposing again is a fresh call: its cost is the one shown, and the proposals shown stay
	// until the user clicks Propose
	const estimate = () => {
		const fresh = proposals !== null;
		setStep({ kind: "estimating" });
		api.stances.proposalEstimate(run, community, fresh).then(
			(plan) => setStep({ kind: "estimated", plan, fresh }),
			(error: Error) => setStep({ kind: "failed", error: error.message }),
		);
	};
	return (
		<div className="flex flex-col gap-1.5">
			{proposals && proposals.questions.length > 0 && (
				<ul className="flex flex-col gap-1">
					{proposals.questions.map((proposed) => (
						<li key={proposed.statement}>
							<RowButton className="py-1" title="Ask this question (its cost is shown first)" onClick={() => onPick(proposed.statement)}>
								<span className="font-sans text-sm" title="Proposed by the model from the abstracts; the quotes are the papers'.">
									{proposed.statement}
								</span>
							</RowButton>
							<EvidenceLinks className="px-2" evidence={proposed.evidence} />
						</li>
					))}
				</ul>
			)}
			{proposals && proposals.left_out > 0 && (
				<p className="text-xs text-muted-foreground">
					{count(proposals.left_out, "proposed question")} left out: their quotes were not found in the abstracts of two papers.
				</p>
			)}
			{step.kind === "idle" && (
				<Button
					variant="ghost"
					className="self-start"
					title="The model reads the abstracts of the most central papers and proposes questions they take positions on. The cost is shown before anything is sent."
					onClick={estimate}
				>
					<Sparkles />
					{proposals ? "Propose new questions…" : "Propose questions with the model…"}
				</Button>
			)}
			{step.kind === "estimating" && <p className="text-xs text-muted-foreground">Working out the cost…</p>}
			{step.kind === "estimated" && (
				<div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
					<span className="w-full tabular-nums">
						One call over the abstracts of {count(step.plan.papers, "paper")} with {step.plan.model}:{" "}
						{costLine(step.plan.estimate, currency, locale)}
					</span>
					<Button
						title={costNote(step.plan.estimate)}
						onClick={() =>
							api.stances.propose(run, community, step.fresh, { model: step.plan.model, approved_usd: step.plan.estimate.most_usd }).then(
								(started) => setStep({ kind: "asking", job: started.job }),
								(error: Error) => setStep({ kind: "failed", error: error.message }),
							)
						}
					>
						Propose
					</Button>
					<Button variant="ghost" onClick={() => setStep({ kind: "idle" })}>
						Not now
					</Button>
				</div>
			)}
			{step.kind === "asking" && <Running job={step.job} progress={progress} what="call" />}
			{step.kind === "failed" && (
				<p className="text-xs text-destructive">
					{step.error}{" "}
					<button type="button" className="underline" onClick={() => setStep({ kind: "idle" })}>
						Dismiss
					</button>
				</p>
			)}
		</div>
	);
}

/**
 * The section. `scope` is a community (by its tree path) or the marked papers; proposals by the model are offered
 * for communities only.
 */
export function CommunityStances({ run, scope, onOpenPaper }: { run: string; scope: StanceScope; onOpenPaper: (id: string) => void }) {
	const scopeKey = JSON.stringify(scope);
	const listing = useAsync(() => api.stances.listing(run, scope), [run, scopeKey]);
	const { reload } = listing;
	const [question, setQuestion] = useState("");
	const [step, setStep] = useState<Step>({ kind: "choosing" });
	const [open, setOpen] = useState<string | null>(null);
	const [version, setVersion] = useState(0);
	// mounted once per scope (keyed by it where it is shown), so nothing here is left from another community

	const estimate = useCallback(
		(asked: string, includeUnrelated: boolean) => {
			setStep({ kind: "estimating", question: asked, includeUnrelated });
			api.stances.estimate(run, scope, asked, includeUnrelated).then(
				(plan) => setStep({ kind: "estimated", plan, includeUnrelated }),
				(error: Error) => setStep({ kind: "failed", error: error.message }),
			);
		},
		// eslint-disable-next-line react-hooks/exhaustive-deps
		[run, scopeKey],
	);
	const end = useCallback(
		(error: string | null) => {
			setStep(error ? { kind: "failed", error } : { kind: "choosing" });
			setVersion((value) => value + 1);
			reload();
		},
		[reload],
	);
	const progress = useJobProgress(step.kind === "asking" ? step.job : null, end);

	const found = ready(listing);
	if (!found) return null; // not loaded yet, or an engine without these routes: nothing to show
	return (
		<section className="flex flex-col gap-3 px-4 pb-4">
			<Heading title={STANCE_NOTE}>Agree / disagree</Heading>
			<form
				className="flex gap-2"
				onSubmit={(event) => {
					event.preventDefault();
					if (question.trim()) estimate(question, false);
				}}
			>
				<Input
					value={question}
					maxLength={300}
					placeholder="A yes/no question, e.g. Does pre-training on synthetic data help?"
					onChange={(event) => setQuestion(event.target.value)}
				/>
				<Button
					type="submit"
					variant="outline"
					disabled={question.trim().length < 10 || step.kind === "asking"}
					title="See what asking would cost; nothing is sent yet"
				>
					Cost…
				</Button>
			</form>
			{found.suggestions.length > 0 && step.kind === "choosing" && (
				<div className="flex flex-wrap gap-1">
					{found.suggestions.map((suggestion) => (
						<button
							type="button"
							key={suggestion.term}
							className="rounded-sm border px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-accent"
							title={`Used by ${count(suggestion.papers, "paper")} here, ${suggestion.lift.toFixed(1)} times as often as across the run. Counted from titles and abstracts, without a model. Click to start a question from it.`}
							onClick={() => setQuestion(starter(suggestion.term))}
						>
							{suggestion.term} · {count(suggestion.papers, "paper")}
						</button>
					))}
				</div>
			)}
			{"community" in scope && step.kind === "choosing" && (
				<Proposals run={run} community={scope.community} proposals={found.proposals} onPick={setQuestion} onChanged={reload} />
			)}
			{step.kind === "estimating" && <p className="text-xs text-muted-foreground">Working out which papers to ask and the cost…</p>}
			{step.kind === "estimated" && (
				<PlanView
					plan={step.plan}
					includeUnrelated={step.includeUnrelated}
					onInclude={(include) => estimate(step.plan.question, include)}
					onCancel={() => setStep({ kind: "choosing" })}
					onAsk={(part) =>
						api.stances
							.ask(run, scope, step.plan.question, step.includeUnrelated, part, {
								model: step.plan.model,
								approved_usd: (part === "sample" ? step.plan.sample : step.plan.all).estimate.most_usd,
							})
							.then(
								(started) => {
									setOpen(started.id);
									setStep({ kind: "asking", job: started.job, id: started.id });
								},
								(error: Error) => setStep({ kind: "failed", error: error.message }),
							)
					}
				/>
			)}
			{step.kind === "asking" && (
				<Running job={step.job} progress={progress} what="paper" onDone={() => setVersion((value) => value + 1)} />
			)}
			{step.kind === "failed" && (
				<p className="text-xs text-destructive">
					{step.error}{" "}
					<button type="button" className="underline" onClick={() => setStep({ kind: "choosing" })}>
						Dismiss
					</button>
				</p>
			)}
			{found.questions.length > 0 && (
				<div>
					<Heading className="mb-1">Asked here</Heading>
					{found.questions.map((asked) => (
						<RowButton
							key={asked.id}
							active={open === asked.id}
							className={cn("py-1 text-sm")}
							title={`Asked ${asked.asked_at.slice(0, 10)}. Open its answers: nothing is sent.`}
							onClick={() => setOpen(open === asked.id ? null : asked.id)}
						>
							{asked.question}
						</RowButton>
					))}
				</div>
			)}
			{open && (
				<ResultView
					key={open}
					run={run}
					scope={scope}
					id={open}
					version={version}
					busy={step.kind === "asking"}
					onOpenPaper={onOpenPaper}
					onAskRest={(result) => estimate(result.question, result.filter.include_unrelated)}
					onRedo={(result) =>
						api.stances.redo(run, scope, result.id).then(
							() => {
								setVersion((value) => value + 1);
								estimate(result.question, result.filter.include_unrelated);
							},
							(error: Error) => setStep({ kind: "failed", error: error.message }),
						)
					}
				/>
			)}
		</section>
	);
}
