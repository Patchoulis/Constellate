import { Heading } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * One block of a community's detail: its small header (with an optional note on the right), a hairline, then
 * what it holds. Every block of the panel is one of these, so that they read alike.
 */
export function Block({
	title,
	note,
	noteTitle,
	children,
	className,
}: {
	title: string;
	/** a figure or a word beside the header: "3 papers" */
	note?: React.ReactNode;
	noteTitle?: string;
	children: React.ReactNode;
	className?: string;
}) {
	return (
		<section className={cn("flex min-w-0 flex-col", className)} aria-label={title}>
			<div className="flex min-w-0 items-baseline justify-between gap-2 pb-1">
				<Heading className="min-w-0 [overflow-wrap:anywhere]">{title}</Heading>
				{note && (
					<span className="shrink-0 text-xs text-muted-foreground tabular-nums" title={noteTitle}>
						{note}
					</span>
				)}
			</div>
			<div className="min-w-0 border-t pt-2 [overflow-wrap:anywhere]">{children}</div>
		</section>
	);
}

/** A header inside a block, one step quieter than the block's own, with its own hairline: a description's sections. */
export function SubBlock({ title, children, className }: { title: string; children: React.ReactNode; className?: string }) {
	return (
		<section className={cn("flex min-w-0 flex-col", className)} aria-label={title}>
			<h4 className="pb-0.5 text-[0.6875rem] tracking-wider text-muted-foreground/80 uppercase [overflow-wrap:anywhere]">{title}</h4>
			<div className="min-w-0 border-t border-border/60 pt-1.5">{children}</div>
		</section>
	);
}

/** A small label over a list inside a block ("Closest papers"), with what it is on hover. */
export function ListLabel({ children, title }: { children: React.ReactNode; title?: string }) {
	return (
		<div className="mt-2 mb-1 text-[0.6875rem] text-muted-foreground" title={title}>
			{children}
		</div>
	);
}
