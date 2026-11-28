"use client";

import { useEffect, useReducer, useRef, useState } from "react";
import { Loader2, Pencil, Play, RotateCcw, Settings as SettingsIcon, Trash2 } from "lucide-react";
import type { Catalog, Library, RunPage, RunRequest, RunSummary, Settings, UsageEntry } from "@rp/contracts";
import { Logo } from "@/components/brand";
import { ModelChoice, chosenPrice } from "@/components/shell/model-choice";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { useConfirm } from "@/components/shell/confirm";
import { Input, Label, SwitchRow } from "@/components/ui/field";
import { GroupedPicker } from "@/components/ui/picker";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/net/api";
import { accepting, costWords, shownModel } from "@/lib/cost";
import { READING_TITLE, chosenLabel, chosenOf, modelsApart, unavailableBecause, type Choosable, type ModelRole } from "@/lib/shell/models";
import { engineWarnings, IMPORT_ADVICE, keepDismissed, notDismissed, readDismissed } from "@/lib/shell/notices";
import { withSignal } from "@/lib/net/request";
import {
	FIRST,
	RUNS_PAGE,
	SEARCH_DEBOUNCE_MS,
	listedOfSummary,
	matchesSearch,
	offsetAsked,
	pageOf,
	runsListStep,
	shownLine,
	withOpenRun,
	type ListedRun,
} from "@/lib/shell/runs-list";
import { ready, useAsync } from "@/lib/use-async";
import { useResizable } from "@/lib/ui/use-resizable";
import { count, currencyOf, money, tokens } from "@/lib/units";
import { APP_NAME } from "@/lib/utils";

const MODES = [
	{ value: "venues", label: "Conferences" },
	{ value: "dates", label: "Date range" },
] as const;

const isoDay = (offsetDays: number) => new Date(Date.now() + offsetDays * 864e5).toISOString().slice(0, 10);
const list = (text: string) =>
	text
		.split(",")
		.map((part) => part.trim())
		.filter(Boolean);

/** What the form holds. Things come from the menus (`picked`) and, for anything a menu lacks, typed (`more`). */
interface Form {
	conferences: string[];
	year: string;
	moreVenues: string;
	workshops: boolean;
	start: string;
	end: string;
	categories: string[];
	moreCategories: string;
}

function request(mode: (typeof MODES)[number]["value"], form: Form): RunRequest {
	return mode === "venues"
		? {
				venues: [...form.conferences.map((name) => `${name} ${form.year}`), ...list(form.moreVenues)],
				include_workshops: form.workshops,
			}
		: { start: form.start, end: form.end, categories: [...form.categories, ...list(form.moreCategories)] };
}

