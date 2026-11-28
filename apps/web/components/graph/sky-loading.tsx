"use client";

/**
 * A night sky behind the words shown while a run is loaded or made: slowly twinkling stars and a small
 * constellation that draws itself, again and again (lib/graph/sky.ts). It fills the box it is put in and draws
 * nothing else: the background is the page's own, and the words stay on top of it.
 */
import { useEffect, useRef } from "react";
import { prefersLessMotion } from "@/lib/graph/glide";
import {
	FIELD_STARS,
	FIELD_TILE,
	FIGURES,
	STAR_COLOUR as STAR,
	figureFrame,
	fieldStars,
	skyFigure,
	skyStarCount,
	starLit,
	tilesCovering,
	twinkle,
} from "@/lib/graph/sky";
import { usePrefs } from "@/lib/ui/prefs";
import { cn } from "@/lib/utils";

const LINE = "rgba(255, 255, 255, 0.55)";
/** How bright an unlit star is, as a share of its brightness, while a run being made has not reached it. */
const UNLIT = 0.18;
/** The sky is the field behind the graph, a little brighter while it is all there is to see. */
const BRIGHTER = 1.5;

const FIELD = fieldStars(FIELD_STARS.many);

export function SkyLoading({ share = null, className }: { share?: number | null; className?: string }) {
	const canvasRef = useRef<HTMLCanvasElement>(null);
	// the share changes while the run is made, and the stars with Settings; the loop reads them without starting again
	const shareRef = useRef(share);
	shareRef.current = share;
	const { stars } = usePrefs();
	const fieldRef = useRef(stars);
	fieldRef.current = stars;

	useEffect(() => {
		const canvas = canvasRef.current;
		const context = canvas?.getContext("2d");
		if (!canvas || !context) return;
		const still = prefersLessMotion();
		let frame = 0;
		let dpr = 1;

		const draw = () => {
			const width = canvas.clientWidth;
			const height = canvas.clientHeight;
			// the page's clock, not this sky's: a sky shown again (the run made, now loading) goes on where it was
			const ms = still ? 0 : performance.now();
			context.setTransform(dpr, 0, 0, dpr, 0, 0);
			context.clearRect(0, 0, width, height);
			// the field behind the graph, where the graph will have it when it is first shown (no pan yet)
			const count = skyStarCount(fieldRef.current);
			context.fillStyle = STAR;
			for (const tile of tilesCovering(width, height, { x: 0, y: 0 }))
				for (let index = 0; index < count; index++) {
					const shine = Math.min(1, FIELD.bright[index] * BRIGHTER);
					const bright = still ? shine : twinkle(shine, FIELD.phase[index], ms);
					context.globalAlpha = starLit(index, shareRef.current) ? bright : bright * UNLIT;
					const size = FIELD.size[index];
					const x = tile.x + FIELD.place[index * 2] * FIELD_TILE;
					const y = tile.y + FIELD.place[index * 2 + 1] * FIELD_TILE;
					context.fillRect(x - size / 2, y - size / 2, size, size);
				}
			const { figure, whole, part, alpha } = skyFigure(ms, still);
			if (alpha > 0) {
				const { stars: points, lines } = FIGURES[figure];
				const { x, y, unit } = figureFrame(width, height);
				const at = (star: number) => [x + points[star][0] * unit, y + points[star][1] * unit] as const;
				context.globalAlpha = alpha;
				context.strokeStyle = LINE;
				context.lineWidth = 1;
				context.beginPath();
				for (let line = 0; line < Math.min(lines.length, whole + 1); line++) {
					const share = line < whole ? 1 : part;
					if (share <= 0) break;
					const [ax, ay] = at(lines[line][0]);
					const [bx, by] = at(lines[line][1]);
					context.moveTo(ax, ay);
					context.lineTo(ax + (bx - ax) * share, ay + (by - ay) * share);
				}
				context.stroke();
				// a star of the figure shines once a line has reached it
				context.fillStyle = "#ffffff";
				context.beginPath();
				points.forEach((_, star) => {
					const reached = lines.some(
						([a, b], line) => (line < whole && (a === star || b === star)) || (line === whole && part > 0 && a === star),
					);
					if (!reached) return;
					const [sx, sy] = at(star);
					context.moveTo(sx + 1.8, sy);
					context.arc(sx, sy, 1.8, 0, Math.PI * 2);
				});
				context.fill();
			}
			context.globalAlpha = 1;
		};

		const loop = () => {
			frame = 0;
			draw();
			if (!still && !document.hidden) frame = requestAnimationFrame(loop);
		};
		// Sized and drawn in the same step: setting a canvas's size empties it, and a frame later the empty canvas
		// would have been painted (a panel dragged wider or narrower would make the sky flicker).
		const resize = () => {
			dpr = window.devicePixelRatio || 1;
			const width = Math.max(1, Math.round(canvas.clientWidth * dpr));
			const height = Math.max(1, Math.round(canvas.clientHeight * dpr));
			if (canvas.width !== width) canvas.width = width;
			if (canvas.height !== height) canvas.height = height;
			draw();
		};
		const observer = new ResizeObserver(resize);
		observer.observe(canvas);
		resize();
		// nothing is drawn while the window or tab is hidden; it goes on where the clock is once it is shown
		const onVisibility = () => {
			cancelAnimationFrame(frame);
			frame = 0;
			if (!document.hidden && !still) loop();
		};
		document.addEventListener("visibilitychange", onVisibility);
		if (!still) loop();
		return () => {
			cancelAnimationFrame(frame);
			observer.disconnect();
			document.removeEventListener("visibilitychange", onVisibility);
		};
	}, []);

	return <canvas ref={canvasRef} aria-hidden className={cn("pointer-events-none absolute inset-0 block size-full", className)} />;
}
