"use client";

/**
 * Citing sentences (engine analysis/citation_contexts.rs): in the reader's Citations tab, for each paper of the
 * library this one cites and each paper citing it, the sentences where it happens, each a link into the citing
 * paper's text, with the sentence before and after, the section, and whether it is cited alone. Beside each, when
 * the model was asked, what kind of citation it is, with the words that show it (components/citation-kinds.tsx asks).
 * Nothing is shown when the engine has no such route (the Python backend).
 */
import { useCallback, useEffect, useState } from "react";
import { BookOpenText, Loader2, Square } from "lucide-react";
import type { CitationContextsOf, CitationKind, CitingSentenceShown, PaperCitationContexts, Status } from "@rp/contracts";
import { EvidenceLink } from "@/components/evidence-link";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton } from "@/components/ui/button";
import { api } from "@/lib/api";
import {
	KIND_HUE,
	KIND_LABEL,
	KINDS,
	KINDS_NOTE,
	SENTENCES_NOTE,
	classified,
	coverageLine,
	groupingNote,
	judgementLabel,
	moreToRead,
	sectionName,
	sentencesOfKind,
	tallyLine,
} from "@/lib/citation-kinds";
import { count, money } from "@/lib/units";
import { ready, useAsync } from "@/lib/use-async";
import { cn, hsl } from "@/lib/utils";

const TAG = "inline-flex items-center rounded-sm border px-1.5 py-px font-ui text-xs";
const SHOWN = 3; // sentences shown per paper before "show all"
const POLL_MS = 1200;
export const USD = { code: "USD", rate: 1, date: null };

/** A paper's contexts asked once per paper and run (the reader and the network's picture share them), until a
 *  job read more (`forgetAsked`). */
const asked = new Map<string, Promise<PaperCitationContexts>>();
export function contextsOf(paper: string, run?: string, treePath?: string, fresh = false): Promise<PaperCitationContexts> {
	const key = `${paper}\n${run ?? ""}\n${treePath ?? ""}`;
	let found = asked.get(key);
	if (!found || fresh) {
		found = api.citationContexts(paper, run, treePath);
		found.catch(() => asked.delete(key));
		asked.set(key, found);
	}
	return found;
}

/** Ask again for every paper's contexts: a job has read more. */
export function forgetAsked() {
	asked.clear();
}

/** The kind a sentence was read as, as a tag, with what it rests on on hover. */
export function KindTag({ sentence }: { sentence: CitingSentenceShown }) {
	const { label, note, kind } = judgementLabel(sentence.judgement);
	const hue = kind ? KIND_HUE[kind] : undefined;
	return (
		<span
			title={note}
			className={cn(
				TAG,
				hue === undefined && "border-border text-muted-foreground",
				sentence.judgement.state === "not_justified" && "border-dashed",
			)}
			style={hue === undefined ? undefined : { borderColor: hsl(hue, 60, 0.6), color: hsl(hue, 72) }}
		>
			{label}
		</span>
	);
}

/** One citing sentence: the sentence between its neighbours, a link to it, where it is, and how it was read. */
export function SentenceRow({ sentence, label }: { sentence: CitingSentenceShown; label?: string }) {
	const judgement = sentence.judgement;
	return (
		<li className="flex flex-col gap-1 rounded-md border-l-2 border-border py-1 pl-3">
			<p className="font-sans text-sm leading-relaxed" title={SENTENCES_NOTE}>
				{sentence.before && <span className="text-muted-foreground">{sentence.before.quote} </span>}
				<span className="text-foreground">{sentence.evidence.quote}</span>
				{sentence.after && <span className="text-muted-foreground"> {sentence.after.quote}</span>}
			</p>
			<div className="flex flex-wrap items-center gap-x-3 gap-y-1 font-ui text-xs text-muted-foreground">
				<EvidenceLink evidence={sentence.evidence} label={label} />
				<span title="The section of the citing paper, from its heading">{sectionName(sentence.section)}</span>
				<span title={`Written “${sentence.written}”. A work cited in a group says less about each of the works in it.`}>
					{groupingNote(sentence.grouping)}
				</span>
				<KindTag sentence={sentence} />
				{judgement.state === "justified" && (
					<span className="inline-flex items-baseline gap-1" title={KINDS_NOTE}>
						because: <span className="text-foreground">&ldquo;{judgement.because.quote}&rdquo;</span>
						<EvidenceLink evidence={judgement.because} label={label} />
					</span>
				)}
			</div>
		</li>
	);
}