function RunForm({
	catalog,
	running,
	onStart,
}: {
	catalog: Catalog | undefined;
	running: boolean;
	onStart: (request: RunRequest) => void;
}) {
	const [mode, setMode] = useState<(typeof MODES)[number]["value"]>("venues");
	const [form, setForm] = useState<Form>({
		conferences: ["NeurIPS", "ICML"],
		year: String(new Date().getFullYear()),
		moreVenues: "",
		workshops: false,
		start: isoDay(-7),
		end: isoDay(0),
		categories: ["cs.LG", "stat.ML"],
		moreCategories: "",
	});
	// naming the communities costs money: its most is shown here and accepted with the click (the engine only)
	const naming = ready(useAsync(() => api.describeBeforeRun(), [running]));
	const [named, setNamed] = useState(true);
	const describe = naming?.api_key_set && named ? { describe: { ...shownModel(naming.estimate), ...accepting(naming.estimate) } } : {};
	const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm({ ...form, [key]: value });
	const text = (key: "year" | "moreVenues" | "start" | "end" | "moreCategories") => ({
		value: form[key],
		onChange: (event: React.ChangeEvent<HTMLInputElement>) => set(key, event.target.value),
	});

	return (
		<div className="flex flex-col gap-3">
			<Segmented options={MODES} value={mode} onChange={setMode} className="flex" />
			{mode === "venues" ? (
				<>
					<div className="grid grid-cols-[1fr_5rem] gap-2">
						<Label>
							Conferences
							<GroupedPicker
								groups={catalog?.conferences ?? []}
								picked={form.conferences}
								onChange={(conferences) => set("conferences", conferences)}
								placeholder="Pick conferences"
							/>
						</Label>
						<Label>
							Year
							<Input type="number" min={2015} max={2100} {...text("year")} />
						</Label>
					</div>
					<Label>
						Others, with their year
						<Input placeholder="NeurIPS 2025, SIGGRAPH 2026" {...text("moreVenues")} />
					</Label>
					<SwitchRow
						label="Include workshop papers"
						hint="Only for conferences found through arXiv comments"
						checked={form.workshops}
						onCheckedChange={(workshops) => set("workshops", workshops)}
					/>
				</>
			) : (
				<>
					<div className="grid grid-cols-2 gap-2">
						<Label>
							From
							<Input type="date" max={form.end} {...text("start")} />
						</Label>
						<Label>
							To
							<Input type="date" min={form.start} {...text("end")} />
						</Label>
					</div>
					<Label>
						arXiv categories (none = all of arXiv)
						<GroupedPicker
							groups={catalog?.categories ?? []}
							picked={form.categories}
							onChange={(categories) => set("categories", categories)}
							placeholder="All categories"
						/>
					</Label>
					<Label>
						Other categories
						<Input placeholder="cs.CR, econ.EM" {...text("moreCategories")} />
					</Label>
				</>
			)}
			{naming?.api_key_set && (
				<SwitchRow
					label="Name the communities"
					hint={`With ${naming.model}: ${costWords(naming.estimate)} for up to ${naming.communities_at_most.toLocaleString()} communities (an estimate)`}
					checked={named}
					onCheckedChange={setNamed}
				/>
			)}
			<Button disabled={running} onClick={() => onStart({ ...request(mode, form), ...describe })}>
				{running ? <Loader2 className="animate-spin" /> : <Play />}
				{running ? "Running" : "Run"}
			</Button>
		</div>
	);
}

/** One run in the list. Its title can be edited in place (pencil, or double-click); the arrow does the whole run
 *  again from scratch; the bin removes the run. */
function RunRow({
	run,
	active,
	onSelect,
	onRename,
	onRedoAll,
	onDelete,
}: {
	run: ListedRun;
	active: boolean;
	/** do this run again with everything redone; undefined while a run is in progress */
	onRedoAll?: () => void;
	onSelect: () => void;
	onRename: (title: string) => void;
	onDelete: () => void;
}) {
	const [draft, setDraft] = useState<string | null>(null); // the title being typed; null = not renaming
	const confirm = useConfirm();
	const details = run.problem ?? run.details ?? "Not read yet";
	if (draft !== null) {
		const finish = (save: boolean) => {
			if (save && draft.trim() !== run.title) onRename(draft);
			setDraft(null);
		};
		return (
			<div className="flex flex-col gap-0.5 px-2 py-1.5">
				<Input
					autoFocus
					value={draft}
					className="h-7"
					onChange={(event) => setDraft(event.target.value)}
					onFocus={(event) => event.target.select()}
					onBlur={() => finish(true)}
					onKeyDown={(event) => {
						if (event.key === "Enter") finish(true);
						if (event.key === "Escape") finish(false);
					}}
				/>
				<span className="text-xs text-muted-foreground tabular-nums">{details}</span>
			</div>
		);
	}
	return (
		<div className="group relative">
			<RowButton active={active} className="flex-col gap-0.5" onClick={onSelect} onDoubleClick={() => setDraft(run.title)}>
				<span className="w-full truncate text-sm text-foreground group-hover:pr-20">{run.title}</span>
				<span className={run.problem ? "text-xs text-warning" : "text-xs text-muted-foreground tabular-nums"}>{details}</span>
			</RowButton>
			{/* shown on hover, and while one of them has the keyboard's focus (the dialog it opens gives it back) */}
			<div className="absolute top-1 right-1 hidden gap-0.5 group-focus-within:flex group-hover:flex">
				<Button variant="ghost" size="icon" className="size-6" title="Rename" onClick={() => setDraft(run.title)}>
					<Pencil />
				</Button>
				<Button
					variant="ghost"
					size="icon"
					className="size-6"
					disabled={!onRedoAll}
					title={
						onRedoAll
							? "Redo all of this run: download, embed and describe everything again from scratch (asks first, with what it replaces and what naming costs)"
							: "A run is in progress: wait for it to finish"
					}
					// what it replaces and what naming would cost are asked before anything is done (lib/shell/run-replace.ts)
					onClick={() => onRedoAll?.()}
				>
					<RotateCcw />
				</Button>
				<Button
					variant="ghost"
					size="icon"
					className="size-6"
					title="Delete this run's results"
					onClick={async () => {
						const yes = await confirm({
							title: `Delete "${run.title}"?`,
							body: (
								<p>
									Its communities and graph are removed, and what was asked about them. The downloaded papers, their embeddings and what was
									read of them stay in the library.
								</p>
							),
							confirm: "Delete",
							destructive: true,
						});
						if (yes) onDelete();
					}}
				>
					<Trash2 />
				</Button>
			</div>
		</div>
	);
}

