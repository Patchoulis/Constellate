/**
 * Where the graph's glows can be drawn once and reused, and which papers a frame needs to draw at all.
 *
 * Every paper adds a halo to its group's glow (components/graph/graph-canvas.ts). Zoomed out, all 12,000 or so halos are
 * on screen and each covers dozens of pixels: drawing them is most of a frame. The glows only change when the
 * level of detail or the focus does, so they are drawn once, in world space, into a canvas of their own (the glow
 * layer), and each frame draws that one image under the camera. The layer has as many pixels per world unit as
 * a halo's sprite has texels, so it is as sharp as the halos drawn one by one: a halo is never shown in more
 * detail than its sprite has.
 */
import type { Box, Point } from "@/lib/graph/layout";

/** A glow layer larger than this many pixels is not kept (about 32 MB): the halos are drawn one by one instead. */
export const GLOW_LAYER_MOST_PIXELS = 8_000_000;

export interface GlowLayerGeometry {
	/** the world rectangle the layer covers: every halo whole */
	box: Box;
	/** layer pixels per world unit */
	scale: number;
	width: number;
	height: number;
}

/**
 * The glow layer for halos of `glow` world units' radius around `points`, drawn from a sprite `texels` wide; null
 * when it would have more than `most` pixels (or there is nothing to draw).
 */
export function glowLayerGeometry(
	points: readonly Point[],
	glow: number,
	texels: number,
	most = GLOW_LAYER_MOST_PIXELS,
): GlowLayerGeometry | null {
	if (points.length === 0 || glow <= 0) return null;
	let minX = Infinity;
	let minY = Infinity;
	let maxX = -Infinity;
	let maxY = -Infinity;
	for (const { x, y } of points) {
		if (x < minX) minX = x;
		if (y < minY) minY = y;
		if (x > maxX) maxX = x;
		if (y > maxY) maxY = y;
	}
	const scale = texels / (2 * glow);
	// whole pixels, so the layer is drawn back at exactly the scale it was drawn at
	const width = Math.ceil((maxX - minX + 2 * glow) * scale);
	const height = Math.ceil((maxY - minY + 2 * glow) * scale);
	if (width * height > most) return null;
	const box = { minX: minX - glow, minY: minY - glow, maxX: minX - glow + width / scale, maxY: minY - glow + height / scale };
	return { box, scale, width, height };
}

/** The part of the world inside a canvas of `width` x `height` px under the camera `screen = world * k + [x, y]`,
 *  grown by `margin` world units on every side. */
export function visibleWorld(camera: { x: number; y: number; k: number }, width: number, height: number, margin = 0): Box {
	return {
		minX: -camera.x / camera.k - margin,
		minY: -camera.y / camera.k - margin,
		maxX: (width - camera.x) / camera.k + margin,
		maxY: (height - camera.y) / camera.k + margin,
	};
}

/** Is the point inside the box (edges included)? */
export const inside = (box: Box, x: number, y: number) => x >= box.minX && x <= box.maxX && y >= box.minY && y <= box.maxY;
