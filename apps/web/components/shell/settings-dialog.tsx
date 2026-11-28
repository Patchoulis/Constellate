"use client";

import { Dialog } from "radix-ui";
import { FileSearch, RotateCcw, X } from "lucide-react";
import { REDO_STEPS, type Catalog, type Library, type ModelProvider, type RedoStep, type Settings } from "@rp/contracts";
import { DetectorTraining } from "@/components/features/detector/detector-training";
import { ModelChoice } from "@/components/shell/model-choice";
import { IMPORT_ADVICE } from "@/lib/shell/notices";
import { READING_TITLE, ROLE_FIELDS, groupsOf, readingStateWords, type Choosable, type ModelRole } from "@/lib/shell/models";
import { Button, Heading, Segmented } from "@/components/ui/button";
import { Input, Label, SwitchRow } from "@/components/ui/field";
import { Choice, GroupedPicker } from "@/components/ui/picker";
import { splitByOwner } from "@/lib/net/engine";
import { SELECT_KEYS, selectKeyName, setPrefs, usePrefs, useSelectKeyName } from "@/lib/ui/prefs";
import { cn } from "@/lib/utils";

/** The whole dialog as data: every setting appears once here, with how it is edited. */
type Field = { key: keyof Settings; label: string; hint?: string } & (
	| { kind: "switch" }
	| { kind: "number"; step?: number }
	// one of a list the engine offers (Catalog), in a dropdown
	| { kind: "choice"; from: "embedding_models" | "clustering" | "acceptance" }
	// any number of a list the engine offers
	| { kind: "several"; from: "flags"; placeholder: string }
	// a language model: a provider and one of its models, and the effort (model-choice.tsx), for one of its roles
	| { kind: "model"; role: ModelRole }
);

/**
 * How the graph is used in this window: the key that selects papers, and whether marks are shown. Kept in this
 * browser (lib/ui/prefs.ts), applied at once, and nothing a run depends on.
 */
function GraphChoices() {
	const prefs = usePrefs();
	const held = useSelectKeyName(prefs.selectKey);
	// the names as on this keyboard: Ctrl is Command on a Mac, Alt is Option
	const mac = useSelectKeyName("primary") !== "Ctrl";
	const keys = SELECT_KEYS.map((key) => ({ value: key, label: selectKeyName(key, mac) }));
	return (
		<section className="flex flex-col gap-2">
			<Heading>The graph</Heading>
			<div className="flex items-start justify-between gap-4 py-1.5">
				<span className="flex flex-col gap-0.5">
					<span className="text-sm text-foreground">Key that selects papers</span>
					<span className="text-xs text-muted-foreground">
						Hold {held} on the graph: a click marks a paper (or unmarks it), and a drag draws a lasso that marks every paper inside it.
						Without the key, a drag moves the graph as before. All three keys work on Windows, macOS and Linux; some Linux desktops use Alt
						with a drag to move windows, so pick another there.
					</span>
				</span>
				<Segmented className="shrink-0" options={keys} value={prefs.selectKey} onChange={(selectKey) => setPrefs({ selectKey })} />
			</div>
			<SwitchRow
				label="Show marked papers on the graph"
				hint="Their yellow rings, links and names. The eye beside the Marked tab does the same. Hidden marks stay marked and listed."
				checked={prefs.marksShown}
				onCheckedChange={(marksShown) => setPrefs({ marksShown })}
			/>
			<SwitchRow
				label="Show marks again when a paper is marked"
				hint="Marking a paper while the marks are hidden shows them. Off: marks stay hidden until the eye is clicked."
				checked={prefs.showMarksOnMark}
				onCheckedChange={(showMarksOnMark) => setPrefs({ showMarksOnMark })}
			/>
		</section>
	);
}

/** How a provider's address is shown: its host, and where that is. */
function where(provider: ModelProvider): string {
	const host = provider.address.host ?? "no address set";
	switch (provider.address.place) {
		case "this_computer":
			return `${host}, on this computer: no API cost`;
		case "set":
			return `${host} (set in ${provider.address.variable})`;
		case "not_set":
			return `no address set: set ${provider.address.variable}`;
		case "provider":
			return host;
	}
}

