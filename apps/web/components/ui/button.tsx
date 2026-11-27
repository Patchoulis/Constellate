import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
	"inline-flex shrink-0 items-center justify-center gap-2 rounded-md text-sm whitespace-nowrap transition-colors disabled:opacity-40 [&_svg]:size-3.5 [&_svg]:shrink-0",
	{
		variants: {
			variant: {
				default: "bg-primary text-primary-foreground hover:bg-primary/85",
				outline: "border border-input bg-transparent hover:bg-accent",
				ghost: "text-muted-foreground hover:bg-accent hover:text-foreground",
			},
			size: {
				default: "h-8 px-3",
				icon: "size-7",
			},
		},
		defaultVariants: { variant: "default", size: "default" },
	},
);

export function Button({ className, variant, size, ...props }: React.ComponentProps<"button"> & VariantProps<typeof buttonVariants>) {
	return <button className={cn(buttonVariants({ variant, size }), className)} {...props} />;
}

/** A full-width, left-aligned row that can be clicked: list entries everywhere in the app. */
export function RowButton({ className, active, ...props }: React.ComponentProps<"button"> & { active?: boolean }) {
	return (
		<button
			className={cn(
				"flex w-full min-w-0 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent",
				active && "bg-accent",
				className,
			)}
			{...props}
		/>
	);
}

/** A small uppercase heading over a group of things. */
export function Heading({ className, ...props }: React.ComponentProps<"h3">) {
	return <h3 className={cn("text-xs tracking-widest text-muted-foreground uppercase", className)} {...props} />;
}

/** Pick one of a few options: the app's tabs, mode switches and sort keys. */
export function Segmented<T extends string>({
	options,
	value,
	onChange,
	className,
}: {
	options: readonly { value: T; label: string; title?: string; icon?: React.ReactNode }[];
	value: T;
	onChange: (value: T) => void;
	className?: string;
}) {
	return (
		<div className={cn("inline-flex rounded-md border p-0.5", className)}>
			{options.map((option) => (
				<button
					key={option.value}
					type="button"
					title={option.title}
					// which one is chosen, for those who do not see the highlight
					aria-pressed={value === option.value}
					onClick={() => onChange(option.value)}
					className={cn(
						"flex h-6 flex-1 items-center justify-center gap-1.5 rounded-sm px-2.5 text-xs transition-colors [&_svg]:size-3",
						value === option.value ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
					)}
				>
					{option.icon}
					{option.label}
				</button>
			))}
		</div>
	);
}
