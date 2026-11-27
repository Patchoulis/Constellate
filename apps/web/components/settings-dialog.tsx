"use client";

import { Dialog } from "radix-ui";
import { FileSearch, RotateCcw, X } from "lucide-react";
import type { Catalog, Library, RedoStep, Settings } from "@rp/contracts";
import { Button, Heading } from "@/components/ui/button";
import { Input, Label, SwitchRow } from "@/components/ui/field";
import { Choice, GroupedPicker } from "@/components/ui/picker";
import { splitByOwner } from "@/lib/engine";
import { cn } from "@/lib/utils";

/** The whole dialog as data: every setting appears once here, with how it is edited. */
type Field = { key: keyof Settings; label: string; hint?: string } & (
	| { kind: "switch" }
	| { kind: "number"; step?: number }
	// one of a list the backend offers (Catalog), in a dropdown
	| { kind: "choice"; from: "embedding_models" | "describe_models" | "clustering" | "acceptance" }
	// any number of a list the backend offers
	| { kind: "several"; from: "flags"; placeholder: string }
);

/** Steps whose stored results can be thrown away and produced again, each as a button that does it now. */
const REDO: { step: RedoStep; label: string; hint: string }[] = [
	{ step: "download", label: "Download again", hint: "Fetch the paper list again and overwrite the stored records" },
	{ step: "embeddings", label: "Embed again", hint: "Recompute the embedding of every paper in the run" },
	{ step: "descriptions", label: "Describe again", hint: "Ask Claude for new names and descriptions" },
];

const AI_GROUP = "AI-written papers";

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
				label: "Describe communities with Claude",
				hint: "Needs ANTHROPIC_API_KEY in the backend's environment",
			},
			{ kind: "choice", key: "describe_model", label: "Claude model", from: "describe_models" },
		],
	},
];

function Editor({
	field,
	settings,
	catalog,
	onChange,
}: {
	field: Field;
	settings: Settings;
	catalog: Catalog | undefined;
	onChange: (changes: Partial<Settings>) => void;
}) {
	const set = (value: Settings[keyof Settings]) => onChange({ [field.key]: value });
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
	// Saved when the field is left, so a half-typed value never reaches the backend.
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
	onOpenChange,
	onChange,
	onRedo,
	onReadDeclarations,
}: {
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
	onOpenChange: (open: boolean) => void;
	onChange: (changes: Partial<Settings>) => void;
	/** Run the open run again with one step redone; undefined when there is no run to redo, or one is running. */
	onRedo?: (step: RedoStep) => void;
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
									described group is not described again. A button here runs the open run again with that step done from scratch.
								</p>
								{REDO.map(({ step, label, hint }) => (
									<div key={step} className="flex items-center justify-between gap-4 py-1">
										<span className="text-xs text-muted-foreground">{hint}</span>
										<Button variant="outline" className="w-36" disabled={!onRedo} onClick={() => onRedo?.(step)}>
											<RotateCcw />
											{label}
										</Button>
									</div>
								))}
							</section>
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
									{group.title === AI_GROUP && validation && detector?.state === "ready" && (
										<p className="text-xs text-warning">
											Measured on {validation.held_out} abstracts it was not trained on: it catches{" "}
											{Math.round(100 * validation.written_caught)}% of abstracts a model wrote from the title and{" "}
											{Math.round(100 * validation.polished_caught)}% of human abstracts a model polished, while tagging{" "}
											{Math.round(100 * validation.false_alarms)}% of human ones. Trained on text from {validation.generators.join(" and ")}{" "}
											only.
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
										{shown.map((field) => (
											<Editor key={field.key} field={field} settings={settings} catalog={catalog} onChange={onChange} />
										))}
									</div>
								</section>
							))}
						</div>
					)}
				</Dialog.Content>
			</Dialog.Portal>
		</Dialog.Root>
	);
}
