/**
 * How marked papers are shown on the graph. A few marks each get a yellow ring, their links and their name. Many
 * marks (a lasso can take hundreds) would cover the graph that way, so from a number the person chooses (Settings,
 * "Show marked papers as a highlighted region from") they are shown as a region instead: the marked papers bright in
 * their own colours with a soft pale halo, every other paper dimmed, the community glows turned down, and no rings,
 * links or names, except for a marked paper that is hovered or selected.
 *
 * The number counted is the marked papers SHOWN: those in the community in focus, of the kinds not dimmed, and none
 * while the marks are hidden.
 */

export type MarkLook = "rings" | "region";

/** The choice: from how many shown marked papers the region is used. */
export const REGION_FROM = { initial: 25, least: 5, most: 500 } as const;

/** How the shown marked papers are drawn, for so many shown and the number chosen. */
export function markLook(shown: number, from: number): MarkLook {
	return shown > 0 && shown >= from ? "region" : "rings";
}

/** How long one look takes to give way to the other. */
export const LOOK_FADE_MS = 200;

/** How strong the glows of the communities are while the region is shown, as a share of their usual strength. */
export const REGION_GLOW = 0.3;

/** A number read from storage or typed in, as the choice: a whole number between the bounds, or the initial choice
 *  when it is no number at all. */
export function regionFromOf(value: unknown): number {
	const number = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN;
	if (!Number.isFinite(number)) return REGION_FROM.initial;
	return Math.min(REGION_FROM.most, Math.max(REGION_FROM.least, Math.round(number)));
}

/** The marked papers' halos are drawn once into a layer at one zoom and kept while the zoom stays within this factor
 *  of it: a halo of about 9 pixels is then between about 7 and 11, and a pan never redraws them. */
export const HALO_ZOOM_SLACK = 1.25;

/** Whether halos drawn at zoom `built` may still be shown at zoom `k`. */
export function haloLayerFits(built: number, k: number): boolean {
	return k > 0 && built > 0 && k <= built * HALO_ZOOM_SLACK && k >= built / HALO_ZOOM_SLACK;
}

/** How far the look has gone from `from` (0: rings, 1: region) toward `to`, `elapsed` ms after it began to change. */
export function lookTowards(from: number, to: number, elapsed: number, ms = LOOK_FADE_MS): number {
	if (!(ms > 0)) return to;
	const share = Math.min(1, Math.max(0, elapsed / ms));
	return from + (to - from) * share;
}
