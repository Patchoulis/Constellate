"use client";

/**
 * "Explain this" in the reader's Text tab (engine features/explanation/): a passage, an equation or a caption explained by
 * the model, every statement about the paper with the paper's own words beside it and general background marked
 * as such; the paper's own definitions of the symbols and terms it uses (found without a model); and passages of
 * nearby papers about the same thing (found without a model). The cost is shown before anything is sent, and an
 * explanation is kept, so opening it again is free.
 */
import { useEffect, useRef, useState } from "react";
import { AlignLeft, Loader2, RotateCcw, Sparkles, Square, X } from "lucide-react";
import type { DefinedTerm, Evidence, Explanation, ExplanationStatement, JobProgress, Passage } from "@rp/contracts";
import { EvidenceLink } from "@/components/reader/evidence-link";
import { MathText } from "@/components/math-text";
import { Button, Heading, Segmented } from "@/components/ui/button";
import { useConfirm } from "@/components/shell/confirm";
import { api } from "@/lib/net/api";
import { pages } from "@/lib/reader/evidence";
import {
	ASK_BELOW_KEY,
	BACKGROUND_NOTE,
	MODEL_NOTE,
	PART_HEADING,
	asksUnconfirmed,
	byPart,
	costLine,
	costNote,
	garbledNote,
	leftOutNote,
	placeKey,
	readAskBelow,
	searchedLine,
	symbolSource,
} from "@/lib/features/explanation/explanation";
import { choiceKey, isFor, useChosenModel } from "@/lib/chosen-model";
import { accepting, shownModel } from "@/lib/cost";
import { money } from "@/lib/units";
import { ready, useAsync } from "@/lib/use-async";
import { elsewhereLine, useFollowedJob } from "@/lib/shell/use-followed-job";
import { cn } from "@/lib/utils";

export type ExplainMode = "explain" | "related";

const USD = { code: "USD", rate: 1, date: null };

/** "Do not ask again below": kept in this browser only; unreadable storage means always asking. */
function storedAskBelow(): number | null {
	try {
		return readAskBelow(window.localStorage.getItem(ASK_BELOW_KEY));
	} catch (error) {
		return null;
	}
}

function storeAskBelow(amount: number | null) {
	try {
		if (amount === null) window.localStorage.removeItem(ASK_BELOW_KEY);
		else window.localStorage.setItem(ASK_BELOW_KEY, String(amount));
	} catch (error) {
		// not kept: the next explanation asks again, which is the safe side
	}
}

/** A place in this paper, labelled by its page: the paper is the one being read. */
function Place({ evidence }: { evidence: Evidence }) {
	const established = evidence.established;
	return <EvidenceLink evidence={evidence} label={established.kind === "verified" ? pages(established.address) : "abstract"} />;
}

function Statement({ statement }: { statement: ExplanationStatement }) {
	const symbol = statement.symbol ? symbolSource(statement.symbol) : "";
	const said = (
		<>
			{symbol && (
				<span className="mr-1.5 font-medium text-foreground">
					<MathText>{symbol}</MathText>
				</span>
			)}
			<MathText>{statement.text}</MathText>
		</>
	);
	if (statement.source === "background") {
		return (
			<p title={BACKGROUND_NOTE} className="border-l-2 border-dashed border-muted-foreground/40 pl-2 text-muted-foreground italic">
				<span className="mr-1.5 rounded-sm border border-dashed px-1 font-ui text-[0.65rem] tracking-wide not-italic uppercase">
					Background, not from this paper
				</span>
				{said}
			</p>
		);
	}
	return (
		<p className="border-l-2 border-primary/40 pl-2">
			<span title={MODEL_NOTE}>{said}</span>{" "}
			<span className="font-ui text-xs text-muted-foreground">
				(
				{statement.evidence.map((piece, index) => (
					<span key={index}>
						{index > 0 && "; "}
						<Place evidence={piece} />
					</span>
				))}
				)
			</span>
		</p>
	);
}

export function Explained({ explained, onRedo }: { explained: Explanation; onRedo: () => void }) {
	const left = leftOutNote(explained.left_out);
	return (
		<div className="flex flex-col gap-3">
			<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
				<span title={MODEL_NOTE} className="flex-1">
					Explained by the model ({explained.model}), kept {explained.kept_at.slice(0, 10)}: opening it again costs nothing.
				</span>
				<Button
					variant="ghost"
					className="h-6 px-2 text-xs"
					title="Set this explanation aside and ask the model again (it costs again)"
					onClick={onRedo}
				>
					<RotateCcw />
					Redo
				</Button>
			</div>
			{explained.statements.length === 0 && (
				<p className="text-sm text-muted-foreground">No statement of this explanation could be shown.</p>
			)}
			{byPart(explained.statements).map((group) => (
				<div key={group.part} className="flex flex-col gap-1.5 text-sm leading-relaxed">
					<Heading>{PART_HEADING[group.part]}</Heading>
					{group.statements.map((statement, index) => (
						<Statement key={index} statement={statement} />
					))}
				</div>
			))}
			{left && <p className="font-ui text-xs text-warning">{left}</p>}
		</div>
	);
}