/**
 * The runs, a page at a time and searched as the user types (lib/shell/runs-list.ts): the engine's paged list. The open run stays in sight, first, when it is not among
 * the rows shown. `plain` is the whole list the page holds: it says when the list changed (a run made, renamed or
 * deleted) and what is known of the open run.
 */
function RunsSection({
	plain,
	current,
	running,
	onSelect,
	onRename,
	onRedoAll,
	onDelete,
}: {
	plain: RunSummary[];
	current: string | null;
	running: boolean;
	onSelect: (name: string) => void;
	onRename: (name: string, title: string) => void;
	onRedoAll: (name: string) => void;
	onDelete: (name: string) => void;
}) {
	const [typed, setTyped] = useState("");
	const [list, dispatch] = useReducer(runsListStep, FIRST);
	const searched = useRef(list.search);
	searched.current = list.search;

	// searched once typing pauses
	useEffect(() => {
		if (typed.trim() === searched.current) return;
		const timer = setTimeout(() => dispatch({ kind: "search", search: typed.trim() }), SEARCH_DEBOUNCE_MS);
		return () => clearTimeout(timer);
	}, [typed]);

	// the list changed (a run made, renamed or deleted, or made again): asked again, the rows in view until then
	const changed = plain.map((run) => `${run.name}\t${run.title}\t${run.papers}\t${run.communities}`).join("\n");
	const firstLook = useRef(true);
	useEffect(() => {
		if (firstLook.current) {
			firstLook.current = false;
			return;
		}
		dispatch({ kind: "search", search: searched.current });
	}, [changed]);

	// the page the list asks for now; called off when another is asked for
	const offset = offsetAsked(list);
	const { search, asked } = list;
	useEffect(() => {
		if (offset === null) return;
		const controller = new AbortController();
		withSignal(controller.signal, () => api.runsPage({ offset, limit: RUNS_PAGE, sort: "created", search: search || undefined })).then(
			(answer) => dispatch({ kind: "page", asked, offset, ...pageOf(answer as RunPage) }),
			(error: Error) => controller.signal.aborted || dispatch({ kind: "refused", asked, error: error.message }),
		);
		return () => controller.abort();
	}, [offset, search, asked]);

	// nothing could be listed at all: the whole list the page holds, searched here, every run of it
	const rows =
		list.state === "failed" && list.rows.length === 0 ? plain.filter((run) => matchesSearch(run, search)).map(listedOfSummary) : list.rows;
	const known = plain.find((run) => run.name === current);
	const shown = withOpenRun(rows, current, known && listedOfSummary(known));
	const more = shownLine(list);
	return (
		<section>
			<Heading className="mb-2">Runs</Heading>
			{(plain.length > RUNS_PAGE || search !== "" || typed !== "" || (list.total ?? 0) > RUNS_PAGE) && (
				<Input
					className="mb-2 h-7"
					type="search"
					placeholder="Search the runs"
					aria-label="Search the runs by title or name"
					value={typed}
					onChange={(event) => setTyped(event.target.value)}
				/>
			)}
			{list.state === "loading" && list.rows.length === 0 && <p className="text-xs text-muted-foreground">Listing the runs…</p>}
			{list.state !== "loading" && shown.rows.length === 0 && (
				<p className="text-xs text-muted-foreground">{search ? `No run matches "${search}".` : "No runs yet. Start one above."}</p>
			)}
			{shown.rows.map((run) => (
				<div key={run.name}>
					{run.name === shown.pinned && <span className="px-2 text-[0.7rem] text-muted-foreground">Open now</span>}
					<RunRow
						run={run}
						active={current === run.name}
						onSelect={() => run.problem === null && onSelect(run.name)} // one whose files cannot be read is said, not opened
						onRename={(title) => onRename(run.name, title)}
						onRedoAll={running || run.problem ? undefined : () => onRedoAll(run.name)}
						onDelete={() => onDelete(run.name)}
					/>
				</div>
			))}
			{list.state === "failed" && (
				<div className="mt-1 flex items-center gap-2 text-xs text-destructive">
					<span>The runs could not be listed: {list.error}</span>
					<Button variant="ghost" className="h-6 px-2" onClick={() => dispatch({ kind: "search", search })}>
						Try again
					</Button>
				</div>
			)}
			{more && (
				<div className="mt-1 flex items-center justify-between gap-2 px-2 text-xs text-muted-foreground tabular-nums">
					<span>{more}</span>
					<Button variant="ghost" className="h-6 px-2" disabled={list.state !== "shown"} onClick={() => dispatch({ kind: "more" })}>
						{list.state === "more" ? <Loader2 className="animate-spin" /> : null}
						Show more
					</Button>
				</div>
			)}
		</section>
	);
}

