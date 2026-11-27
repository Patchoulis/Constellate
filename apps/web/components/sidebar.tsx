"use client";

import { useEffect, useState } from "react";
import { Loader2, Pencil, Play, RotateCcw, Settings as SettingsIcon, Trash2 } from "lucide-react";
import type { Catalog, Library, RunRequest, RunSummary, Settings, UsageEntry } from "@rp/contracts";
import { Logo } from "@/components/brand";
import { ModelChoice, chosenPrice } from "@/components/model-choice";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { Input, Label, SwitchRow } from "@/components/ui/field";
import { GroupedPicker } from "@/components/ui/picker";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/api";
import { chosenLabel, chosenOf, unavailableBecause, type Choosable } from "@/lib/models";
import { ready, useAsync } from "@/lib/use-async";
import { useResizable } from "@/lib/use-resizable";
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
			<Button disabled={running} onClick={() => onStart(request(mode, form))}>
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
	run: RunSummary;
	active: boolean;
	/** do this run again with everything redone; undefined while a run is in progress */
	onRedoAll?: () => void;
	onSelect: () => void;
	onRename: (title: string) => void;
	onDelete: () => void;
}) {
	const [draft, setDraft] = useState<string | null>(null); // the title being typed; null = not renaming
	const details = `${count(run.papers, "paper")} · ${count(run.communities, "community", "communities")}`;
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
				<span className="text-xs text-muted-foreground tabular-nums">{details}</span>
			</RowButton>
			<div className="absolute top-1 right-1 hidden gap-0.5 group-hover:flex">
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
							? "Redo all of this run: download, embed and describe everything again from scratch"
							: "A run is in progress: wait for it to finish"
					}
					onClick={() =>
						window.confirm(
							`Redo all of "${run.title}"?\n\nIts ${count(run.papers, "paper")} are downloaded, embedded and described again from scratch. This takes a while and, with a language model's API key set, the descriptions cost money again.`,
						) && onRedoAll?.()
					}
				>
					<RotateCcw />
				</Button>
				<Button
					variant="ghost"
					size="icon"
					className="size-6"
					title="Delete this run's results"
					onClick={() =>
						window.confirm(
							`Delete "${run.title}"?\n\nIts communities and graph are removed. The downloaded papers, embeddings and descriptions stay in the library.`,
						) && onDelete()
					}
				>
					<Trash2 />
				</Button>
			</div>
		</div>
	);
}

/** The dollar itself, until the backend has said what the user's currency is worth. */
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
	const { reload } = usage;
	useEffect(() => {
		// Names and overviews are written in the background, so the cost is asked for again now and then; not while
		// the window is hidden (minimised, behind another), where nobody reads it, but at once when it is shown.
		const timer = setInterval(() => document.hidden || reload(), 15_000);
		const shown = () => document.hidden || reload();
		document.addEventListener("visibilitychange", shown);
		return () => {
			clearInterval(timer);
			document.removeEventListener("visibilitychange", shown);
		};
	}, [reload]);
	const spent = ready(usage);
	const chosen = chosenOf(choosable.catalog, settings);
	const why = chosen.provider && unavailableBecause(chosen.provider);
	const price = chosenPrice(choosable, settings, spent?.currency ?? DOLLAR, locale);
	const choice = (
		<span className="flex min-w-0 items-center gap-1.5" title="The model that names communities and writes overviews, and who provides it">
			<ModelChoice
				quiet
				choosable={choosable}
				settings={settings}
				providerOwned={providerOwned}
				currency={spent?.currency ?? DOLLAR}
				locale={locale}
				onChange={onChange}
			/>
			{why && <span className="shrink-0 text-warning">· {why}</span>}
			{!why && choosable.source === "older_backend" && spent && !spent.provider.ready && (
				<span className="shrink-0 text-warning">· no key set</span>
			)}
		</span>
	);
	if (!spent)
		return (
			<div className="flex flex-col gap-0.5">
				{choice}
				{price && <span className="truncate">{price}</span>}
			</div>
		);
	const cost = (usd: number) => money(usd, spent.currency, locale);
	const lines = (table: Record<string, UsageEntry>) =>
		Object.entries(table).map(
			([name, entry]) =>
				`  ${name}: ${cost(entry.usd)} (${count(entry.calls, "call")}, ${tokens(entry.input_tokens)} sent, ${tokens(entry.output_tokens)} received)`,
		);
	const detail = [
		spent.since ? `Counted since ${spent.since}; calls made before that were not recorded.` : "Nothing spent yet.",
		...(spent.calls ? ["By purpose:", ...lines(spent.purposes), "By model:", ...lines(spent.models)] : []),
		...(spent.unpriced_calls
			? [`${count(spent.unpriced_calls, "call")} went to a model whose price is not known here and are not in the total.`]
			: []),
		...(price ? [`${chosenLabel(chosen)}: ${price}.`] : []),
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
			<div className="flex items-center justify-between gap-2">
				<span className="min-w-0 truncate" title={price ?? undefined}>
					{price}
				</span>
				<span className="shrink-0 text-foreground/80" title={detail}>
					{cost(spent.usd)} spent
					{spent.session_calls > 0 && <span className="text-muted-foreground"> · {cost(spent.session_usd)} this session</span>}
				</span>
			</div>
		</div>
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
	onSelect,
	onRename,
	onRedoAll,
	onDelete,
	onStart,
	onOpenSettings,
}: {
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
				<section>
					<Heading className="mb-2">New run</Heading>
					<RunForm catalog={catalog} running={running} onStart={onStart} />
					{error && <p className="mt-2 text-xs text-destructive">{error}</p>}
				</section>
				<section>
					<Heading className="mb-2">Runs</Heading>
					{runs.length === 0 && <p className="text-xs text-muted-foreground">No runs yet. Start one above.</p>}
					{runs.map((run) => (
						<RunRow
							key={run.name}
							run={run}
							active={current === run.name}
							onSelect={() => onSelect(run.name)}
							onRename={(title) => onRename(run.name, title)}
							onRedoAll={running ? undefined : () => onRedoAll(run.name)}
							onDelete={() => onDelete(run.name)}
						/>
					))}
				</section>
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