/** Whether a provider's key is set, by its variable's name: never the key. */
function keyState(provider: ModelProvider): string {
	if (provider.key.set) return `${provider.key.variable} is set`;
	return provider.key.needed ? `${provider.key.variable} is not set` : `no key needed (${provider.key.variable} if the server wants one)`;
}

/** Every provider: whether it can be used, its key (set or not, never the key), its host, what it cannot do. */
function LanguageModels({ choosable }: { choosable: Choosable }) {
	if (choosable.source === "waiting") {
		return (
			<p className={choosable.why ? "text-xs text-warning" : "text-xs text-muted-foreground"}>
				{choosable.why
					? `Constellate could not say which language models it offers: ${choosable.why}`
					: "Asking which language models can be used…"}
			</p>
		);
	}
	return (
		<>
			<p className="text-xs text-muted-foreground">
				Keys and addresses are not entered here. In the desktop app, write the variable named below, as NAME=value, in a file named .env in
				Constellate&apos;s data folder (Windows: %LOCALAPPDATA%\Constellate\data; Linux: ~/.local/share/constellate/data), then close
				Constellate and open it again. On a server, set it in the engine&apos;s environment or its .env. A key is never shown, only whether
				it is set.
			</p>
			<ul className="flex flex-col gap-2">
				{groupsOf(choosable.catalog).map(({ provider, why }) => (
					<li key={provider.id} className="flex flex-col gap-0.5 text-xs">
						<span className="flex items-baseline justify-between gap-2">
							<span className="text-sm text-foreground">{provider.label}</span>
							<span className={why ? "text-warning" : "text-muted-foreground"}>{why ?? "ready"}</span>
						</span>
						<span className="text-muted-foreground">
							{keyState(provider)} · {where(provider)}
						</span>
						{provider.limits.map((limit) => (
							<span key={limit} className="text-muted-foreground">
								{limit}
							</span>
						))}
					</li>
				))}
			</ul>
		</>
	);
}

/** Stored results that can be thrown away and produced again, each as a button that does it now: one step for
 *  all of the open run's papers, or every step. */
const REDO: { steps: RedoStep[]; label: string; hint: string }[] = [
	{ steps: ["download"], label: "Download all again", hint: "Fetch the run's whole paper list again and overwrite the stored records" },
	{ steps: ["embeddings"], label: "Embed all again", hint: "Recompute the embedding of every paper in the run" },
	{
		steps: ["descriptions"],
		label: "Describe all again",
		hint: "Ask the language model for new names and descriptions of every community; what it costs is shown first",
	},
	{
		steps: REDO_STEPS,
		label: "Redo all of it",
		hint: "All three: download, embed and describe everything in the run from scratch; what it replaces and costs is shown first",
	},
];

const AI_GROUP = "AI-written papers";

/** Prices in the menu's hints are told in US dollars here; the sidebar tells them in the user's currency. */
const DOLLAR = { code: "USD", rate: 1, date: null };

