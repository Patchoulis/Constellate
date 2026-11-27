"use client";

/**
 * A community's citation kinds (engine analysis/citation_kinds.rs): which papers its papers use as a baseline,
 * contrast, take data or benchmarks from, or build on, each figure a count of citing sentences out of all those
 * citing that paper, and each number opening the sentences behind it. The kinds are the model's reading: asked for
 * only after the estimate is shown, on a click, a sample first and the rest on a second click, within a budget.
 * Nothing is shown when the engine has no such route (the Python backend).
 */
import { useCallback, useState } from "react";
import { BookOpenText, RotateCcw, Sparkles } from "lucide-react";
import type { CitationKind, CitationKindsEstimate, CitationKindsScope, RankedCitation } from "@rp/contracts";
import { JobStrip, SentenceRow, USD, contextsOf, forgetAsked, useFollowedJob } from "@/components/citation-contexts";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { api } from "@/lib/api";
import {
	KINDS_NOTE,
	KIND_LABEL,
	SENTENCES_NOTE,
	budgetFor,
	moreToRead,
	sentencesOfKind,
	tallyLine,
	tokensPerSentence,
} from "@/lib/citation-kinds";
import { count, money, tokens } from "@/lib/units";
import { ready, useAsync } from "@/lib/use-async";

/** The sentences behind one count: the community's citing sentences of `paper` read as `kinds`. */
function Behind({ run, treePath, paper, kinds }: { run: string; treePath: string; paper: string; kinds: CitationKind[] }) {
	const contexts = useAsync(() => contextsOf(paper, run, treePath), [paper, run, treePath]);
	const found = ready(contexts);
	if (contexts.status === "loading") return <p className="ml-4 font-ui text-xs text-muted-foreground">Loading the sentences…</p>;
	if (!found) return null;
	const sentences = found.cited_by.flatMap((citing) => kinds.flatMap((kind) => sentencesOfKind(citing.sentences, kind)));
	return (
		<ul className="ml-4 flex flex-col gap-1.5">
			{sentences.map((sentence) => (
				<SentenceRow key={`${sentence.evidence.paper}:${JSON.stringify(sentence.evidence.established)}`} sentence={sentence} />
			))}
		</ul>
	);
}

function Ranked({
	title,
	ranked,
	kinds,
	titles,
	run,
	treePath,
	onOpenPaper,
}: {
	title: string;
	ranked: RankedCitation[];
	kinds: CitationKind[];
	titles: Record<string, string>;
	run: string;
	treePath: string;
	onOpenPaper: (id: string) => void;
}) {
	const [open, setOpen] = useState<string | null>(null);
	if (ranked.length === 0) return null;
	const what = kinds.map((kind) => KIND_LABEL[kind]).join(" or ");
	return (
		<div>
			<Heading className="mb-1.5">{title}</Heading>
			<ol className="flex flex-col">
				{ranked.map((paper) => (
					<li key={paper.paper} className="flex flex-col">
						<div className="flex items-baseline gap-2">
							<RowButton className="flex-1" onClick={() => onOpenPaper(paper.paper)} title="Open this paper in the reader">
								<span className="font-sans text-base leading-snug">
									<MathText>{titles[paper.paper] ?? paper.paper}</MathText>
								</span>
							</RowButton>
							<button
								type="button"
								className="shrink-0 font-ui text-xs text-muted-foreground tabular-nums hover:text-foreground"
								title={`${tallyLine(paper.tally)}. Click to show the sentences read as “${what}”. ${KINDS_NOTE}`}
								onClick={() => setOpen(open === paper.paper ? null : paper.paper)}
							>
								{paper.count} of {count(paper.tally.sentences, "citing sentence")}
							</button>
						</div>
						{open === paper.paper && <Behind run={run} treePath={treePath} paper={paper.paper} kinds={kinds} />}
					</li>
				))}
			</ol>
		</div>
	);
}

/** What asking would cost, said before anything is sent, and the button that asks. */
function Asking({
	estimate,
	onStart,
	busy,
}: {
	estimate: CitationKindsEstimate;
	onStart: (scope: CitationKindsScope, budget: number) => void;
	busy: boolean;
}) {
	const cap = budgetFor(estimate);
	const perSentence = tokensPerSentence(estimate);
	const what = estimate.scope === "sample" ? "the citations of the 20 papers it cites most" : "the rest of its citations";
	if (estimate.to_ask === 0)
		return (
			<p className="font-ui text-xs text-muted-foreground">
				All {count(estimate.sentences, "citing sentence")} of {what} have been read.
			</p>
		);
	return (
		<div className="flex flex-col items-start gap-1.5 rounded-lg border p-3 font-ui text-xs text-muted-foreground">
			<p
				title={`An estimate: prompt tokens are reckoned from characters (${estimate.estimate.by_model[0]?.counting.chars_per_token ?? 3} per token), so it errs high. The most is if every answer were as long as allowed.`}
			>
				Reading {what}: {count(estimate.to_ask, "citing sentence")} to ask the model ({estimate.model}) about
				{estimate.kept > 0 && `, ${estimate.kept.toLocaleString()} read before`}, in {count(estimate.estimate.to_ask, "call")}
				{perSentence !== null && `, about ${tokens(perSentence)} per sentence`}. Estimated cost:{" "}
				{estimate.estimate.usd === null ? "unknown (the model's price is not known)" : `about ${money(estimate.estimate.usd, USD)}`}
				{estimate.estimate.most_usd !== null && `, at most ${money(estimate.estimate.most_usd, USD)}`}. This is an estimate.
			</p>
			{estimate.can_ask ? (
				cap !== null && (
					<Button
						variant="outline"
						disabled={busy}
						onClick={() => onStart(estimate.scope, cap)}
						title="Starts a job you can follow and stop; it never spends more than the cap"
					>
						<Sparkles />
						{estimate.scope === "sample" ? "Read a sample" : "Read the rest"}, at most {money(cap, USD)}
					</Button>
				)
			) : (
				<p>The model cannot be asked: no key for it is set in the engine&apos;s environment.</p>
			)}
		</div>
	);
}

