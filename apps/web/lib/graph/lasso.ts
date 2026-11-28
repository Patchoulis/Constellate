/**
 * A lasso drawn over the graph: a closed line through the places the pointer went, and which papers are inside it.
 * Pure: the places are numbers in whatever frame the caller draws in (the canvas, in CSS pixels).
 */

export interface Place {
	x: number;
	y: number;
}

/** Shorter than this, in pixels, a press and release with the select key held is a click, not a lasso. */
export const LASSO_MIN = 6;

/** How far the pointer went from where it was pressed, at most: what tells a lasso from a click. */
export function reach(line: readonly Place[]): number {
	const [start] = line;
	return start ? Math.max(...line.map((place) => Math.hypot(place.x - start.x, place.y - start.y))) : 0;
}

/** Whether a place is inside the closed line (even-odd: a line crossing itself leaves its crossing's inside out). */
export function inside(line: readonly Place[], x: number, y: number): boolean {
	let within = false;
	for (let index = 0, before = line.length - 1; index < line.length; before = index++) {
		const a = line[index];
		const b = line[before];
		if (a.y > y !== b.y > y && x < ((b.x - a.x) * (y - a.y)) / (b.y - a.y) + a.x) within = !within;
	}
	return within;
}

/** The indexes of the places inside the closed line, in order. A line of fewer than three places holds nothing. */
export function lassoed(line: readonly Place[], places: readonly Place[], among: (index: number) => boolean = () => true): number[] {
	if (line.length < 3) return [];
	const xs = line.map((place) => place.x);
	const ys = line.map((place) => place.y);
	const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
	const found: number[] = [];
	places.forEach((place, index) => {
		if (place.x < minX || place.x > maxX || place.y < minY || place.y > maxY) return;
		if (among(index) && inside(line, place.x, place.y)) found.push(index);
	});
	return found;
}