/** The counts of a tally, each a button that shows the sentences behind it. */
export function TallyCounts({
	tally,
	kind,
	onKind,
}: {
	tally: CitationContextsOf["tally"];
	kind: CitationKind | null;
	onKind: (kind: CitationKind | null) => void;
}) {
	if (!classified(tally)) return <span className="font-ui text-xs text-muted-foreground">{tallyLine(tally)}</span>;
	const parts = KINDS.map((each) => ({
		each,
		times: each === "mention" ? tally.mentions + tally.not_justified : (tally.kinds[each] ?? 0),
	})).filter(({ times }) => times > 0);
	return (
		<span className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 font-ui text-xs text-muted-foreground" title={KINDS_NOTE}>
			<button type="button" className={cn("hover:text-foreground", kind === null && "text-foreground")} onClick={() => onKind(null)}>
				{tallyLine(tally)}
			</button>
			{parts.map(({ each, times }) => (
				<button
					key={each}
					type="button"
					className={cn(
						"rounded-sm border px-1 hover:text-foreground",
						kind === each ? "border-foreground/50 text-foreground" : "border-border",
					)}
					onClick={() => onKind(kind === each ? null : each)}
					title={`Show the ${count(times, "sentence")} read as “${KIND_LABEL[each]}”, of ${count(tally.sentences, "citing sentence")}. ${KINDS_NOTE}`}
				>
					{times} of {tally.sentences}: {each === "mention" ? "mentions" : KIND_LABEL[each]}
				</button>
			))}
		</span>
	);
}

/** One paper and the sentences between it and the paper the reader shows. */
function Between({ between, onOpen }: { between: CitationContextsOf; onOpen: (id: string) => void }) {
	const [kind, setKind] = useState<CitationKind | null>(null);
	const [all, setAll] = useState(false);
	const sentences = sentencesOfKind(between.sentences, kind);
	const shown = all ? sentences : sentences.slice(0, SHOWN);
	return (
		<li className="flex flex-col gap-1">
			<RowButton className="flex-col gap-0.5" onClick={() => onOpen(between.paper)} title="Open this paper in the reader">
				<span className="text-base leading-snug">
					<MathText>{between.title}</MathText>
				</span>
			</RowButton>
			<div className="px-2">
				<TallyCounts tally={between.tally} kind={kind} onKind={setKind} />
			</div>
			<ul className="ml-2 flex flex-col gap-1.5">
				{shown.map((sentence) => (
					<SentenceRow key={`${sentence.evidence.paper}:${JSON.stringify(sentence.evidence.established)}`} sentence={sentence} />
				))}
			</ul>
			{sentences.length > SHOWN && (
				<button
					type="button"
					className="ml-4 self-start font-ui text-xs text-muted-foreground hover:text-foreground"
					onClick={() => setAll(!all)}
				>
					{all ? "Show fewer" : `Show all ${count(sentences.length, "sentence")}`}
				</button>
			)}
		</li>
	);
}

/** A job of the engine followed while it runs: its progress and a stop button. */
export type FollowedJob =
	{ state: "idle" } | { state: "running"; id: number; progress: Status["progress"] } | { state: "failed"; error: string };

/** Start a job with `start` and follow it until it ends; `onDone` is called when it finished. */
export function useFollowedJob(onDone: () => void) {
	const [job, setJob] = useState<FollowedJob>({ state: "idle" });
	const start = useCallback(async (begin: () => Promise<{ id: number }>) => {
		try {
			const { id } = await begin();
			setJob({ state: "running", id, progress: null });
		} catch (error) {
			setJob({ state: "failed", error: (error as Error).message });
		}
	}, []);
	useEffect(() => {
		if (job.state !== "running") return;
		const timer = setTimeout(async () => {
			try {
				const status = await api.status(Number.MAX_SAFE_INTEGER);
				if (status.id !== job.id || status.state === "done") {
					setJob({ state: "idle" });
					onDone();
				} else if (status.state === "failed") setJob({ state: "failed", error: status.error ?? "the job failed" });
				else setJob({ ...job, progress: status.progress ?? null });
			} catch (error) {
				setJob({ state: "failed", error: (error as Error).message });
			}
		}, POLL_MS);
		return () => clearTimeout(timer);
	}, [job, onDone]);
	return { job, start };
}