/** A community's Citation kinds section. */
export function CommunityCitationKinds({
	run,
	treePath,
	onOpenPaper,
}: {
	run: string;
	treePath: string;
	onOpenPaper: (id: string) => void;
}) {
	const [version, setVersion] = useState(0);
	const summary = ready(useAsync(() => api.citationKinds(run, treePath), [run, treePath, version]));
	const reload = useCallback(() => {
		forgetAsked();
		setVersion((value) => value + 1);
	}, []);
	const { job, start } = useFollowedJob(reload);
	const sampleRead = summary ? summary.sample.length > 0 && summary.tally.not_classified < summary.tally.sentences : false;
	const scope: CitationKindsScope = sampleRead ? "rest" : "sample";
	const estimate = ready(
		useAsync(summary ? () => api.citationKindsEstimate(run, scope, treePath) : null, [run, treePath, scope, version, !!summary]),
	);
	const [forgetting, setForgetting] = useState(false);
	if (!summary) return null;
	const coverage = { papers: summary.papers, read: summary.papers_read, without_pdf: summary.without_pdf };
	const lists = { run, treePath, titles: summary.titles, onOpenPaper };
	const running = job.state === "running";
	return (
		<section className="flex flex-col gap-3 px-4 pb-4">
			<Heading>Citation kinds</Heading>
			<p className="text-xs text-muted-foreground" title={SENTENCES_NOTE}>
				Citing sentences read for {summary.papers_read.toLocaleString()} of {count(summary.papers, "paper")}
				{summary.without_pdf > 0 && ` (${count(summary.without_pdf, "has", "have")} no PDF kept)`}: they cite{" "}
				{count(summary.cited_papers, "paper")} of your library in {count(summary.tally.sentences, "sentence")}.
			</p>
			{moreToRead(coverage) && !running && (
				<Button
					variant="outline"
					className="self-start"
					title="Reads the reference list and citing sentences of each of the run's papers whose PDF is kept, once. No model is asked: it costs nothing."
					onClick={() => void start(() => api.readCitationContexts(run))}
				>
					<BookOpenText />
					Read the citing sentences of this run&apos;s papers
				</Button>
			)}
			{summary.tally.sentences > 0 && summary.tally.not_classified < summary.tally.sentences && (
				<p className="text-xs text-muted-foreground" title={KINDS_NOTE}>
					Its papers&apos; citations of your library, as the model read them: {tallyLine(summary.tally, "cited")}.
				</p>
			)}
			<Ranked title="Most used as a baseline" ranked={summary.baseline} kinds={["baseline"]} {...lists} />
			<Ranked title="Data and benchmarks used" ranked={summary.data} kinds={["uses_data"]} {...lists} />
			<Ranked title="Methods used or extended" ranked={summary.used} kinds={["uses", "extends"]} {...lists} />
			<Ranked title="Most contrasted" ranked={summary.contrasts} kinds={["contrasts"]} {...lists} />
			<Ranked title="Most supported" ranked={summary.supports} kinds={["supports"]} {...lists} />
			{estimate && summary.tally.sentences > 0 && (
				<Asking
					estimate={estimate}
					busy={running}
					onStart={(chosen, budget) =>
						void start(() => api.classifyCitations(run, { community: treePath, scope: chosen, budget_usd: budget }))
					}
				/>
			)}
			<JobStrip job={job} what="Reading" />
			{summary.tally.not_classified < summary.tally.sentences && !running && (
				<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
					{forgetting ? (
						<>
							<span>Forget the kinds read for the papers this community cites? Reading them again costs again.</span>
							<Button
								variant="outline"
								onClick={() => {
									setForgetting(false);
									void api.forgetCitationKinds(run, treePath).then(reload);
								}}
							>
								Forget them
							</Button>
							<Button variant="ghost" onClick={() => setForgetting(false)}>
								Keep them
							</Button>
						</>
					) : (
						<Button variant="ghost" title="Forget these readings, to have them read again" onClick={() => setForgetting(true)}>
							<RotateCcw />
							Read again
						</Button>
					)}
				</div>
			)}
		</section>
	);
}