const GROUPS: { title: string; note?: string; columns?: number; fields: Field[] }[] = [
	{
		title: "Embeddings",
		fields: [
			{ kind: "choice", key: "embedding_model", label: "Embedding model", from: "embedding_models" },
			{
				kind: "switch",
				key: "center_embeddings",
				label: "Centre embeddings",
				hint: "Remove the mean embedding before cosine similarity, so unrelated papers score below 0 and are clipped",
			},
		],
	},
	{
		title: "Communities",
		fields: [{ kind: "choice", key: "clustering", label: "Clustering method", from: "clustering" }],
	},
	{
		title: "Sizes",
		columns: 2,
		fields: [
			{ kind: "number", key: "max_communities", label: "Communities" },
			{ kind: "number", key: "min_community_size", label: "Smallest size" },
			{ kind: "number", key: "min_split_modularity", label: "Split threshold", step: 0.01 },
			{ kind: "number", key: "sub_levels", label: "Sub-community levels" },
			{ kind: "number", key: "max_links", label: "Most links per paper" },
		],
	},
	{
		title: "Which papers",
		note: "Applied when a run starts. Each rule only judges the papers it knows about.",
		fields: [
			{ kind: "choice", key: "acceptance_filter", label: "Conference papers to include", from: "acceptance" },
			{
				kind: "several",
				key: "exclude_flags",
				label: "Leave out by what authors declare",
				from: "flags",
				placeholder: "Nothing left out",
				hint: "Only acts on papers whose declarations have been read: open a paper to read its own, or read them all below.",
			},
		],
	},
	{
		title: AI_GROUP,
		note:
			"Every abstract is scored for wording of the kind language models produce, and papers above a threshold " +
			"are tagged. It is an estimate from word choice, never proof of how a paper was written.",
		fields: [
			{
				kind: "switch",
				key: "detect_ai",
				label: "Check abstracts for AI-style wording",
				hint: "Works without a graphics card. Off: nothing is scored or tagged",
			},
			{
				kind: "switch",
				key: "include_ai_papers",
				label: "Include tagged papers",
				hint: "Off: papers tagged as AI-written are left out of the next run",
			},
		],
	},
	{
		title: "Descriptions and overviews",
		fields: [
			{
				kind: "switch",
				key: "describe",
				label: "Describe communities with a language model",
				hint: "Needs the chosen provider's key (see Language model below)",
			},
			{
				kind: "model",
				role: "naming",
				key: "describe_model",
				label: "Language model",
				hint: "Names communities and writes their overviews",
			},
			{
				kind: "model",
				role: "reading",
				key: "reading_model",
				label: "Model that reads papers",
				hint: "Tables, agree / disagree, questions, claims, weaknesses, explanations and citation kinds; one setting for all of them, also chosen beside their estimates. Its menu sets it back to the default (on the Claude API its Haiku, elsewhere the naming model) or to the same as the naming model",
			},
		],
	},
];

/** Each setting by the name this dialog gives it: what other screens call it (a run made again says which differ). */
export const SETTING_LABELS: Partial<Record<keyof Settings, string>> = Object.fromEntries(
	GROUPS.flatMap((group) => group.fields.map((field) => [field.key, field.label])),
);

const MODELS_GROUP = "Language model";

/** A change the engine refused, and why. */
export type Refusal = { changes: Partial<Settings>; error: string };

/** The settings a field edits: a model's field picks the provider and the effort too. */
function keysOf(field: Field): (keyof Settings)[] {
	if (field.kind !== "model") return [field.key];
	const { provider, model, effort } = ROLE_FIELDS[field.role];
	return [provider, model, effort];
}

/** Why the last change to this field was refused, or null when it was not. */
export function refusalOf(field: Field, refused: Refusal | null | undefined): string | null {
	return refused && keysOf(field).some((key) => key in refused.changes) ? refused.error : null;
}

function Editor({
	field,
	settings,
	catalog,
	choosable,
	providerOwned,
	onChange,
}: {
	field: Field;
	settings: Settings;
	catalog: Catalog | undefined;
	choosable: Choosable;
	providerOwned: boolean;
	onChange: (changes: Partial<Settings>) => void;
}) {
	const set = (value: Settings[keyof Settings]) => onChange({ [field.key]: value });
	if (field.kind === "model") {
		const state = field.role === "reading" ? readingStateWords(choosable.catalog, settings) : null;
		return (
			// Not a <label>: a label hands its hover and its click to the first control inside it, so pointing anywhere
			// near the field lit the model menu and clicking the field's name or hint opened it. Two menus live here.
			<div role="group" aria-label={field.label} className="flex flex-col gap-1 text-xs text-muted-foreground">
				<span>{field.label}</span>
				<span
					className="flex h-8 items-center rounded-md border border-input bg-background px-2 text-sm"
					title={field.role === "reading" ? READING_TITLE : undefined}
				>
					<ModelChoice
						quiet
						role={field.role}
						choosable={choosable}
						settings={settings}
						providerOwned={providerOwned}
						currency={DOLLAR}
						onChange={onChange}
					/>
				</span>
				{field.hint && <span>{state ? `${state} ${field.hint}` : field.hint}</span>}
			</div>
		);
	}
	if (field.kind === "switch") {
		return <SwitchRow label={field.label} hint={field.hint} checked={Boolean(settings[field.key])} onCheckedChange={set} />;
	}
	if (field.kind === "several") {
		return (
			<Label>
				{field.label}
				<GroupedPicker
					groups={catalog?.[field.from] ?? []}
					picked={settings[field.key] as string[]}
					onChange={set}
					placeholder={field.placeholder}
				/>
				{field.hint && <span>{field.hint}</span>}
			</Label>
		);
	}
	if (field.kind === "choice") {
		return (
			<Label>
				{field.label}
				<Choice options={catalog?.[field.from] ?? []} value={String(settings[field.key])} onChange={set} />
			</Label>
		);
	}
	// Saved when the field is left, so a half-typed value never reaches the engine.
	return (
		<Label>
			{field.label}
			<Input
				type="number"
				step={field.step}
				min={0}
				defaultValue={String(settings[field.key])}
				onBlur={(event) => event.target.value.trim() && set(Number(event.target.value))}
			/>
		</Label>
	);
}

