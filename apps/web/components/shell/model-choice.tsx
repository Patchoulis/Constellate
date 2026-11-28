"use client";

import { createContext, useContext, useState } from "react";
import { DropdownMenu } from "radix-ui";
import { Check } from "lucide-react";
import type { Effort, ProviderId, Settings } from "@rp/contracts";
import { api } from "@/lib/net/api";
import {
	EFFORT_LABEL,
	READING_TITLE,
	changeFor,
	chosenLabel,
	chosenOf,
	effortChange,
	effortSent,
	effortsOf,
	endpointIds,
	groupsOf,
	isPlainId,
	priceLabel,
	priceOf,
	readingPicks,
	readingEntries,
	readingStateOf,
	unavailableBecause,
	type Choosable,
	type ModelGroup,
	type ModelRole,
	type ModelSettings,
} from "@/lib/shell/models";
import { count, type Currency } from "@/lib/units";
import { MENU, ROW, Trigger } from "@/components/ui/picker";
import { cn } from "@/lib/utils";

type Picked = ModelSettings;

/** How long the server's list of models is waited for. */
const LISTING_MS = 10_000;

/** Asking the server which models it has: one state at a time. */
type Listing =
	{ state: "idle" } | { state: "asking" } | { state: "listed"; ids: string[]; dropped: number } | { state: "failed"; why: string };

/** The server's models, or why there are none: a server that is down, slow, or answers something else. */
async function listEndpoint(): Promise<Listing> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const slow = new Promise<never>((_, reject) => {
		timer = setTimeout(() => reject(new Error(`The server did not answer within ${LISTING_MS / 1000} seconds.`)), LISTING_MS);
	});
	try {
		const found = endpointIds(await Promise.race([api.endpointModels(), slow]));
		if (!found) return { state: "failed", why: "The server's answer is not a list of models." };
		return { state: "listed", ...found };
	} catch (error) {
		return { state: "failed", why: (error as Error).message };
	} finally {
		clearTimeout(timer);
	}
}

/** A model id the provider does not list, typed in; for a server, the ids it says it has can be listed first. */
function Typed({ group, onPick }: { group: ModelGroup; onPick: (id: string) => void }) {
	const [draft, setDraft] = useState("");
	const [listing, setListing] = useState<Listing>({ state: "idle" });
	const plain = isPlainId(draft.trim());
	const use = () => plain && onPick(draft.trim());
	return (
		<div className="flex flex-col gap-1 px-2 py-1" onKeyDown={(event) => event.stopPropagation()}>
			<div className="flex gap-1">
				<input
					className="h-7 min-w-0 flex-1 rounded-sm border border-input bg-background px-2 text-sm"
					placeholder={group.provider.id === "compatible" ? "model id, e.g. qwen3:32b" : "another model id"}
					value={draft}
					onChange={(event) => setDraft(event.target.value)}
					onKeyDown={(event) => event.key === "Enter" && use()}
				/>
				<button type="button" className="rounded-sm border px-2 text-xs disabled:opacity-50" disabled={!plain} onClick={use}>
					Use
				</button>
			</div>
			{draft.trim() && !plain && (
				<span className="text-xs text-warning">
					A model id is letters, digits and . _ / : - only, at most 200 characters, and not a path.
				</span>
			)}
			{group.provider.id === "compatible" && (
				<button
					type="button"
					className="self-start text-xs text-muted-foreground underline disabled:no-underline disabled:opacity-50"
					disabled={listing.state === "asking"}
					onClick={() => {
						setListing({ state: "asking" });
						void listEndpoint().then(setListing);
					}}
				>
					{listing.state === "asking" ? "Asking the server…" : "List the server's models"}
				</button>
			)}
			{listing.state === "failed" && <span className="text-xs text-warning">{listing.why}</span>}
			{listing.state === "listed" &&
				listing.ids.map((id) => (
					<button key={id} type="button" className="truncate text-left text-sm hover:underline" onClick={() => onPick(id)}>
						{id}
					</button>
				))}
			{listing.state === "listed" && listing.ids.length === 0 && (
				<span className="text-xs text-muted-foreground">The server lists no models it can be asked by.</span>
			)}
			{listing.state === "listed" && listing.dropped > 0 && (
				<span className="text-xs text-muted-foreground">
					{count(listing.dropped, "entry", "entries")} of its list left out: not a model id that can be used.
				</span>
			)}
		</div>
	);
}