function Definitions({ definitions }: { definitions: DefinedTerm[] }) {
	if (definitions.length === 0) return null;
	return (
		<div className="flex flex-col gap-1">
			<Heading title="Found by a program in the paper's text, with no model: the sentence where the paper introduces each symbol or term this passage uses.">
				Defined here
			</Heading>
			<ul className="flex flex-col gap-1 text-sm">
				{definitions.map((defined) => (
					<li key={defined.term.text} className="flex flex-wrap items-baseline gap-x-1.5">
						<span className="font-medium">{defined.term.text}</span>
						<span className="text-muted-foreground">“{defined.evidence.quote}”</span>
						<Place evidence={defined.evidence} />
					</li>
				))}
			</ul>
		</div>
	);
}

/** Asking the model, as the engine's job: one state at a time. */
type Asking =
	| { state: "idle" }
	| { state: "starting" }
	/** `unanswered`: the last look at the job failed; it is looked at again, and Stop stays */
	| { state: "running"; job: number; progress: JobProgress | null; unanswered: string | null }
	| { state: "failed"; error: string };

function AskBelow({ value, onChange }: { value: number | null; onChange: (value: number | null) => void }) {
	const [amount, setAmount] = useState(value ?? 0.05);
	return (
		<label
			className="flex items-center gap-1.5 font-ui text-xs text-muted-foreground"
			title="Off unless ticked. Kept in this browser only. The amount compared is the most an explanation could cost, not its likely cost; a model with no known price is always asked about first, and so is any estimate other than the first one shown for a passage. Applies from the next passage."
		>
			<input
				type="checkbox"
				checked={value !== null}
				onChange={(event) => {
					const next = event.target.checked ? amount : null;
					storeAskBelow(next);
					onChange(next);
				}}
			/>
			Explain the next passages without this click when the most they could cost is known and at most USD
			<input
				type="number"
				min={0.001}
				step={0.01}
				value={amount}
				onChange={(event) => {
					const next = Number(event.target.value);
					if (!(next > 0)) return;
					setAmount(next);
					if (value !== null) {
						storeAskBelow(next);
						onChange(next);
					}
				}}
				className="h-5 w-16 rounded-sm border bg-transparent px-1 tabular-nums"
			/>
		</label>
	);
}

/** The passages of the open paper already asked about without a confirmation (or redone, or stopped): never
 *  again without a click while the paper is open, whatever the panel does in between. */
export type AskedOnce = Set<string>;

