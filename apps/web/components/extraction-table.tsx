"use client";

/**
 * The extraction table of a community or of the marked papers (engine analysis/extraction.rs): one row per paper,
 * one column per question, every value with the paper's own words beside it. It is run on a sample first (the
 * set's most central papers), its estimate shown before, its cost after; then on the rest, with a second click.
 * Nothing is shown when the engine has no such route (the Python backend).
 */
import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Download, Plus, RotateCcw, Square, X } from "lucide-react";
import type {
	ExtractionCell,
	ExtractionColumn,
	ExtractionColumns,
	ExtractionEstimate,
	ExtractionPart,
	ExtractionRow,
	ExtractionScope,
	ExtractionTable as Table,
} from "@rp/contracts";
import { AcceptanceTag } from "@/components/brand";
import { EvidenceLinks } from "@/components/evidence-link";
import { MathText } from "@/components/math-text";
import { Button, Heading } from "@/components/ui/button";
import { api } from "@/lib/api";
import {
	cellText,
	columnId,
	coverage,
	filterOptions,
	filterRows,
	KIND_NAMES,
	leftOutNote,
	moved,
	NOT_STATED,
	sortRows,
	toCsv,
	valueText,
} from "@/lib/extraction";
import { ready, useAsync } from "@/lib/use-async";
import { count, money, tokens } from "@/lib/units";
import { cn } from "@/lib/utils";

const USD = { code: "USD", rate: 1, date: null };
const usd = (amount: number) => money(amount, USD);
/** A budget is the "at most" shown, rounded up to the cent it is shown in, so that what is allowed is what was
 *  shown. */
const ceiling = (amount: number) => Math.ceil(amount * 100) / 100;

const readNote = (model: string) =>
	`Read from the paper by the model (${model}): its reading, not the paper's statement. The quotes beside it are the paper's own words, found in it; a value whose quotes were not found is never shown.`;

/** How a run of the table stands: one state at a time. */
type Asking =
	| { step: "idle" }
	| { step: "starting"; part: ExtractionPart }
	/** `part` is null for a run found under way when the table was opened */
	| { step: "running"; part: ExtractionPart | null; job: number; done: number; of: number; usd: number }
	| { step: "finished"; part: ExtractionPart | null; usd: number; error: string | null };

/** The kind of job the engine gives a run of the table (engine api/extraction.rs). */
const JOB_KIND = "the extraction table";

const POLL_MS = 1200;

/** "about USD 1.80 for 10 papers, at most USD 4.20 (an estimate)", with how it was reckoned on hover. */
function EstimateLine({ estimate }: { estimate: ExtractionEstimate }) {
	if (estimate.usd === null || estimate.most_usd === null) {
		return (
			<p className="text-xs text-warning">
				The price of {estimate.model} is not known, so its cost cannot be shown before asking: choose another model in the settings.
			</p>
		);
	}
	const basis =
		estimate.basis.kind === "texts"
			? "from these papers' own texts"
			: estimate.basis.kind === "sample_texts"
				? `from the mean length of the sample's ${count(estimate.basis.papers, "read paper")} (${count(estimate.basis.characters, "character")}), and each paper's own abstract when it has no PDF`
				: `from a typical paper's length (${count(estimate.basis.characters, "character")}): no paper of the sample has been read yet`;
	const all =
		estimate.tokens.input +
		estimate.tokens.output +
		estimate.tokens.cache_read +
		estimate.tokens.cache_write_5m +
		estimate.tokens.cache_write_1h;
	const title =
		`An estimate, reckoned ${basis}, at ${estimate.counting.chars_per_token} characters a token (${estimate.counting.kind}: it errs high); about ${tokens(all)}. ` +
		`"At most" is if every answer took its longest. Billed by the model's provider in US dollars.`;
	return (
		<p className="text-xs text-muted-foreground tabular-nums" title={title}>
			About {usd(estimate.usd)} for {count(estimate.to_ask, "paper")} with {estimate.model}, at most {usd(ceiling(estimate.most_usd))} (an
			estimate)
		</p>
	);
}