/**
 * A language model: providers in groups, each model with a short label and a note, those that cannot be used
 * greyed with what to set, an id typed where the provider takes one; and beside it the effort, where the model has
 * one. `role` says which of the settings' two choices it shows and changes: the naming model, or the model that
 * reads papers (whose menu also offers to be the naming model again). `providerOwned`: the provider is the
 * server's, and only its models may be picked.
 */
export function ModelChoice({
	choosable,
	settings,
	providerOwned,
	currency,
	locale,
	quiet,
	role = "naming",
	onChange,
}: {
	choosable: Choosable;
	settings: Picked | undefined;
	providerOwned: boolean;
	currency: Currency;
	locale?: string;
	quiet?: boolean;
	role?: ModelRole;
	onChange: (changes: Partial<Settings>) => void;
}) {
	const chosen = chosenOf(choosable.catalog, settings, role);
	// until the engine has told its providers, nothing but the settings' own model is shown, and nothing is offered
	const fixed = providerOwned || choosable.source === "waiting";
	const pick = (provider: ProviderId, id: string) => onChange(changeFor(provider, id, fixed, role));
	const efforts = effortsOf(chosen);
	const sent = effortSent(chosen.effort, efforts);
	const priced = choosable.source === "engine";
	// none chosen to read (the default, or the naming model): no model below is ticked, an entry above is
	const same = role === "reading" && readingStateOf(settings) !== "chosen";
	const entries = role === "reading" ? readingEntries(choosable.catalog, settings) : [];
	// a server's window: the model that reads papers is the owner's to choose, as readingPicks says
	const allowed = readingPicks(choosable.catalog, settings, providerOwned);
	const locked = role === "reading" && allowed.locked;
	const canReset = role === "reading" && choosable.source === "engine" && allowed.canReset;
	return (
		<span className="flex min-w-0 items-center gap-1.5">
			<DropdownMenu.Root>
				<Trigger text={chosenLabel(chosen)} empty={false} quiet={quiet} />
				<DropdownMenu.Portal>
					<DropdownMenu.Content align="start" sideOffset={4} className={cn(MENU, "w-[min(24rem,calc(100vw-1rem))]")}>
						{entries.length > 0 && (
							<DropdownMenu.Group className="mb-1">
								{entries.map((entry) => (
									<DropdownMenu.Item
										key={entry.label}
										disabled={!canReset}
										className={cn(ROW, !canReset && "cursor-default opacity-50")}
										onSelect={() => onChange(entry.change)}
									>
										<Check className={cn("size-3.5 shrink-0", !entry.ticked && "invisible")} />
										<span className="min-w-0 truncate">{entry.label}</span>
										<span className="min-w-0 flex-1 truncate text-right text-xs text-muted-foreground">{entry.note}</span>
									</DropdownMenu.Item>
								))}
								<DropdownMenu.Separator className="my-1 h-px bg-border" />
							</DropdownMenu.Group>
						)}
						{groupsOf(choosable.catalog).map((group) => {
							const off = locked || group.why !== null || (fixed && group.provider.id !== chosen.providerId);
							return (
								<DropdownMenu.Group key={group.provider.id} className="mb-1 last:mb-0">
									<DropdownMenu.Label className="flex items-baseline justify-between gap-2 px-2 pt-1 text-xs tracking-widest text-muted-foreground uppercase">
										<span>{group.provider.label}</span>
										{group.why && <span className="text-[11px] tracking-normal normal-case text-warning">{group.why}</span>}
										{!group.why && fixed && off && <span className="text-[11px] tracking-normal normal-case">decided by the server</span>}
									</DropdownMenu.Label>
									{group.models.map((model) => (
										<DropdownMenu.Item
											key={model.id}
											disabled={off}
											className={cn(ROW, off && "cursor-default opacity-50")}
											title={(priced && priceLabel(model.price, currency, locale)) || undefined}
											onSelect={() => pick(group.provider.id, model.id)}
										>
											<Check
												className={cn(
													"size-3.5 shrink-0",
													(same || !(chosen.providerId === group.provider.id && chosen.id === model.id)) && "invisible",
												)}
											/>
											<span className="shrink-0">{model.label}</span>
											<span className="min-w-0 flex-1 truncate text-right text-xs text-muted-foreground">{model.note}</span>
										</DropdownMenu.Item>
									))}
									{group.typed && !off && <Typed group={group} onPick={(id) => pick(group.provider.id, id)} />}
								</DropdownMenu.Group>
							);
						})}
					</DropdownMenu.Content>
				</DropdownMenu.Portal>
			</DropdownMenu.Root>
			{efforts.length > 0 && sent && !locked && (
				<DropdownMenu.Root>
					<Trigger text={EFFORT_LABEL[sent] ?? sent} empty={false} quiet={quiet} />
					<DropdownMenu.Portal>
						<DropdownMenu.Content align="start" sideOffset={4} className={cn(MENU, "w-48")}>
							{efforts.map((effort: Effort) => (
								<DropdownMenu.Item key={effort} className={ROW} onSelect={() => onChange(effortChange(effort, role))}>
									<Check className={cn("size-3.5 shrink-0", effort !== sent && "invisible")} />
									{EFFORT_LABEL[effort] ?? effort}
								</DropdownMenu.Item>
							))}
						</DropdownMenu.Content>
					</DropdownMenu.Portal>
				</DropdownMenu.Root>
			)}
		</span>
	);
}

