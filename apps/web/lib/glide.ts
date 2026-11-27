/**
 * The camera's way from one view of the graph to another, so that a jump (to a community, to a paper, back to
 * the whole graph) is something the eye can follow.
 *
 * A view is a zoom `k` and an offset `(x, y)`: the world point `(wx, wy)` is drawn at `(k * wx + x, k * wy + y)`.
 * Going from one view to another, the zoom changes by the same factor every instant (so zooming in by 8 feels
 * even, rather than rushing at the start) while the world point in the middle of the picture moves in a straight
 * line; the whole is eased, slow at both ends.
 */

export interface View {
	x: number;
	y: number;
	k: number;
}

/** How long a glide takes. Long enough to follow, short enough not to wait for. */
export const GLIDE_MS = 450;

/** Slow at both ends, quick in the middle; 0 at 0 and 1 at 1. */
export function ease(progress: number): number {
	const t = Math.min(1, Math.max(0, progress));
	return t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
}

/** The view `progress` (0 to 1) of the way from `from` to `to`, in a picture of the given size. */
export function viewBetween(from: View, to: View, size: { width: number; height: number }, progress: number): View {
	const t = ease(progress);
	if (t <= 0) return from;
	if (t >= 1) return to;
	const middle = { x: size.width / 2, y: size.height / 2 };
	// the world point each view has in the middle of the picture
	const centre = (view: View) => ({ x: (middle.x - view.x) / view.k, y: (middle.y - view.y) / view.k });
	const [start, end] = [centre(from), centre(to)];
	const k = from.k * (to.k / from.k) ** t;
	const wx = start.x + (end.x - start.x) * t;
	const wy = start.y + (end.y - start.y) * t;
	return { k, x: middle.x - k * wx, y: middle.y - k * wy };
}

/** Has the person asked their system for less motion? Then the camera goes at once. */
export function prefersLessMotion(): boolean {
	return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/** How the next frame is asked for and called off, and what time it is: the browser's, unless a test says otherwise. */
export interface Frames {
	request: (step: () => void) => number;
	cancel: (frame: number) => void;
	now: () => number;
}

const BROWSER: Frames = {
	request: (step) => requestAnimationFrame(step),
	cancel: (frame) => cancelAnimationFrame(frame),
	now: () => performance.now(),
};

export type Size = { width: number; height: number };

/** Where a glide goes: a view, or how to work it out for a picture of a given size (fitting a box, centring a
 *  paper), so that the destination can be worked out again when the picture changes size on the way. */
export type Aim = View | ((size: Size) => View);

const viewOf = (aim: Aim, size: Size): View => (typeof aim === "function" ? aim(size) : aim);

/**
 * One camera, one glide at a time.
 *
 * Asking for a new glide while one is running ends the running one where it is: it shows nothing more and its
 * `arrived` is never called. The new one starts from the view given to it, which is the view the camera has at
 * that instant, so the picture never jumps back or ahead: it just changes course.
 *
 * The picture changing size on the way (`resized`) ends the glide at once at its destination worked out for the
 * new size: the view it was aiming at was worked out for a size that no longer is, and gliding on to it would end
 * off its mark.
 */
export class Glide {
	private frame = 0;
	/** the glide under way: where it goes, how a view is shown, and what to do on arriving */
	private under: { aim: Aim; show: (view: View) => void; arrived?: () => void } | null = null;

	constructor(private frames: Frames = BROWSER) {}

	get running(): boolean {
		return this.frame !== 0;
	}

	/** End the glide in progress, if any, leaving the camera where it is. */
	stop() {
		if (this.frame) this.frames.cancel(this.frame);
		this.frame = 0;
		this.under = null;
	}

	/** Glide from `from` (where the camera is now) to `aim`, showing each view on the way; `arrived` is called once,
	 *  at the destination, unless another glide or `stop` comes first. */
	go(from: View, aim: Aim, size: Size, show: (view: View) => void, arrived?: () => void) {
		this.stop();
		const to = viewOf(aim, size);
		const started = this.frames.now();
		const under = { aim, show, arrived };
		this.under = under;
		const step = () => {
			const progress = (this.frames.now() - started) / GLIDE_MS;
			if (progress >= 1) return this.arrive(under, to);
			show(viewBetween(from, to, size, progress));
			this.frame = this.frames.request(step);
		};
		this.frame = this.frames.request(step);
	}

	/** The picture is now `size`: a glide under way ends at once at its destination for that size. */
	resized(size: Size) {
		const under = this.under;
		if (!under) return;
		if (this.frame) this.frames.cancel(this.frame);
		this.arrive(under, viewOf(under.aim, size));
	}

	private arrive(under: NonNullable<Glide["under"]>, to: View) {
		this.frame = 0;
		this.under = null;
		under.show(to);
		under.arrived?.();
	}
}