/** One cell: its value with the evidence for it, or what stands in for a value. */
function CellView({ cell, model }: { cell: ExtractionCell; model: string }) {
	switch (cell.kind) {
		case "stated":
			return (
				<div className="flex flex-col gap-0.5">
					<span className="font-sans text-sm leading-snug text-foreground" title={readNote(model)}>
						<MathText>{valueText(cell.value)}</MathText>
					</span>
					<EvidenceLinks evidence={cell.evidence} />
				</div>
			);
		case "not_stated":
			return (
				<span
					className="text-muted-foreground italic"
					title="The model found nothing in the paper that answers this column. That needs no quote."
				>
					{NOT_STATED}
				</span>
			);
		case "not_verified":
		case "malformed":
			return (
				<span
					className="text-muted-foreground/70"
					title="The model gave an answer, but its quotes were not found in the paper (or its number is not in them, or it did not fit the column). It is left out, not shown as a value."
				>
					{cellText(cell)}
				</span>
			);
		case "not_asked":
			return (
				<span className="text-muted-foreground/50" title="Not asked yet: run the table on these papers.">
					–
				</span>
			);
	}
}

/** Write, remove and reorder the columns; add one back from the defaults. */
function ColumnsEditor({ run, given, onSaved }: { run: string; given: ExtractionColumns; onSaved: () => void }) {
	const [columns, setColumns] = useState(given.columns);
	const [question, setQuestion] = useState("");
	const [label, setLabel] = useState("");
	const [kind, setKind] = useState<ExtractionColumn["kind"]>("text");
	const [error, setError] = useState<string | null>(null);
	const missing = given.defaults.filter((one) => !columns.some((column) => column.id === one.id));
	const add = () => {
		const name = label.trim() || question.trim().slice(0, 40).trimEnd();
		const id = columnId(
			name,
			columns.map((column) => column.id),
		);
		setColumns([...columns, { id, label: name, question: question.trim(), kind }]);
		setQuestion("");
		setLabel("");
	};
	const save = () =>
		api.saveExtractionColumns(run, columns).then(
			() => {
				setError(null);
				onSaved();
			},
			(failed: Error) => setError(failed.message),
		);
	return (
		<div className="flex flex-col gap-2 rounded-md border p-2 text-xs">
			<ul className="flex flex-col gap-0.5">
				{columns.map((column, index) => (
					<li key={column.id} className="flex items-center gap-1">
						<span className="min-w-0 flex-1 truncate" title={column.question}>
							{column.label} <span className="text-muted-foreground">· {KIND_NAMES[column.kind]}</span>
						</span>
						<Button variant="ghost" size="icon" title="Move it left" onClick={() => setColumns(moved(columns, index, -1))}>
							<ArrowUp />
						</Button>
						<Button variant="ghost" size="icon" title="Move it right" onClick={() => setColumns(moved(columns, index, 1))}>
							<ArrowDown />
						</Button>
						<Button
							variant="ghost"
							size="icon"
							title="Remove this column (its answers are kept: added back, it is not asked again)"
							onClick={() => setColumns(columns.filter((other) => other.id !== column.id))}
						>
							<X />
						</Button>
					</li>
				))}
			</ul>
			<div className="flex flex-col gap-1 border-t pt-2">
				<input
					className="h-7 rounded-md border bg-transparent px-2"
					placeholder="A question for every paper: Does it evaluate on out-of-distribution data?"
					value={question}
					maxLength={400}
					onChange={(event) => setQuestion(event.target.value)}
				/>
				<div className="flex gap-1">
					<input
						className="h-7 min-w-0 flex-1 rounded-md border bg-transparent px-2"
						placeholder="Its header (short)"
						value={label}
						maxLength={60}
						onChange={(event) => setLabel(event.target.value)}
					/>
					<select
						className="h-7 rounded-md border bg-card px-1"
						value={kind}
						onChange={(event) => setKind(event.target.value as ExtractionColumn["kind"])}
						title="What kind of answer it takes; every column may also be answered 'not stated in the paper'"
					>
						{Object.entries(KIND_NAMES).map(([value, name]) => (
							<option key={value} value={value}>
								{name}
							</option>
						))}
					</select>
					<Button variant="outline" disabled={!question.trim() || columns.length >= 20} onClick={add}>
						<Plus />
						Add
					</Button>
				</div>
				{missing.length > 0 && (
					<div className="flex flex-wrap items-center gap-1 text-muted-foreground">
						Add back:
						{missing.map((column) => (
							<button
								key={column.id}
								type="button"
								className="rounded-sm border px-1.5 py-0.5 hover:text-foreground"
								onClick={() => setColumns([...columns, column])}
							>
								{column.label}
							</button>
						))}
					</div>
				)}
			</div>
			<div className="flex items-center gap-2">
				<Button variant="outline" disabled={columns.length === 0} onClick={() => void save()}>
					Save the columns
				</Button>
				<span className="text-muted-foreground">A new or changed column is asked of each paper on its own, at the next run.</span>
			</div>
			{error && <p className="text-warning">{error}</p>}
		</div>
	);
}

