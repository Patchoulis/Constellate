"use client";

import { Switch as SwitchPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

export function Input({ className, ...props }: React.ComponentProps<"input">) {
	return (
		<input
			className={cn(
				"h-8 w-full min-w-0 rounded-md border border-input bg-background px-2 text-sm text-foreground placeholder:text-muted-foreground/60 focus:border-ring",
				className,
			)}
			{...props}
		/>
	);
}

export function Label({ className, ...props }: React.ComponentProps<"label">) {
	return <label className={cn("flex flex-col gap-1 text-xs text-muted-foreground", className)} {...props} />;
}

export function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
	return (
		<SwitchPrimitive.Root
			className={cn(
				"inline-flex h-4.5 w-8 shrink-0 items-center rounded-full border border-input bg-background transition-colors data-[state=checked]:border-foreground data-[state=checked]:bg-foreground",
				className,
			)}
			{...props}
		>
			<SwitchPrimitive.Thumb className="block size-3 translate-x-0.5 rounded-full bg-muted-foreground transition-transform data-[state=checked]:translate-x-4 data-[state=checked]:bg-background" />
		</SwitchPrimitive.Root>
	);
}

/** A switch with its label and an optional one-line explanation. */
export function SwitchRow({
	label,
	hint,
	checked,
	onCheckedChange,
}: {
	label: string;
	hint?: string;
	checked: boolean;
	onCheckedChange: (checked: boolean) => void;
}) {
	return (
		<label className="flex cursor-pointer items-start justify-between gap-4 py-1.5">
			<span className="flex flex-col gap-0.5">
				<span className="text-sm text-foreground">{label}</span>
				{hint && <span className="text-xs text-muted-foreground">{hint}</span>}
			</span>
			<Switch checked={checked} onCheckedChange={onCheckedChange} className="mt-0.5" />
		</label>
	);
}
