"use client";

/**
 * Ask a run or a community a question (engine api/answers.rs). The passages that match are found first, with no
 * model and at no cost, and shown with their citations; the model is asked only when the user clicks the button
 * that shows what it would cost. Its answer is a list of statements, each followed by the papers' words it rests
 * on; statements whose quotes were not found in the papers are left out and counted. Nothing is shown when the
 * engine has no such routes (the Python backend).
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
	missingRoute,
	MODEL_NOTE,
	reduceAsk,
	splitPassages,
	stateNote,
} from "@/lib/answers";
import { ready, useAsync } from "@/lib/use-async";
import { useResizable } from "@/lib/use-resizable";
import { count } from "@/lib/units";

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
	const [jobError, setJobError] = useState<string | null>(null);
	const [posting, setPosting] = useState(false);
	const [version, setVersion] = useState(0);
	const asked = useAsync(() => api.questions(run), [run, version]);
	const history = useMemo(() => historyOf(ready(asked) ?? [], community), [asked, community]);

	const find = useCallback(
		(question: string) => {
			setJobError(null);
			dispatch({ type: "asked", question });
			Promise.all([api.passagesFound(run, question, community), api.answerEstimate(run, question, community)]).then(
				([found, estimate]) => dispatch({ type: "searched", found, estimate }),
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			);
		},
		[run, community],
	);

	// while the papers are being indexed, the search is asked again
	useEffect(() => {
		if (state.kind !== "indexing") return;
		const timer = setTimeout(() => find(state.question), POLL_MS);
		return () => clearTimeout(timer);
	}, [state, find]);

	// while the model is being asked, the answer is looked at until it is there (or the job ended without one)
	// (also when an answer opened from the history is still being asked)
	useEffect(() => {
		const waiting = state.kind === "asking" ? state.id : state.kind === "answer" && state.view.state === "asking" ? state.view.id : null;
		if (waiting === null) return;
		const job = state.kind === "asking" ? state.job : null;
		const timer = setInterval(() => {
			api.answer(run, waiting).then(
				(view) => {
					dispatch({ type: "viewed", view });
					if (view.state !== "asking") setVersion((value) => value + 1);
					if (view.state === "unanswered" && job !== null)
						api.status(Number.MAX_SAFE_INTEGER).then(
							(status) => setJobError(status.id === job ? status.error : null),
							() => {},
						);
				},
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			);
		}, POLL_MS);
		return () => clearInterval(timer);
	}, [state, run]);

	const open = useCallback(
		(id: string) => {
			setJobError(null);
			dispatch({ type: "opened", id });
			api.answer(run, id).then(
				(view) => dispatch({ type: "viewed", view }),
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			);
		},
		[run],
	);

	if (asked.status === "failed" && missingRoute(asked.error)) return null; // an engine without these routes
	const askModel = () => {
		if (state.kind !== "found" || posting) return;
		const estimate = state.estimate;
		setPosting(true); // one click, one request
		api
			.askQuestion(run, { id: estimate.id, question: state.question, community, ...shownModel(estimate), ...accepting(estimate) })
			.then(
				(started) => {
					setVersion((value) => value + 1);
					if (started.job === null) open(started.id);
					else dispatch({ type: "started", id: started.id, job: started.job });
				},
				(error: Error) => dispatch({ type: "failed", error: error.message }),
			)
			.finally(() => setPosting(false));
	};
	const busy = state.kind === "searching" || state.kind === "opening" || state.kind === "asking";

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
					) : (
						<Button className="self-start" title={costTitle(state.estimate)} disabled={posting} onClick={askModel}>
							<MessageCircleQuestion />
							{costLine(state.estimate)}
						</Button>
					)}
					<p className="text-xs text-muted-foreground">
						The model is asked to answer only from these passages and to quote them; statements whose quotes are not found in the papers are
						left out. The answer is kept: opening it again costs nothing.
					</p>
				</div>
			)}

			{state.kind === "asking" && (
				<div className="flex items-center gap-2 text-sm text-muted-foreground">
					<Loader2 className="size-3.5 animate-spin" />
					The model is being asked: one call.
					<Button variant="ghost" size="icon" title="Stop: nothing more is sent" onClick={() => void api.cancelJob(state.job)}>
						<Square />
					</Button>
				</div>
			)}

			{state.kind === "answer" && (
				<>
					<Answer view={state.view} onCitedPapers={onCitedPapers} />
					{jobError && <p className="text-sm text-destructive">{jobError}</p>}
					{state.view.state === "unanswered" && (
						<Button variant="outline" className="self-start" onClick={() => find(state.view.question)}>
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
									onClick={() => (question.answered ? open(question.id) : find(question.question))}
								>
									<span className="font-sans text-sm text-foreground">{question.question}</span>
									<span className="text-xs text-muted-foreground">
										{question.answered ? "answered" : "not answered"} · {question.model} · {question.asked_at.slice(0, 10)}
									</span>
								</RowButton>
								<Button
									variant="ghost"
									size="icon"
									title="Ask again: the kept answer is cleared, and the cost is shown before anything is sent"
									onClick={() =>
										api.redoAnswer(run, question.id).then(
											() => {
												setVersion((value) => value + 1);
												find(question.question);
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
									title="Delete this question and its answer"
									onClick={() =>
										api.forgetQuestion(run, question.id).then(
											() => {
												setVersion((value) => value + 1);
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
	const probe = useAsync(() => api.questions(run), [run]);
	if (probe.status === "failed" && missingRoute(probe.error)) return null;
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
	const probe = useAsync(() => api.questions(run), [run]);
	const { ref, style, handle } = useResizable<HTMLDivElement>("overview");
	if (probe.status === "failed" && missingRoute(probe.error)) return null;
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