/** The table, sortable by a column, filterable by a column's value, and saved as CSV. */
function Grid({
	run,
	table,
	onOpenPaper,
	onForgotten,
}: {
	run: string;
	table: Table;
	onOpenPaper: (id: string) => void;
	onForgotten: () => void;
}) {
	const [sort, setSort] = useState<{ column: number; descending: boolean } | null>(null);
	const [filter, setFilter] = useState<{ column: number; key: string } | null>(null);
	const columns = table.columns;
	const shown = useMemo(() => {
		const kept = filter && filter.column < columns.length ? filterRows(table.rows, filter.column, filter.key) : table.rows;
		return sort && sort.column < columns.length ? sortRows(kept, sort.column, sort.descending) : kept;
	}, [table, columns, sort, filter]);
	const active = filter?.column ?? sort?.column ?? 0;
	const save = () => {
		const url = URL.createObjectURL(new Blob([toCsv(table, shown)], { type: "text/csv;charset=utf-8" }));
		const link = document.createElement("a");
		link.href = url;
		link.download = `extraction-table-${run}.csv`;
		link.click();
		setTimeout(() => URL.revokeObjectURL(url), 10_000); // once the download has surely started
	};
	const options = filterOptions(table.rows, active);
	return (
		<div className="flex flex-col gap-2">
			<div className="flex flex-wrap items-center gap-1.5 text-xs">
				<span className="text-muted-foreground">Filter</span>
				<select
					className="h-7 rounded-md border bg-card px-1"
					value={filter?.column ?? ""}
					onChange={(event) => setFilter(event.target.value === "" ? null : { column: Number(event.target.value), key: "stated" })}
				>
					<option value="">no filter</option>
					{columns.map((column, index) => (
						<option key={column.id} value={index}>
							{column.label}
						</option>
					))}
				</select>
				{filter && (
					<select
						className="h-7 max-w-40 rounded-md border bg-card px-1"
						value={filter.key}
						onChange={(event) => setFilter({ ...filter, key: event.target.value })}
					>
						{options.map((option) => (
							<option key={option.key} value={option.key}>
								{option.label} ({count(option.papers, "paper")})
							</option>
						))}
					</select>
				)}
				<Button
					variant="ghost"
					className="ml-auto"
					title="Save these rows, with every cell's quotes and pages, as a CSV file"
					onClick={save}
				>
					<Download />
					CSV
				</Button>
			</div>
			{columns[active] && (
				<p className="text-xs text-muted-foreground tabular-nums">{coverage(filter ? shown : table.rows, active, columns[active].label)}</p>
			)}
			<div className="max-h-[32rem] overflow-auto rounded-md border">
				<table className="w-max min-w-full border-collapse text-xs">
					<thead className="sticky top-0 z-10 bg-card">
						<tr>
							<th className="sticky left-0 z-10 w-56 border-b bg-card p-2 text-left font-normal text-muted-foreground">
								{count(shown.length, "paper")}
							</th>
							{columns.map((column, index) => (
								<th key={column.id} className="w-48 border-b p-2 text-left align-top font-normal">
									<button
										type="button"
										className="flex items-center gap-1 text-left text-foreground hover:underline"
										title={`${column.question} Click to sort by it: numbers by value, the rest alphabetically, "${NOT_STATED}" last.`}
										onClick={() =>
											setSort(
												sort?.column === index ? { column: index, descending: !sort.descending } : { column: index, descending: false },
											)
										}
									>
										{column.label}
										{sort?.column === index && (sort.descending ? <ArrowDown className="size-3" /> : <ArrowUp className="size-3" />)}
									</button>
								</th>
							))}
						</tr>
					</thead>
					<tbody>
						{shown.map((row) => (
							<RowView key={row.paper} row={row} model={table.model} run={run} onOpenPaper={onOpenPaper} onForgotten={onForgotten} />
						))}
					</tbody>
				</table>
			</div>
		</div>
	);
}