export function SettingsDialog({
	open,
	settings,
	catalog,
	validation,
	detector,
	machine,
	serverOwned = new Set<keyof Settings>(),
	choosable,
	onOpenChange,
	onChange,
	refused,
	onRedo,
	onReadDeclarations,
	importProblems,
	onDetectorChanged,
}: {
	/** what of the earlier version's data folder could not be moved in, in words (`Library.import_problems`) */
	importProblems?: string[];
	/** the detector was trained or checked: what the library says of it is read again */
	onDetectorChanged?: () => void;
	open: boolean;
	settings: Settings | undefined;
	catalog: Catalog | undefined;
	/** how the AI-text detector did when it was last measured, if it was */
	validation: Library["ai_validation"] | undefined;
	/** whether abstracts get scored in the next run, and why */
	detector: Library["ai_detector"] | undefined;
	/** what this computer can do, and what that means for embedding */
	machine: Library["machine"] | undefined;
	/** the settings the server this window is connected to decides (none on the user's own computer) */
	serverOwned?: ReadonlySet<keyof Settings>;
	/** the language models to choose from, and what each provider can do */
	choosable: Choosable;
	onOpenChange: (open: boolean) => void;
	onChange: (changes: Partial<Settings>) => void;
	/** the last change the engine refused and its reason, shown beside the settings it was about */
	refused?: Refusal | null;
	/** Run the open run again with these steps redone; undefined when there is no run to redo, or one is running. */
	onRedo?: (steps: RedoStep[]) => void;
	/** Read the declarations of every paper of the open run; undefined when that cannot be started now. */
	onReadDeclarations?: () => void;
}) {
	return (
		<Dialog.Root open={open} onOpenChange={onOpenChange}>
			<Dialog.Portal>
				<Dialog.Overlay className="fixed inset-0 z-40 bg-black/60" />
				<Dialog.Content className="fixed inset-0 z-50 m-auto flex h-fit max-h-[86dvh] w-[30rem] max-w-[92vw] flex-col rounded-lg border bg-card shadow-2xl">
					<div className="flex h-11 shrink-0 items-center justify-between border-b px-4">
						<Dialog.Title className="text-sm font-semibold">Settings</Dialog.Title>
						<Dialog.Close asChild>
							<Button variant="ghost" size="icon" title="Close">
								<X />
							</Button>
						</Dialog.Close>
					</div>
					<Dialog.Description className="sr-only">Pipeline settings, applied to the next run.</Dialog.Description>
					{settings && (
						<div className="flex flex-col gap-6 overflow-y-auto p-4">
							<section className="flex flex-col gap-2">
								<Heading>Redo</Heading>
								<p className="text-xs text-muted-foreground">
									Anything already stored is reused: a known paper is not downloaded again, an unchanged one is not embedded again, a
									described group is not described again. A button here runs the open run again with that step done from scratch, for all of
									its papers. Each run in the sidebar also has its own button to redo all of it.
								</p>
								{REDO.map(({ steps, label, hint }) => (
									<div key={label} className="flex items-center justify-between gap-4 py-1">
										<span className="text-xs text-muted-foreground">{hint}</span>
										<Button variant="outline" className="w-44" disabled={!onRedo} onClick={() => onRedo?.(steps)}>
											<RotateCcw />
											{label}
										</Button>
									</div>
								))}
							</section>
							<GraphChoices />
							{GROUPS.map((group) => ({ group, ...splitByOwner(group.fields, serverOwned) })).map(({ group, shown, decided }) => (
								<section key={group.title} className="flex flex-col gap-2">
									<Heading>{group.title}</Heading>
									{group.note && <p className="text-xs text-muted-foreground">{group.note}</p>}
									{group.title === "Which papers" && (
										<div className="flex items-center justify-between gap-4 py-1">
											<span className="text-xs text-muted-foreground">
												Read what the authors of every paper of the open run declare. It downloads each paper&apos;s PDF, so a whole
												conference takes hours; it can be stopped and continued.
											</span>
											<Button variant="outline" className="w-36" disabled={!onReadDeclarations} onClick={() => onReadDeclarations?.()}>
												<FileSearch />
												Read them all
											</Button>
										</div>
									)}
									{group.title === "Embeddings" && machine && decided.length === 0 && (
										<p className={cn("text-xs", machine.gpu ? "text-muted-foreground" : "text-warning")}>{machine.embedding}</p>
									)}
									{decided.map((field) => (
										<p key={field.key} className="text-xs text-muted-foreground">
											{field.label}: decided by the server this window is connected to.
										</p>
									))}
									{group.title === AI_GROUP && detector && (
										<p className={cn("text-xs", detector.state === "ready" ? "text-muted-foreground" : "text-warning")}>{detector.why}</p>
									)}
									{group.title === AI_GROUP && detector && detector.state !== "off" && (
										<DetectorTraining state={detector.state} onChanged={() => onDetectorChanged?.()} />
									)}
									{group.title === AI_GROUP &&
										validation &&
										validation.written_caught !== null &&
										validation.polished_caught !== null &&
										(detector?.state === "ready" || detector?.state === "stale") && (
											<p className="text-xs text-warning">
												Measured on {validation.held_out} abstracts it was not trained on: it catches{" "}
												{Math.round(100 * validation.written_caught)}% of abstracts a model wrote from the title and{" "}
												{Math.round(100 * validation.polished_caught)}% of human abstracts a model polished, while tagging{" "}
												{Math.round(100 * validation.false_alarms)}% of human ones. Trained on text from{" "}
												{validation.generators.join(" and ")} only.
												{validation.by_year && (
													<>
														{" "}
														Of a week of arXiv abstracts in each year it tags:{" "}
														{Object.entries(validation.by_year)
															.map(([year, found]) => `${year} ${Math.round(100 * found.flagged)}%`)
															.join(", ")}
														. ChatGPT appeared at the end of 2022.
													</>
												)}
											</p>
										)}
									<div className={group.columns ? "grid grid-cols-2 gap-2" : "flex flex-col gap-2"}>
										{shown.map((field) => {
											const why = refusalOf(field, refused);
											return (
												<div key={field.key} className="flex flex-col gap-1">
													<Editor
														field={field}
														settings={settings}
														catalog={catalog}
														choosable={choosable}
														providerOwned={serverOwned.has(
															field.kind === "model" && field.role === "reading" ? "reading_provider" : "describe_provider",
														)}
														onChange={onChange}
													/>
													{why && (
														<p role="alert" className="text-xs text-destructive">
															Not saved: {why}
														</p>
													)}
												</div>
											);
										})}
									</div>
								</section>
							))}
							{importProblems && importProblems.length > 0 && (
								<section className="flex flex-col gap-2">
									<Heading>The earlier version&apos;s data</Heading>
									{importProblems.map((problem) => (
										<p key={problem} className="text-xs text-warning">
											{problem}
										</p>
									))}
									<p className="text-xs text-muted-foreground">{IMPORT_ADVICE}</p>
								</section>
							)}
							<section className="flex flex-col gap-2">
								<Heading>{MODELS_GROUP}</Heading>
								{serverOwned.has("describe_provider") && (
									<p className="text-xs text-muted-foreground">
										The provider is decided by the server this window is connected to: its key and account are the server&apos;s. Its models
										can be chosen above.
									</p>
								)}
								<LanguageModels choosable={choosable} />
							</section>
						</div>
					)}
				</Dialog.Content>
			</Dialog.Portal>
		</Dialog.Root>
	);
}
