import type { Scale } from "@rp/contracts";
import { Star } from "lucide-react";
import { starsOf } from "@/lib/units";
import { cn } from "@/lib/utils";

/**
 * A score as stars out of its scale's points: grey outlines, and over them as much of the row filled as the score.
 * Nothing for a scale too long for stars (the words beside it say the score). `size` is the stars' size class.
 */
export function Stars({ value, scale, label, size = "size-3.5" }: { value: number; scale: Scale; label: string; size?: string }) {
	const stars = starsOf(value, scale);
	if (!stars) return null;
	const row = (filled: boolean) =>
		Array.from({ length: stars.total }, (_, index) => (
			<Star key={index} className={cn(size, "shrink-0")} fill={filled ? "currentColor" : "none"} />
		));
	return (
		<span role="img" aria-label={label} className="relative inline-flex shrink-0 align-middle">
			<span className="flex text-muted-foreground/40">{row(false)}</span>
			<span
				className="absolute inset-y-0 left-0 flex overflow-hidden text-mark"
				style={{ width: `${(100 * stars.filled) / stars.total}%` }}
			>
				{row(true)}
			</span>
		</span>
	);
}
