"use client";

/**
 * Ask a run or a community a question (engine api/answers.rs). The passages that match are found first, with no
 * model and at no cost, and shown with their citations; the model is asked only when the user clicks the button
 * that shows what it would cost. Its answer is a list of statements, each followed by the papers' words it rests
 * on; statements whose quotes were not found in the papers are left out and counted.
 */
import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import { Loader2, MessageCircleQuestion, RotateCcw, Search, Square, Trash2, X } from "lucide-react";
import type { AnswerView, PassageFound } from "@rp/contracts";
import { EvidenceLink, EvidenceLinks } from "@/components/evidence-link";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/api";
import { accepting, shownModel } from "@/lib/cost";
import {
	cannotAsk,
	costLine,
	costTitle,
	coverageLine,
	excerpt,
	historyOf,
	indexingLine,
	leftOutLine,
	MODEL_NOTE,
	reduceAsk,
	splitPassages,
	stateNote,
} from "@/lib/answers";
import { choiceKey, isFor, useChosenModel } from "@/lib/chosen-model";
import { ready, useAsync } from "@/lib/use-async";
import { elsewhereLine, useFollowedJob } from "@/lib/use-followed-job";
import { useResizable } from "@/lib/use-resizable";
import { count } from "@/lib/units";

/** How often the search is asked again while the papers are indexed (no model, no cost). */
const POLL_MS = 1500;

/** One passage: the paper's words there, and its citation (which opens the reader at the place). */
function Passage({ found }: { found: PassageFound }) {
	return (
		<li className="flex flex-col gap-0.5 border-l pl-2">
			<span className="font-sans text-sm text-foreground/90">&ldquo;{excerpt(found.evidence.quote)}&rdquo;</span>
			<EvidenceLink evidence={found.evidence} />
		</li>
	);
}

function Passages({ passages }: { passages: PassageFound[] }) {
	return (
		<ul className="flex flex-col gap-2">
			{passages.map((found, index) => (
				<Passage key={`${found.evidence.paper}-${index}`} found={found} />
			))}
		</ul>
	);
}

/** A checked answer: its statements with their evidence, what was left out, and the passages it was given. */
function Answer({ view, onCitedPapers }: { view: AnswerView; onCitedPapers?: (papers: string[]) => void }) {
	const { cited, unused } = splitPassages(view.passages);
	const leftOut = leftOutLine(view.counts);
	const note = stateNote(view);
	useEffect(() => onCitedPapers?.(view.cited_papers), [view.cited_papers, onCitedPapers]);
	return (
		<div className="flex flex-col gap-3">
			<p className="font-sans text-base font-semibold text-foreground">{view.question}</p>
			{note && (
				<p className={view.state === "not_in_papers" ? "font-sans text-base text-foreground" : "text-sm text-muted-foreground"}>{note}</p>
			)}
			{view.state === "answered" && view.statements.length > 0 && (
				<ul className="flex flex-col gap-2.5">
					{view.statements.map((cited, index) => (
						<li key={`${index}-${cited.statement.sentence}`} className="font-sans text-base leading-relaxed">
							<span title={MODEL_NOTE}>{cited.statement.sentence}</span>{" "}
							{cited.statement.about === "several_papers" && (
								<span
									className="font-ui text-xs text-muted-foreground"
									title="Said of several papers: shown only because quotes of at least two of them were found."
								>
									(several papers){" "}
								</span>
							)}
							<EvidenceLinks evidence={cited.evidence} />
						</li>
					))}
				</ul>
			)}
			{view.state === "answered" && (
				<p className="text-xs text-muted-foreground" title={MODEL_NOTE}>
					Written by the model ({view.model}) from {count(view.passages.length, "passage")}; {count(view.cited_papers.length, "paper")}{" "}
					cited in this answer.{leftOut && ` ${leftOut}.`}
				</p>
			)}
			{view.state === "unverified" && leftOut && <p className="text-xs text-muted-foreground">{leftOut}.</p>}
			{view.state === "answered" && cited.length > 0 && (
				<div>
					<Heading className="mb-1.5">Passages it cites</Heading>
					<Passages passages={cited} />
				</div>
			)}
			{(view.state === "answered" ? unused : view.passages).length > 0 && (
				<details open={view.state !== "answered"}>
					<summary className="cursor-pointer text-xs tracking-widest text-muted-foreground uppercase">
						{view.state === "answered"
							? `Found but not used (${count(unused.length, "passage")})`
							: `Passages found (${count(view.passages.length, "passage")})`}
					</summary>
					<div className="mt-1.5">
						<Passages passages={view.state === "answered" ? unused : view.passages} />
					</div>
				</details>
			)}
		</div>
	);
}