/** The dollar itself, until the engine has said what the user's currency is worth. */
const DOLLAR = { code: "USD", rate: 1, date: null };

/** The model that writes names and overviews and its provider (a menu to change both, and the effort), what it
 *  costs, and what has been spent so far; the breakdown on hover. */
function CostLine({
	choosable,
	settings,
	providerOwned,
	onChange,
}: {
	choosable: Choosable;
	settings: Settings | undefined;
	providerOwned: boolean;
	onChange: (changes: Partial<Settings>) => void;
}) {
	const locale = typeof navigator === "undefined" ? undefined : navigator.language;
	const wanted = currencyOf(locale); // the currency of the user's region; the bills themselves are in US dollars
	const usage = useAsync(() => api.usage(wanted), [wanted]);
	// calls whose outcome is unknown: said on their own, never added into what was spent
	const unsettledAsked = useAsync(api.unsettled, []);
	const unsettled = ready(unsettledAsked);
	const { reload: reloadUsage } = usage;
	const { reload: reloadUnsettled } = unsettledAsked;
	useEffect(() => {
		const reload = () => {
			reloadUsage();
			reloadUnsettled();
		};
		// Names and overviews are written in the background, so the cost is asked for again now and then; not while
		// the window is hidden (minimised, behind another), where nobody reads it, but at once when it is shown.
		const timer = setInterval(() => document.hidden || reload(), 15_000);
		const shown = () => document.hidden || reload();
		document.addEventListener("visibilitychange", shown);
		return () => {
			clearInterval(timer);
			document.removeEventListener("visibilitychange", shown);
		};
	}, [reloadUsage, reloadUnsettled]);
	const unknownOutcome = unsettled && unsettled.calls > 0 ? unsettled.said : null;
	const spent = ready(usage);
	const chosen = chosenOf(choosable.catalog, settings);
	const reader = chosenOf(choosable.catalog, settings, "reading");
	// two models: each is said for what it does; one: it does both, and is shown as it always was
	const apart = modelsApart(choosable.catalog, settings);
	const price = chosenPrice(choosable, settings, spent?.currency ?? DOLLAR, locale);
	const readerPrice = apart ? chosenPrice(choosable, settings, spent?.currency ?? DOLLAR, locale, "reading") : null;
	const line = (role: ModelRole, title: string, label: string | null) => {
		const shown = role === "naming" ? chosen : reader;
		const why = shown.provider && unavailableBecause(shown.provider);
		return (
			// a narrow sidebar puts the menu under its label rather than past its edge
			<span className="flex min-w-0 flex-wrap items-center gap-x-1.5" title={title}>
				{label && <span className="shrink-0">{label}</span>}
				<span className="flex max-w-full min-w-0 items-center">
					<ModelChoice
						quiet
						role={role}
						choosable={choosable}
						settings={settings}
						providerOwned={providerOwned}
						currency={spent?.currency ?? DOLLAR}
						locale={locale}
						onChange={onChange}
					/>
				</span>
				{why && <span className="min-w-0 text-warning">· {why}</span>}
			</span>
		);
	};
	const choice = apart ? (
		<>
			{line("naming", "The model that names communities and writes overviews, and who provides it", "Names:")}
			{line("reading", READING_TITLE, "Reads papers:")}
		</>
	) : (
		line(
			"naming",
			"The model that names communities, writes overviews and reads papers, and who provides it. Another can be chosen to read papers in Settings or beside any estimate of a section that reads them.",
			null,
		)
	);
	const unknownLine = unknownOutcome && (
		<span className="text-warning" title="Not counted in what was spent: whether these calls were billed is not known.">
			{unknownOutcome}
		</span>
	);
	// with two models, each price says whose it is
	const priceShown = price && apart ? `Names: ${price}` : price;
	const readerLine = readerPrice && (
		<span className="max-w-full min-w-0 truncate" title={readerPrice}>
			Reads papers: {readerPrice}
		</span>
	);
	if (!spent)
		return (
			<div className="flex flex-col gap-0.5">
				{choice}
				{priceShown && (
					<span className="truncate" title={priceShown}>
						{priceShown}
					</span>
				)}
				{readerLine}
				{unknownLine}
			</div>
		);
	const allTokens = Object.values(spent.models).reduce((sum, entry) => sum + entry.input_tokens + entry.output_tokens, 0);
	const cost = (usd: number) => money(usd, spent.currency, locale);
	const lines = (table: Record<string, UsageEntry>) =>
		Object.entries(table).map(([name, entry]) => {
			const counted = `${count(entry.calls, "call")}, ${tokens(entry.input_tokens)} sent, ${tokens(entry.output_tokens)} received`;
			// no priced call: no cost is shown, since none is known (not a cost of 0)
			return entry.usd === null ? `  ${name}: ${counted}` : `  ${name}: ${cost(entry.usd)} (${counted})`;
		});
	const detail = [
		spent.since ? `Counted since ${spent.since}; calls made before that were not recorded.` : "Nothing spent yet.",
		...(spent.calls ? ["By purpose:", ...lines(spent.purposes), "By model:", ...lines(spent.models)] : []),
		...(spent.unpriced_calls
			? [`${count(spent.unpriced_calls, "call")} to a model with no price ${spent.unpriced_calls === 1 ? "is" : "are"} not in this total.`]
			: []),
		...(unknownOutcome ? [`${unknownOutcome} Not counted above.`] : []),
		...(price ? [`${chosenLabel(chosen)}${apart ? " (names, overviews)" : ""}: ${price}.`] : []),
		...(readerPrice ? [`${chosenLabel(reader)} (reads papers): ${readerPrice}.`] : []),
		"Prices are the published list prices in US dollars per million tokens.",
		...(spent.currency.code === "USD"
			? []
			: [
					`Billed in US dollars; shown in ${spent.currency.code} at the European Central Bank's rate of ${spent.currency.date} (USD 1 = ${spent.currency.code} ${spent.currency.rate}).`,
				]),
	].join("\n");
	return (
		<div className="flex flex-col gap-0.5">
			{choice}
			{/* what was spent goes onto lines of its own when the sidebar is too narrow: never past its edge */}
			<div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-0.5">
				<span className="max-w-full min-w-0 truncate" title={priceShown ?? undefined}>
					{priceShown}
				</span>
				<span className="flex max-w-full min-w-0 flex-wrap gap-x-1 text-foreground/80" title={detail}>
					{/* no priced call: no amount at all, only the calls and their tokens */}
					<span>{spent.usd === null ? `${count(spent.calls, "call")}, ${tokens(allTokens)}` : `${cost(spent.usd)} spent`}</span>
					{spent.session_calls > 0 && spent.session_usd !== null && (
						<span className="text-muted-foreground">· {cost(spent.session_usd)} this session</span>
					)}
				</span>
			</div>
			{readerLine}
			{unknownLine}
		</div>
	);
}

