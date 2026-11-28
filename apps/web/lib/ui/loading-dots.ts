/**
 * The three dots after words that say something is under way ("Loading the run..."): one dot, two, three, then
 * round again. A dot that comes fades in, and the two that go at the end of a round fade out, so nothing pops.
 * This is only the timing; components/ui/loading-dots.tsx draws it.
 */

/** How long each step (one dot, two, three) lasts. */
export const DOT_STEP_MS = 450;
/** How long a dot takes to fade in. */
const FADE_IN_MS = 180;
/** How long the second and third dots take to fade out before the round begins again. */
const FADE_OUT_MS = 120;

/** How strong dot `dot` (0, 1 or 2) is, from 0 to 1, `elapsed` milliseconds after the dots began. The first dot is
 *  always there; the second comes after one step, the third after two, and both go as the round ends. */
export function dotOpacity(dot: number, elapsed: number, step = DOT_STEP_MS): number {
	if (dot <= 0) return 1;
	const round = step * 3;
	const within = ((elapsed % round) + round) % round;
	const appears = dot * step;
	if (within < appears) return 0;
	const coming = Math.min(1, (within - appears) / FADE_IN_MS);
	const going = Math.min(1, (round - within) / FADE_OUT_MS);
	return Math.max(0, Math.min(coming, going));
}

/** How many dots are shown `elapsed` milliseconds after the dots began: 1, 2 or 3. */
export function dotsShown(elapsed: number, step = DOT_STEP_MS): number {
	const round = step * 3;
	const within = ((elapsed % round) + round) % round;
	return Math.floor(within / step) + 1;
}