/** The chosen model's price in words, for beside the cost; null (nothing shown) when it has no price. */
export function chosenPrice(
	choosable: Choosable,
	settings: Picked | undefined,
	currency: Currency,
	locale?: string,
	role: ModelRole = "naming",
): string | null {
	if (choosable.source !== "engine") return null;
	const chosen = chosenOf(choosable.catalog, settings, role);
	return priceLabel(priceOf(chosen.provider, chosen.id), currency, locale);
}

/** What a menu of the model that reads papers needs from the page: the models, the settings, who owns the
 *  provider, and how a change is saved (app/page.tsx provides it). */
export interface ModelChoosing {
	choosable: Choosable;
	settings: Picked | undefined;
	providerOwned: boolean;
	onChange: (changes: Partial<Settings>) => void;
}

export const ModelChoosingContext = createContext<ModelChoosing | null>(null);

/** The dollar: what the features' estimates are told in, and so the prices beside them. */
const DOLLAR: Currency = { code: "USD", rate: 1, date: null };

/**
 * The model that reads papers, as a quiet menu on a line of its own: for a section that reads papers, beside what
 * asking would cost, where the choice is made. It is the one shared setting (its title says so): changing it here
 * changes it for every such section, and the estimates are asked for again for the new model.
 */
export function ReadingModel({ className }: { className?: string }) {
	const choosing = useContext(ModelChoosingContext);
	if (!choosing) return null;
	const chosen = chosenOf(choosing.choosable.catalog, choosing.settings, "reading");
	const why = chosen.provider && unavailableBecause(chosen.provider);
	return (
		<div
			className={cn("flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs text-muted-foreground", className)}
			title={READING_TITLE}
		>
			<span className="shrink-0">Model that reads papers:</span>
			<span className="flex max-w-full min-w-0 items-center">
				<ModelChoice
					quiet
					role="reading"
					choosable={choosing.choosable}
					settings={choosing.settings}
					providerOwned={choosing.providerOwned}
					currency={DOLLAR}
					onChange={choosing.onChange}
				/>
			</span>
			{why && <span className="min-w-0 text-warning">· {why}</span>}
		</div>
	);
}