function ExplainView({
	paper,
	passage,
	onExplained,
	askedOnce,
}: {
	paper: string;
	passage: Passage;
	onExplained: () => void;
	askedOnce: AskedOnce;
}) {
	const chosen = useChosenModel();
	// read again when the model chosen changes: the estimate, and the explanation kept, are that model's
	const prepared = useAsync(
		() => api.prepareExplanation(paper, passage.address),
		[paper, passage.address.page, passage.address.start, choiceKey(chosen)],
	);
	const found = ready(prepared);
	const [failure, setFailure] = useState<string | null>(null);
	const confirm = useConfirm();
	// follow the job until it is over (also when the panel was closed and opened again); then what was kept is read
	const reload = useRef(prepared.reload);
	reload.current = prepared.reload;
	const explained = useRef(onExplained);
	explained.current = onExplained;
	const key = placeKey(passage.address);
	const {
		job,
		elsewhere,
		start: follow,
	} = useFollowedJob((ended) => {
		reload.current();
		if (ended.state === "done") explained.current();
	}, `explanation:${paper}:${key}`);
	const asking: Asking =
		job.state === "running"
			? { state: "running", job: job.id, progress: job.progress, unanswered: job.unanswered }
			: job.state === "starting"
				? { state: "starting" }
				: failure !== null
					? { state: "failed", error: failure }
					: job.state === "failed"
						? { state: "failed", error: job.id === null ? `Not asked: ${job.error}` : job.error }
						: { state: "idle" };
	const [askBelow, setAskBelow] = useState<number | null>(null);
	// what decides asking without a confirmation is the setting as it was when the panel opened: changing it
	// applies to the next passage, never to the estimate on screen
	const autoBelow = useRef<number | null | undefined>(undefined);
	useEffect(() => {
		autoBelow.current = storedAskBelow();
		setAskBelow(autoBelow.current);
	}, []);

	// the estimate that may be accepted: answered for this passage and the model chosen now, not one being asked again
	const estimate =
		found?.state === "ready" && found.estimate && prepared.status === "ready" && isFor(found, chosen) ? found.estimate : null;
	const start = () => {
		if (found?.state !== "ready" || !estimate) return;
		askedOnce.add(key);
		setFailure(null);
		// the model and the worst case shown go with the request: the engine refuses if either changed
		void follow(async () => {
			const started = await api.explain(paper, passage.address, shownModel(found), accepting(estimate));
			if (started.started && started.job !== null) return started.job;
			prepared.reload();
			return null;
		});
	};

	// Asked without a confirmation only for the passage the reader just chose to explain, and only on the first
	// estimate it gets: an estimate answered later (another model chosen, the panel's data read again) never asks
	// unseen; the reader clicks.
	const firstEstimate = useRef(true);
	useEffect(() => {
		if (prepared.status !== "ready" || !found || found.state !== "ready") return;
		const first = firstEstimate.current;
		firstEstimate.current = false;
		const below = autoBelow.current;
		if (below === undefined) return; // the setting not read yet: never taken for "on"
		const unconfirmed = asksUnconfirmed({
			askBelow: below,
			current: estimate,
			first,
			keySet: found.api_key_set,
			askedBefore: askedOnce.has(key),
			idle: asking.state === "idle",
		});
		if (unconfirmed) start();
		// start is a fresh closure every render; only the first answer can lead to asking unconfirmed
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [prepared.status, found]);

	if (prepared.status === "failed") return <p className="text-sm text-destructive">{prepared.error}</p>;
	if (!found) return <p className="text-sm text-muted-foreground">Looking at the passage…</p>;
	if (found.state === "no_text")
		return (
			<p className="text-sm text-muted-foreground">
				{found.why === "no_pdf" ? "The library keeps no PDF of this paper" : "This paper's PDF could not be read"}: there is no text to
				explain.
			</p>
		);
	const garbled = garbledNote(found.passage.kind);
	return (
		<div className="flex flex-col gap-3">
			{garbled && (
				<p className="font-ui text-xs text-warning" title="Images of a page are not sent: the model reads the text extracted from the PDF.">
					{garbled}
				</p>
			)}
			<Definitions definitions={found.definitions} />
			{found.explained && (
				<Explained
					explained={found.explained}
					onRedo={async () => {
						const sure = await confirm({
							title: "Set this explanation aside and ask again?",
							body: <p>The kept explanation is forgotten, and asking again costs again. The cost is shown before anything is sent.</p>,
							confirm: "Forget it",
							destructive: true,
						});
						if (!sure) return;
						try {
							await api.redoExplanation(paper, passage.address);
							askedOnce.add(key); // a redo asks again only on a click
							prepared.reload();
						} catch (error) {
							setFailure((error as Error).message);
						}
					}}
				/>
			)}
			{found.estimate && !estimate && asking.state !== "running" && (
				<p className="font-ui text-xs text-muted-foreground">Working out the cost for the model chosen now…</p>
			)}
			{estimate && asking.state !== "running" && (
				<div className="flex flex-col gap-1.5 rounded-md border p-2">
					<p className="font-ui text-xs" title={costNote(estimate)}>
						{costLine(estimate)} Asked of the model chosen in the settings ({found.model}).
					</p>
					<div className="flex flex-wrap items-center gap-3">
						<Button
							className="h-7 text-xs"
							disabled={asking.state === "starting" || elsewhere !== null || !found.api_key_set}
							onClick={start}
						>
							{asking.state === "starting" ? <Loader2 className="animate-spin" /> : <Sparkles />}
							Explain
						</Button>
						<AskBelow value={askBelow} onChange={setAskBelow} />
					</div>
					{elsewhere && <p className="font-ui text-xs text-muted-foreground">{elsewhereLine(elsewhere)}</p>}
					{!found.api_key_set && (
						<p className="font-ui text-xs text-muted-foreground">
							The chosen provider&apos;s key is not set (Settings, Language model, names the variable), so the model cannot be asked.
						</p>
					)}
				</div>
			)}
			{asking.state === "running" && (
				<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
					<Loader2 className="size-3.5 animate-spin" />
					<span className="flex-1">
						Asking the model…
						{asking.progress && asking.progress.usd > 0 && ` ${money(asking.progress.usd, USD)} spent so far.`}
						{asking.unanswered && ` (${asking.unanswered}: looking again.)`}
					</span>
					<Button
						variant="ghost"
						className="h-6 px-2 text-xs"
						title="Stop asking"
						onClick={() => {
							askedOnce.add(key);
							void api.cancelJob(asking.job).catch(() => {});
						}}
					>
						<Square />
						Stop
					</Button>
				</div>
			)}
			{asking.state === "failed" && <p className="text-sm text-destructive">{asking.error}</p>}
		</div>
	);
}

function RelatedView({ paper, run, passage }: { paper: string; run: string; passage: Passage }) {
	const related = useAsync(
		() => api.relatedPassages(run, paper, passage.address),
		[run, paper, passage.address.page, passage.address.start],
	);
	const found = ready(related);
	if (related.status === "failed") return <p className="text-sm text-destructive">{related.error}</p>;
	if (!found) return <p className="text-sm text-muted-foreground">Reading the nearby papers…</p>;
	return (
		<div className="flex flex-col gap-2">
			<p
				className="font-ui text-xs text-muted-foreground"
				title="Found by the words they share, with no model: a passage that uses the same words may still be about something else."
			>
				{searchedLine(found)}
			</p>
			{found.found.length === 0 && (
				<p className="text-sm text-muted-foreground">No passage of these papers shares enough words with this one.</p>
			)}
			<ul className="flex flex-col gap-2">
				{found.found.map((near, index) => (
					<li key={index} className="flex flex-col gap-0.5 text-sm">
						<span className="text-muted-foreground">“{near.evidence.quote}”</span>
						<span className="flex flex-wrap items-baseline gap-x-2 font-ui text-xs text-muted-foreground">
							<EvidenceLink evidence={near.evidence} />
							<span title="The words this passage shares with the one you chose, the rarest first">shares: {near.words.join(", ")}</span>
						</span>
					</li>
				))}
			</ul>
		</div>
	);
}

/** The panel under a chosen passage: its explanation, or passages of nearby papers about the same thing. */
export function PassageExplainer({
	paper,
	run,
	passage,
	mode,
	note,
	onMode,
	onClose,
	onExplained,
	askedOnce,
}: {
	paper: string;
	/** the run the paper is read from: related passages are looked for among its papers */
	run?: string;
	passage: Passage;
	mode: ExplainMode;
	/** said above the panel: how the selection was taken */
	note: string | null;
	onMode: (mode: ExplainMode) => void;
	onClose: () => void;
	/** an explanation was just kept: the passage's mark is shown */
	onExplained: () => void;
	askedOnce: AskedOnce;
}) {
	const tabs = [
		{
			value: "explain" as const,
			label: "Explain",
			icon: <Sparkles />,
			title: "The model explains this passage (shown with its cost first)",
		},
		...(run
			? [
					{
						value: "related" as const,
						label: "Find related",
						icon: <AlignLeft />,
						title: "Passages of nearby papers sharing its words: free",
					},
				]
			: []),
	];
	return (
		<section className={cn("my-1 flex flex-col gap-3 rounded-md border bg-card p-3 font-sans shadow-sm")} aria-label="Explain this passage">
			<div className="flex items-center gap-2">
				<Segmented options={tabs} value={run ? mode : "explain"} onChange={onMode} />
				<span className="flex-1 font-ui text-xs text-muted-foreground">{pages(passage.address)}</span>
				<Button variant="ghost" size="icon" title="Close" onClick={onClose}>
					<X />
				</Button>
			</div>
			{note && <p className="font-ui text-xs text-muted-foreground">{note}</p>}
			{mode === "related" && run ? (
				<RelatedView paper={paper} run={run} passage={passage} />
			) : (
				<ExplainView paper={paper} passage={passage} onExplained={onExplained} askedOnce={askedOnce} />
			)}
		</section>
	);
}

/** Said on the PDF tab: what is selected in the browser's PDF viewer cannot be read, so explaining is on the Text tab. */
export function ExplainFromText({ onText }: { onText: () => void }) {
	return (
		<div className="flex shrink-0 items-center gap-2 border-b px-3 py-1 font-ui text-xs text-muted-foreground">
			<span className="flex-1">
				To explain a passage, an equation or a table, select it on the Text tab: what is selected in the PDF view cannot be read.
			</span>
			<Button variant="ghost" className="h-6 px-2 text-xs" onClick={onText}>
				<AlignLeft />
				Text tab
			</Button>
		</div>
	);
}