/**
 * What the engine reports that is not about a run: how it listens (not dismissible: it stays true while it runs so),
 * and what of the earlier version's data could not be moved in, each dismissible until the app is next started
 * (Settings keeps listing them).
 */
function Notices({
	warnings,
	importProblems,
	onOpenSettings,
}: {
	warnings: string[];
	importProblems: string[];
	onOpenSettings: () => void;
}) {
	const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => (typeof window === "undefined" ? new Set() : readDismissed()));
	const shown = notDismissed(importProblems, dismissed);
	const listening = engineWarnings(warnings, importProblems);
	if (shown.length === 0 && listening.length === 0) return null;
	const dismiss = (problem: string) => {
		const next = new Set(dismissed).add(problem);
		keepDismissed(next);
		setDismissed(next);
	};
	return (
		<section className="flex flex-col gap-2" aria-label="Notices">
			{listening.map((warning) => (
				<p key={warning} role="alert" className="rounded-md border border-warning/40 px-2.5 py-2 text-xs text-warning">
					{warning}
				</p>
			))}
			{shown.map((problem) => (
				<div key={problem} role="status" className="flex flex-col gap-1.5 rounded-md border border-warning/40 px-2.5 py-2 text-xs">
					<p className="text-warning">{problem}</p>
					<p className="text-muted-foreground">{IMPORT_ADVICE}</p>
					<div className="flex justify-end gap-1">
						<Button variant="ghost" className="h-6 px-2" onClick={onOpenSettings}>
							Settings
						</Button>
						<Button
							variant="ghost"
							className="h-6 px-2"
							title="Hide this until Constellate is next started"
							onClick={() => dismiss(problem)}
						>
							Dismiss
						</Button>
					</div>
				</div>
			))}
		</section>
	);
}

