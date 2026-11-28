"use client";

/**
 * Words that say something is under way, with three dots after them that come one by one and go round again
 * (lib/ui/loading-dots.ts), in the pale colour of the loading sky's stars. The dots always take the room of three,
 * so the words never move. A screen reader hears the words once; the dots are decoration. With less motion, the
 * words have a plain "…" and simply appear.
 */
import { useEffect, useRef, useState } from "react";
import { prefersLessMotion } from "@/lib/graph/glide";
import { STAR_COLOUR } from "@/lib/graph/sky";
import { dotOpacity } from "@/lib/ui/loading-dots";
import { cn } from "@/lib/utils";

/** The three dots alone, for after words. */
export function LoadingDots() {
	const dots = useRef<(HTMLSpanElement | null)[]>([]);
	const [still, setStill] = useState(false);
	useEffect(() => {
		if (prefersLessMotion()) return setStill(true);
		const began = performance.now();
		let frame = 0;
		const step = () => {
			const elapsed = performance.now() - began;
			dots.current.forEach((dot, index) => {
				if (dot) dot.style.opacity = String(dotOpacity(index, elapsed));
			});
			frame = document.hidden ? 0 : requestAnimationFrame(step);
		};
		// nothing is drawn while the window or tab is hidden
		const onVisibility = () => {
			cancelAnimationFrame(frame);
			frame = document.hidden ? 0 : requestAnimationFrame(step);
		};
		document.addEventListener("visibilitychange", onVisibility);
		step();
		return () => {
			cancelAnimationFrame(frame);
			document.removeEventListener("visibilitychange", onVisibility);
		};
	}, []);
	if (still) return <span aria-hidden>…</span>;
	return (
		<span aria-hidden className="inline-flex" style={{ color: STAR_COLOUR }}>
			{[0, 1, 2].map((index) => (
				<span key={index} ref={(dot) => void (dots.current[index] = dot)} style={{ opacity: index === 0 ? 1 : 0 }}>
					.
				</span>
			))}
		</span>
	);
}

/** `words` with the dots after them and `after` following ("Working… the graph appears when the run finishes."),
 *  announced once, as one status. They fade and rise in as they appear. */
export function LoadingWords({ words, after, className }: { words: string; after?: string; className?: string }) {
	return (
		<span role="status" className={cn("enter-rise", className)}>
			<span className="sr-only">{`${words}…${after ?? ""}`}</span>
			<span aria-hidden>
				{words}
				<LoadingDots />
				{after}
			</span>
		</span>
	);
}
