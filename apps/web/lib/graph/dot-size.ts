/**
 * How large a paper's dot is drawn. Its true size in world units, but never under about a pixel on screen: zoomed out,
 * the dot keeps that pixel and its radius in world units grows as the zoom falls.
 *
 * The renderer keeps the dots as paths made for one radius (components/graph/graph-canvas.ts `dotsOf`), and making
 * them for 12,500 papers takes several milliseconds. So below its true size the radius goes in steps of a sixteenth
 * of an octave of zoom: a zoom remakes the paths every 4% or so instead of on every frame, and a dot is never more than
 * about 2% off the size it would have had (a hundredth of a pixel at that size).
 */

/** The smallest radius a dot has on screen, in CSS pixels. */
export const SMALLEST_DOT_PX = 0.85;
/** Steps of the radius per doubling of the zoom, below a dot's true size. */
const STEPS_PER_OCTAVE = 16;

/** A dot's radius in world units at zoom `k`, for a true radius of `radius` world units. */
export function screenDotRadius(radius: number, k: number): number {
	const smallest = SMALLEST_DOT_PX / k;
	if (!(smallest > radius)) return radius;
	const steps = Math.round(Math.log2(smallest / radius) * STEPS_PER_OCTAVE);
	return radius * 2 ** (steps / STEPS_PER_OCTAVE);
}