export function Sidebar({
	runs,
	current,
	running,
	library,
	choosable,
	settings,
	providerOwned,
	onChange,
	catalog,
	error,
	told,
	warnings,
	onSelect,
	onRename,
	onRedoAll,
	onDelete,
	onStart,
	onOpenSettings,
}: {
	/** what came of the last start that is not an error ("the names are kept as they are") */
	told?: string | null;
	/** what is unsafe about how the engine listens (`Status.warnings`, the import problems left out) */
	warnings?: string[];
	runs: RunSummary[];
	current: string | null;
	running: boolean;
	library: Library | undefined;
	/** the language models that can write names and overviews, the settings that choose one, and what to do when
	 *  another is picked in the footer; the provider is the server's when `providerOwned` */
	choosable: Choosable;
	settings: Settings | undefined;
	providerOwned: boolean;
	onChange: (changes: Partial<Settings>) => void;
	catalog: Catalog | undefined;
	error: string | null;
	onSelect: (name: string) => void;
	onRename: (name: string, title: string) => void;
	/** do a run again with every step redone */
	onRedoAll: (name: string) => void;
	onDelete: (name: string) => void;
	onStart: (request: RunRequest) => void;
	onOpenSettings: () => void;
}) {
	const embedded = Object.values(library?.embeddings ?? {}).reduce((sum, count) => sum + count, 0);
	const { ref, style, handle } = useResizable("sidebar");
	return (
		<aside ref={ref} style={style} className="relative flex w-(--panel-size) shrink-0 flex-col border-r bg-card">
			<ResizeHandle {...handle} />
			<div className="flex h-11 items-center justify-between border-b px-4">
				<span className="flex items-center gap-2 text-sm font-semibold tracking-tight">
					<Logo />
					{APP_NAME}
				</span>
				<Button variant="ghost" size="icon" title="Settings" onClick={onOpenSettings}>
					<SettingsIcon />
				</Button>
			</div>
			<div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-4">
				<Notices warnings={warnings ?? []} importProblems={library?.import_problems ?? []} onOpenSettings={onOpenSettings} />
				<section>
					<Heading className="mb-2">New run</Heading>
					<RunForm catalog={catalog} running={running} onStart={onStart} />
					{error && <p className="mt-2 text-xs text-destructive">{error}</p>}
					{!error && told && <p className="mt-2 text-xs text-muted-foreground">{told}</p>}
				</section>
				<RunsSection
					plain={runs}
					current={current}
					running={running}
					onSelect={onSelect}
					onRename={onRename}
					onRedoAll={onRedoAll}
					onDelete={onDelete}
				/>
			</div>
			<div className="flex flex-col gap-1 border-t px-4 py-2.5 text-xs text-muted-foreground tabular-nums">
				<div>
					Library: {library ? count(library.papers, "paper") : "–"} · {count(embedded, "embedding")}
				</div>
				<CostLine choosable={choosable} settings={settings} providerOwned={providerOwned} onChange={onChange} />
			</div>
		</aside>
	);
}