/** "412 of 739 done, USD 0.21 so far", and a stop button. */
export function JobStrip({ job, what }: { job: FollowedJob; what: string }) {
	if (job.state === "failed") return <p className="font-ui text-xs text-destructive">{job.error}</p>;
	if (job.state !== "running") return null;
	const progress = job.progress;
	return (
		<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
			<Loader2 className="size-3.5 animate-spin" />
			<span>
				{what}
				{progress ? `: ${progress.done.toLocaleString()} of ${progress.of.toLocaleString()} done` : "…"}
				{progress && progress.usd > 0 ? `, ${money(progress.usd, USD)} spent so far` : ""}
			</span>
			<Button variant="ghost" title="Stop: nothing new is started; what is under way finishes" onClick={() => void api.cancelJob(job.id)}>
				<Square />
				Stop
			</Button>
		</div>
	);
}

/**
 * The citing sentences of the reader's Citations tab: where this paper cites papers of the library, and where the
 * run's papers cite it.
 */
export function CitationSentences({ paper, run, onOpen }: { paper: string; run?: string; onOpen: (id: string) => void }) {
	const [version, setVersion] = useState(0);
	const contexts = ready(useAsync(() => contextsOf(paper, run), [paper, run, version]));
	const reload = useCallback(() => {
		forgetAsked();
		setVersion((value) => value + 1);
	}, []);
	const { job, start } = useFollowedJob(reload);
	if (!contexts) return null;
	const { coverage } = contexts;
	return (
		<div className="mx-auto mt-6 flex max-w-[68rem] flex-col gap-4 font-sans">
			<section className="flex flex-col gap-1.5">
				<Heading className="font-ui">Where it cites papers of your library</Heading>
				<p className="font-ui text-xs text-muted-foreground" title={SENTENCES_NOTE}>
					{contexts.state === "read"
						? `${count(contexts.cites.length, "paper")} of your library cited, of ${count(contexts.references_listed, "entry", "entries")} in its reference list`
						: contexts.state === "no_pdf"
							? "Its PDF is not kept, so the sentences where it cites other papers cannot be read."
							: "Its PDF could not be read, so the sentences where it cites other papers are not known."}
				</p>
				<ol className="flex flex-col gap-3">
					{contexts.cites.map((between) => (
						<Between key={between.paper} between={between} onOpen={onOpen} />
					))}
				</ol>
			</section>
			<section className="flex flex-col gap-1.5">
				<Heading className="font-ui">Where papers of your library cite it</Heading>
				<p className="font-ui text-xs text-muted-foreground" title={SENTENCES_NOTE}>
					{contexts.cited_by_total > 0
						? `${count(contexts.cited_by_total, "paper")} whose citing sentences were read cite it${
								contexts.cited_by.length < contexts.cited_by_total
									? `; the ${contexts.cited_by.length} with the most sentences are shown`
									: ""
							}`
						: "No paper whose citing sentences were read cites it."}
					{coverage && ` ${coverageLine(coverage)}.`}
				</p>
				{run && coverage && moreToRead(coverage) && job.state !== "running" && (
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
				<JobStrip job={job} what="Reading citing sentences" />
				<ol className="flex flex-col gap-3">
					{contexts.cited_by.map((between) => (
						<Between key={between.paper} between={between} onOpen={onOpen} />
					))}
				</ol>
			</section>
			{contexts.left_out > 0 && (
				<p className="font-ui text-xs text-muted-foreground">
					{count(contexts.left_out, "sentence")} left out: their place was not found again in their paper&apos;s text (its PDF changed since
					they were read).
				</p>
			)}
		</div>
	);
}