/**
 * The Ask box of a run (`community` null) or of one community. `onCitedPapers` is told the papers an answer
 * shown cites, for whoever wants to light them up on the graph.
 */
export function AskBox({
	run,
	community,
	papers,
	onCitedPapers,
}: {
	run: string;
	community: string | null;
	/** how many papers the scope has, for the prompt */
	papers: number;
	onCitedPapers?: (papers: string[]) => void;
}) {
	const [state, dispatch] = useReducer(reduceAsk, { kind: "idle" });
	const [text, setText] = useState("");
	const asked = useAsync(() => api.questions(run), [run], { keep: false });
	const history = useMemo(() => historyOf(ready(asked) ?? [], community), [asked, community]);
	const chosen = useChosenModel();
	const choice = choiceKey(chosen);

	const find = useCallback(
		(question: string) => {
			dispatch({ type: "asked", question });
			Promise.all([api.passagesFound(run, question, community), api.answerEstimate(run, question, community)]).then(
				([found, estimate]) => dispatch({ type: "searched", found, estimate }),
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			);
		},
		[run, community],
	);

	/** A question asked before, put back in the box and found again. */
	const again = (question: string) => {
		setText(question);
		find(question);
	};

	const open = useCallback(
		(id: string) => {
			dispatch({ type: "opened", id });
			api.answer(run, id).then(
				(view) => dispatch({ type: "viewed", view }),
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			);
		},
		[run],
	);

	// The model is asked as the engine's one job, followed by the page's one poller (also when the box was closed
	// and opened again meanwhile); when it ends, its answer, or what stopped it, is shown.
	const { job, elsewhere, start } = useFollowedJob(
		(ended) => {
			asked.reload();
			if (ended.about) open(ended.about);
		},
		`answers:${run}:${community ?? ""}`,
	);

	// while the papers are being indexed, the search is asked again
	useEffect(() => {
		if (state.kind !== "indexing") return;
		const timer = setTimeout(() => find(state.question), POLL_MS);
		return () => clearTimeout(timer);
	}, [state, find]);

	// another model chosen while a cost is shown: the cost of asking that one is worked out, the old one goes
	const shown = state.kind === "found" ? state.question : null;
	useEffect(() => {
		if (shown !== null) find(shown);
		// only a new choice asks again; `shown` and `find` are read as they are then
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [choice]);

	// an answer opened while another window asks it: looked at again once that job is over, not polled
	const waiting = state.kind === "answer" && state.view.state === "asking" ? state.view.id : null;
	useEffect(() => {
		if (waiting !== null && elsewhere === null && job.state !== "running") open(waiting);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [elsewhere]);

	const askModel = () => {
		if (state.kind !== "found") return;
		const { question, estimate } = state;
		// one click, one request: a second click while it is sent or runs does nothing
		void start(async () => {
			const started = await api.askQuestion(run, { id: estimate.id, question, community, ...shownModel(estimate), ...accepting(estimate) });
			asked.reload();
			if (started.job === null) {
				open(started.id); // kept: nothing was asked
				return null;
			}
			dispatch({ type: "started", id: started.id, job: started.job });
			return { id: started.job, about: started.id };
		});
	};
	const running = job.state === "starting" || job.state === "running";
	const busy = state.kind === "searching" || state.kind === "opening" || running;
	// the cost shown is for the question in the box and the model chosen now, or it is not accepted
	const stale =
		state.kind === "found" && (text.trim() !== state.question || !isFor(state.estimate, chosen))
			? text.trim() !== state.question
				? "The question was changed: press Find to see what asking it would cost."
				: "Working out the cost for the model chosen now…"
			: null;
	// why the last ask did not answer: beside the cost it was refused at, or beside the answer it left unanswered
	const refused = job.state === "failed" && job.id === null && state.kind === "found" ? `Not asked: ${job.error}` : null;
	const jobError = job.state === "failed" && state.kind === "answer" && state.view.id === job.about ? job.error : null;

	return (
		<div className="flex flex-col gap-3">
			<form
				className="flex gap-1.5"
				onSubmit={(event) => {
					event.preventDefault();
					if (text.trim()) find(text.trim());
				}}
			>
				<Input
					value={text}
					maxLength={1000}
					onChange={(event) => setText(event.target.value)}
					placeholder={`Ask a question of ${count(papers, "paper")}`}
				/>
				<Button
					type="submit"
					variant="outline"
					disabled={!text.trim() || busy}
					title="Find the passages that talk about it: no model, no cost"
				>
					{busy ? <Loader2 className="animate-spin" /> : <Search />}
					Find
				</Button>
			</form>

			{state.kind === "indexing" && (
				<p className="flex items-center gap-2 text-xs text-muted-foreground">
					<Loader2 className="size-3 animate-spin" />
					{indexingLine(state.done, state.of)}
				</p>
			)}
			{state.kind === "failed" && <p className="text-sm text-destructive">{state.error}</p>}
			{asked.status === "failed" && <p className="text-sm text-destructive">The questions asked before could not be read: {asked.error}</p>}

			{state.kind === "found" && (
				<div className="flex flex-col gap-3">
					<p
						className="text-xs text-muted-foreground"
						title="Found by the words they share with the question, with no model: nothing was sent anywhere."
					>
						{coverageLine(state.found.coverage)}. {count(state.found.passages.length, "passage")} found.
					</p>
					{state.found.passages.length > 0 && (
						<div>
							<Heading className="mb-1.5">Passages found</Heading>
							<Passages passages={state.found.passages} />
						</div>
					)}
					{cannotAsk(state.estimate) ? (
						<p className="text-xs text-muted-foreground">{cannotAsk(state.estimate)}</p>
					) : stale ? (
						<p className="text-xs text-muted-foreground">{stale}</p>
					) : (
						<Button className="self-start" title={costTitle(state.estimate)} disabled={running || elsewhere !== null} onClick={askModel}>
							<MessageCircleQuestion />
							{costLine(state.estimate)}
						</Button>
					)}
					{elsewhere && <p className="text-xs text-muted-foreground">{elsewhereLine(elsewhere)}</p>}
					{refused && <p className="text-xs text-destructive">{refused}</p>}
					<p className="text-xs text-muted-foreground">
						The model is asked to answer only from these passages and to quote them; statements whose quotes are not found in the papers are
						left out. The answer is kept: opening it again costs nothing.
					</p>
				</div>
			)}

			{job.state === "running" && (
				<div className="flex items-center gap-2 text-sm text-muted-foreground">
					<Loader2 className="size-3.5 animate-spin" />
					The model is being asked: one call.
					{job.unanswered && ` (${job.unanswered}: looking again.)`}
					<Button
						variant="ghost"
						size="icon"
						title="Stop: nothing more is sent"
						onClick={() => void api.cancelJob(job.id).catch(() => undefined)}
					>
						<Square />
					</Button>
				</div>
			)}

			{state.kind === "answer" && (
				<>
					<Answer view={state.view} onCitedPapers={onCitedPapers} />
					{jobError && <p className="text-sm text-destructive">{jobError}</p>}
					{state.view.state === "unanswered" && (
						<Button variant="outline" className="self-start" onClick={() => again(state.view.question)}>
							<RotateCcw />
							Show the cost of asking again
						</Button>
					)}
				</>
			)}

			{history.length > 0 && (
				<details>
					<summary className="cursor-pointer text-xs tracking-widest text-muted-foreground uppercase">
						Asked before ({count(history.length, "question")})
					</summary>
					<ul className="mt-1.5 flex flex-col">
						{history.map((question) => (
							<li key={question.id} className="flex items-center gap-1">
								<RowButton
									className="flex-1 flex-col gap-0.5"
									title={question.answered ? "Open the kept answer: no cost" : "Not answered: open to see the cost of asking"}
									onClick={() => (question.answered ? open(question.id) : again(question.question))}
								>
									<span className="font-sans text-sm text-foreground">{question.question}</span>
									<span className="text-xs text-muted-foreground">
										{question.answered ? "answered" : "not answered"} · {question.model} · {question.asked_at.slice(0, 10)}
									</span>
								</RowButton>
								<Button
									variant="ghost"
									size="icon"
									disabled={running}
									title="Ask again: the kept answer is cleared (it asks first), and the cost is shown before anything is sent"
									onClick={() =>
										(!question.answered ||
											window.confirm(
												`Clear the kept answer to “${question.question}”?\n\nIt cannot be undone, and asking the model again costs again. The cost is shown before anything is sent.`,
											)) &&
										api.redoAnswer(run, question.id).then(
											() => {
												asked.reload();
												again(question.question);
											},
											(error: Error) => dispatch({ type: "failed", error: error.message }),
										)
									}
								>
									<RotateCcw />
								</Button>
								<Button
									variant="ghost"
									size="icon"
									disabled={running}
									title="Delete this question and its answer (it asks first)"
									onClick={() =>
										window.confirm(
											`Delete the question “${question.question}”${question.answered ? " and its kept answer" : ""}?\n\nIt cannot be undone.`,
										) &&
										api.forgetQuestion(run, question.id).then(
											() => {
												asked.reload();
												if (state.kind === "answer" && state.view.id === question.id) dispatch({ type: "reset" });
											},
											(error: Error) => dispatch({ type: "failed", error: error.message }),
										)
									}
								>
									<Trash2 />
								</Button>
							</li>
						))}
					</ul>
				</details>
			)}
		</div>
	);
}

/** A community's Ask section, for the community panel. */
export function CommunityAsk({ run, treePath, papers }: { run: string; treePath: string; papers: number }) {
	return (
		<section className="flex flex-col gap-2 px-4 pb-4">
			<Heading>Ask</Heading>
			<AskBox key={treePath} run={run} community={treePath} papers={papers} />
		</section>
	);
}

/** Ask the whole run: a button over the graph that opens the Ask box in a panel. */
export function RunAsk({ run, papers, onCitedPapers }: { run: string; papers: number; onCitedPapers?: (papers: string[]) => void }) {
	const [open, setOpen] = useState(false);
	const { ref, style, handle } = useResizable<HTMLDivElement>("overview");
	if (!open)
		return (
			<Button
				variant="outline"
				className="absolute right-3 bottom-14 z-10 gap-1.5 bg-card"
				title="Ask a question of the whole run"
				onClick={() => setOpen(true)}
			>
				<MessageCircleQuestion />
				Ask the run
			</Button>
		);
	return (
		<div
			ref={ref}
			style={style}
			className="enter-from-right absolute inset-y-0 right-0 z-20 flex w-[min(var(--panel-size),100%)] flex-col border-l bg-card shadow-2xl"
		>
			<ResizeHandle {...handle} />
			<div className="flex h-11 shrink-0 items-center gap-2 border-b px-4">
				<MessageCircleQuestion className="size-3.5 text-muted-foreground" />
				<span className="min-w-0 flex-1 truncate text-sm font-semibold">Ask the run</span>
				<Button variant="ghost" size="icon" title="Close" onClick={() => setOpen(false)}>
					<X />
				</Button>
			</div>
			<div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
				<AskBox run={run} community={null} papers={papers} onCitedPapers={onCitedPapers} />
			</div>
		</div>
	);
}
