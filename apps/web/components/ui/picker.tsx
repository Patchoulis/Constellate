"use client";

import { DropdownMenu } from "radix-ui";
import { Check, ChevronDown, Minus } from "lucide-react";
import type { CatalogGroup, CatalogItem } from "@rp/contracts";
import { cn } from "@/lib/utils";

/** A tick box as drawn inside a menu: empty, ticked, or a dash for "some of the group". */
function Tick({ state }: { state: "none" | "some" | "all" }) {
	return (
		<span
			className={cn(
				"flex size-3.5 shrink-0 items-center justify-center rounded-sm border border-input",
				state !== "none" && "border-foreground bg-foreground text-background",
			)}
		>
			{state === "all" && <Check className="size-3" />}
			{state === "some" && <Minus className="size-3" />}
		</span>
	);
}

const ROW = "flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1 text-sm outline-none data-[highlighted]:bg-accent";
const MENU =
	"z-50 max-h-[min(28rem,var(--radix-dropdown-menu-content-available-height))] overflow-y-auto rounded-md border bg-popover p-1 shadow-xl";

/** What a menu opens from: looks like a text field, shows what is chosen. `quiet` is for a menu inside a line of
 *  small text (a status line): no box, the line's own size. */
function Trigger({ text, empty, count, quiet }: { text: string; empty: boolean; count?: number; quiet?: boolean }) {
	return (
		<DropdownMenu.Trigger
			className={
				quiet
					? "-mx-1 flex min-w-0 items-center gap-1 rounded-sm px-1 text-left outline-none hover:bg-accent hover:text-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground"
					: "flex h-8 w-full min-w-0 items-center gap-2 rounded-md border border-input bg-background px-2 text-left text-sm data-[state=open]:border-ring"
			}
		>
			<span className={cn("min-w-0 flex-1 truncate", empty && "text-muted-foreground/60")}>{text}</span>
			{count !== undefined && count > 1 && <span className="text-xs text-muted-foreground tabular-nums">{count}</span>}
			<ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
		</DropdownMenu.Trigger>
	);
}

/** One line of a menu: a name and, right-aligned and quieter, a few words about it. */
function Line({ item }: { item: CatalogItem }) {
	return (
		<>
			<span className="shrink-0">{item.label ?? item.name}</span>
			<span className="min-w-0 flex-1 truncate text-right text-xs text-muted-foreground" title={item.note || undefined}>
				{item.note}
			</span>
		</>
	);
}

/**
 * Pick any number of things from a menu grouped by field. A field's heading ticks or clears the whole field.
 * The menu stays open while picking.
 */
export function GroupedPicker({
	groups,
	picked,
	onChange,
	placeholder,
}: {
	groups: CatalogGroup[];
	picked: string[];
	onChange: (picked: string[]) => void;
	placeholder: string;
}) {
	const chosen = new Set(picked);
	const toggle = (names: string[], on: boolean) => {
		const next = new Set(picked);
		for (const name of names) on ? next.add(name) : next.delete(name);
		// keep the menu's order, so the same picks always read (and name a run) the same way
		onChange(groups.flatMap((group) => group.items.map((item) => item.name)).filter((name) => next.has(name)));
	};
	const stay = (event: Event) => event.preventDefault(); // picking does not close the menu

	return (
		<DropdownMenu.Root>
			<Trigger
				text={
					picked.length
						? groups
								.flatMap((group) => group.items)
								.filter((item) => chosen.has(item.name))
								.map((item) => item.label ?? item.name)
								.join(", ")
						: placeholder
				}
				empty={picked.length === 0}
				count={picked.length}
			/>
			<DropdownMenu.Portal>
				<DropdownMenu.Content align="start" sideOffset={4} className={cn(MENU, "w-80")}>
					{groups.map((group) => {
						const names = group.items.map((item) => item.name);
						const count = names.filter((name) => chosen.has(name)).length;
						const state = count === 0 ? "none" : count === names.length ? "all" : "some";
						return (
							<DropdownMenu.Group key={group.field} className="mb-1 last:mb-0">
								<DropdownMenu.Item
									className={cn(ROW, "text-xs tracking-widest text-muted-foreground uppercase")}
									onSelect={(event) => {
										stay(event);
										toggle(names, state !== "all");
									}}
								>
									<Tick state={state} />
									{group.field}
								</DropdownMenu.Item>
								{group.items.map((item) => (
									<DropdownMenu.Item
										key={item.name}
										className={ROW}
										onSelect={(event) => {
											stay(event);
											toggle([item.name], !chosen.has(item.name));
										}}
									>
										<Tick state={chosen.has(item.name) ? "all" : "none"} />
										<Line item={item} />
									</DropdownMenu.Item>
								))}
							</DropdownMenu.Group>
						);
					})}
				</DropdownMenu.Content>
			</DropdownMenu.Portal>
		</DropdownMenu.Root>
	);
}

/** Choose exactly one of a few options. A value that is not among them (set by hand) is still shown. */
export function Choice({
	options,
	value,
	onChange,
	quiet,
}: {
	options: CatalogItem[];
	value: string;
	onChange: (value: string) => void;
	/** drawn as part of a line of small text rather than as a field */
	quiet?: boolean;
}) {
	return (
		<DropdownMenu.Root>
			<Trigger text={options.find((item) => item.name === value)?.label ?? value} empty={false} quiet={quiet} />
			<DropdownMenu.Portal>
				<DropdownMenu.Content align="start" sideOffset={4} className={cn(MENU, "w-(--radix-dropdown-menu-trigger-width) min-w-80")}>
					{options.map((item) => (
						<DropdownMenu.Item key={item.name} className={ROW} onSelect={() => onChange(item.name)}>
							<Check className={cn("size-3.5 shrink-0", item.name !== value && "invisible")} />
							<Line item={item} />
						</DropdownMenu.Item>
					))}
				</DropdownMenu.Content>
			</DropdownMenu.Portal>
		</DropdownMenu.Root>
	);
}