function RowView({
	row,
	model,
	run,
	onOpenPaper,
	onForgotten,
}: {
	row: ExtractionRow;
	model: string;
	run: string;
	onOpenPaper: (id: string) => void;
	onForgotten: () => void;
}) {
	const asked = row.cells.some((cell) => cell.kind !== "not_asked");
	const [confirming, setConfirming] = useState(false);
	return (
		<tr className="border-b last:border-b-0">
			<td className="sticky left-0 w-56 bg-card p-2 align-top">
				<button
					type="button"
					className="text-left font-sans text-sm leading-snug hover:underline"
					title="Open it in the reader"
					onClick={() => onOpenPaper(row.paper)}
				>
					<MathText>{row.title || row.paper}</MathText>
				</button>
				<div className="mt-1 flex flex-wrap items-center gap-1 text-muted-foreground">
					{row.year !== null && <span>{row.year}</span>}
					<AcceptanceTag state={row.acceptance} term={row.acceptance_term} small />
					{row.sample && <span title="One of the sample's papers: the most central of this set on the graph">sample</span>}
					{(row.text === "no_pdf" || row.text === "unreadable") && (
						<span
							className="text-warning"
							title="No readable PDF is kept: this paper is asked from its title and abstract only, and its quotes are found there."
						>
							abstract only
						</span>
					)}
					{row.text === "not_in_library" && <span title="Not in the library: it is not asked">not in the library</span>}
					{asked && !confirming && (
						<Button
							variant="ghost"
							size="icon"
							className="size-5"
							title="Forget this paper's answers, so that it is asked again at the next run (asks first)"
							onClick={() => setConfirming(true)}
						>
							<RotateCcw />
						</Button>
					)}
					{confirming && (
						<span className="flex flex-wrap items-center gap-1 text-foreground">
							Forget its answers in every table? This cannot be undone, and asking again costs money.
							<Button
								variant="outline"
								className="h-6 px-2"
								onClick={() => void api.forgetExtraction(run, [row.paper]).then(onForgotten, onForgotten)}
							>
								Forget
							</Button>
							<Button variant="ghost" className="h-6 px-2" onClick={() => setConfirming(false)}>
								Keep
							</Button>
						</span>
					)}
				</div>
			</td>
			{row.cells.map((cell, index) => (
				<td key={index} className="w-48 max-w-48 p-2 align-top">
					<CellView cell={cell} model={model} />
				</td>
			))}
		</tr>
	);
}

