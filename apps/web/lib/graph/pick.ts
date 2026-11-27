import type { Point } from "@/lib/graph/layout";

/**
 * The paper under a point of the world: the nearest one whose dot, `radius` x its size, plus `margin` (all in world
 * units) reaches the point, among those `pickable`; null if none does. Of two equally near, the first.
 *
 * Every paper is looked at: at 12,500 papers that takes about a tenth of a millisecond (tests/perf.bench.ts), far
 * less than a frame, so no spatial index is kept.
 */
export function nearestPaper(
	points: readonly Point[],
	sizes: ArrayLike<number>,
	at: Point,
	radius: number,
	margin: number,
	pickable: (index: number) => boolean,
): number | null {
	let best = Infinity;
	let found: number | null = null;
	for (let index = 0; index < points.length; index++) {
		const point = points[index];
		const distance = (point.x - at.x) ** 2 + (point.y - at.y) ** 2;
		if (distance < best && distance < (radius * sizes[index] + margin) ** 2 && pickable(index)) {
			best = distance;
			found = index;
		}
	}
	return found;
}