/** The table once opened: its estimates, its runs, its columns and its rows. */
function Opened({
	run,
	scope,
	given,
	onColumns,
	onOpenPaper,
}: {
	run: string;
	scope: ExtractionScope;
	given: ExtractionColumns;
	onColumns: () => void;
	onOpenPaper: (id: string) => void;
}) {
	const where = JSON.stringify(scope);
	// raised after a run, a redo or new columns: the table and the estimates are read again
	const [version, setVersion] = useState(0);
	const again = () => setVersion((value) => value + 1);
	const table = useAsync(() => api.extractionTable(run, scope), [run, where, version]);
	const sample = useAsync(() => api.extractionEstimate(run, scope, "sample"), [run, where, version]);
	const sampleDone = ready(sample)?.to_ask === 0;
	const tried = ready(table)?.rows.some((row) => row.sample && row.cells.some((cell) => cell.kind !== "not_asked")) ?? false;
	const offerRest = sampleDone || tried;
	const rest = useAsync(offerRest ? () => api.extractionEstimate(run, scope, "rest") : null, [run, where, version, offerRest]);
	const [asking, setAsking] = useState<Asking>({ step: "idle" });
	const [editing, setEditing] = useState(false);

	// A run of the table already under way when this is opened (the section was closed, or another community
	// looked at meanwhile) is followed again rather than offered anew.
	useEffect(() => {
		api.status(Number.MAX_SAFE_INTEGER).then(
			(status) => {
				if (status.state !== "running" || status.kind !== JOB_KIND || status.id === undefined) return;
				const progress = status.progress;
				const job = status.id;
				setAsking((now) =>
					now.step === "idle"
						? { step: "running", part: null, job, done: progress?.done ?? 0, of: progress?.of ?? 0, usd: progress?.usd ?? 0 }
						: now,
				);
			},
			() => {},
		);
	}, []);

	// While a run is under way, and only then, its progress is asked for; when it ends, the table is read again.
	useEffect(() => {
		if (asking.step !== "running") return;
		const timer = setTimeout(() => {
			api.status(Number.MAX_SAFE_INTEGER).then(
				(status) => {
					const ours = status.id === asking.job;
					if (ours && status.state === "running") {
						const progress = status.progress;
						setAsking({ ...asking, done: progress?.done ?? asking.done, of: progress?.of ?? asking.of, usd: progress?.usd ?? asking.usd });
						return;
					}
					setAsking({
						step: "finished",
						part: asking.part,
						usd: ours ? (status.progress?.usd ?? asking.usd) : asking.usd,
						error: ours ? status.error : null,
					});
					again();
				},
				() => setAsking({ ...asking }), // not answered: asked again at the next turn
			);
		}, POLL_MS);
		return () => clearTimeout(timer);
	}, [asking]);

	const begin = (part: ExtractionPart, estimate: ExtractionEstimate) => {
		if (estimate.most_usd === null) return;
		setAsking({ step: "starting", part });
		api.runExtraction(run, scope, part, ceiling(estimate.most_usd)).then(
			({ id }) => setAsking({ step: "running", part, job: id, done: 0, of: estimate.to_ask, usd: 0 }),
			(error: Error) => setAsking({ step: "finished", part, usd: 0, error: error.message }),
		);
	};

	const shownTable = ready(table);
	const sampleEstimate = ready(sample);
	const restEstimate = ready(rest);
	const busy = asking.step === "starting" || asking.step === "running";
	const sampleSize = shownTable?.sample ?? 0;

	const note = shownTable && leftOutNote(shownTable);
	return (
		<div className="flex flex-col gap-2">
			<p className="text-xs text-muted-foreground">
				What each paper says, one column per question, with the paper&apos;s own words for every value. It is tried first on a sample: the{" "}
				{count(sampleSize, "paper")} most central to this set on the graph (the most similar to its other papers, by the summed similarity
				of their links).
			</p>
			{table.status === "failed" && <p className="text-xs text-warning">{table.error}</p>}

			{asking.step === "running" ? (
				<div className="flex items-center gap-2 text-xs tabular-nums">
					<span className="flex-1">
						Asking the model: {asking.done} of {count(asking.of, "paper")}, {usd(asking.usd)} so far
					</span>
					<Button
						variant="outline"
						title="Stop: nothing new is asked, what is under way finishes and is kept"
						onClick={() => void api.cancelJob(asking.job)}
					>
						<Square />
						Stop
					</Button>
				</div>
			) : (
				<>
					{asking.step === "finished" && (
						<p className={cn("text-xs tabular-nums", asking.error ? "text-warning" : "text-foreground")}>
							{asking.error
								? `The run stopped: ${asking.error}. ${usd(asking.usd)} was spent; what was answered is kept.`
								: `${asking.part === "sample" ? "The sample" : asking.part === "rest" ? "The rest" : "The run"} cost ${usd(asking.usd)} (as billed at the engine's prices).`}
						</p>
					)}
					{sampleEstimate && sampleEstimate.to_ask > 0 && (
						<div className="flex flex-col gap-1">
							<EstimateLine estimate={sampleEstimate} />
							<Button disabled={busy || sampleEstimate.most_usd === null} onClick={() => begin("sample", sampleEstimate)}>
								Run on the sample: {count(sampleEstimate.to_ask, "paper")}
							</Button>
						</div>
					)}
					{offerRest && restEstimate && restEstimate.to_ask > 0 && restEstimate.usd !== null && (
						<div className="flex flex-col gap-1">
							<EstimateLine estimate={restEstimate} />
							<Button variant="outline" disabled={busy} onClick={() => begin("rest", restEstimate)}>
								Run on the remaining {count(restEstimate.to_ask, "paper")} (about {usd(restEstimate.usd)}, estimate)
							</Button>
						</div>
					)}
				</>
			)}

			<div className="flex items-center gap-2">
				<Button variant="ghost" onClick={() => setEditing(!editing)}>
					{editing ? <ChevronDown /> : <ChevronRight />}
					Columns · {count(given.columns.length, "column")}
				</Button>
			</div>
			{editing && (
				<ColumnsEditor
					run={run}
					given={given}
					onSaved={() => {
						setEditing(false);
						onColumns();
						again();
					}}
				/>
			)}
			{note && (
				<p
					className="text-xs text-muted-foreground"
					title="These cells are not shown as values: only what the paper's own words were found for is."
				>
					{note}
				</p>
			)}
			{shownTable && (
				<Grid
					key={shownTable.columns.map((column) => column.id).join(" ")}
					run={run}
					table={shownTable}
					onOpenPaper={onOpenPaper}
					onForgotten={again}
				/>
			)}
		</div>
	);
}

/**
 * The "Table" section of a community, or of the marked papers: closed until opened, so that nothing is read or
 * reckoned before it is wanted.
 */
export function ExtractionTable({ run, scope, onOpenPaper }: { run: string; scope: ExtractionScope; onOpenPaper: (id: string) => void }) {
	const [open, setOpen] = useState(false);
	// the columns are asked for first: an engine without the route answers nothing, and nothing is shown
	const columns = useAsync(() => api.extractionColumns(run), [run]);
	const given = ready(columns);
	if (!given) return null;
	return (
		<section className="flex flex-col gap-2 px-4 pb-4">
			<button
				type="button"
				className="flex items-center gap-1 text-left"
				onClick={() => setOpen(!open)}
				title="A table of what each paper says, with its quotes"
			>
				{open ? <ChevronDown className="size-3 text-muted-foreground" /> : <ChevronRight className="size-3 text-muted-foreground" />}
				<Heading>Table</Heading>
			</button>
			{open && (
				<Opened
					key={`${run}:${JSON.stringify(scope)}`}
					run={run}
					scope={scope}
					given={given}
					onColumns={columns.reload}
					onOpenPaper={onOpenPaper}
				/>
			)}
		</section>
	);
}
