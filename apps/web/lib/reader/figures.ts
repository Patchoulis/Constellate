/**
 * The arithmetic of drawing one figure of a paper: from its rectangle in PDF points, on a page with a crop box and
 * a rotation, to the scale pdf.js renders the page at and the pixels of that rendering the figure is (what
 * lib/reader/figure-frame.ts asks the sandboxed renderer for). Plain functions, tested in tests/reader/figures.test.ts.
 *
 * pdf.js draws a page as its viewport says: the crop box, turned by the page's rotation (clockwise), scaled, with
 * y downwards from the top left of what is shown. `toViewport` is that transform, so the pixels asked for are the
 * ones pdf.js puts the figure at.
 */
import type { PaperFigure, PdfRect } from "@rp/contracts";

/** A point of the page (PDF points, y up) where pdf.js's viewport puts it at `scale` (pixels, y down). */
export function toViewport(crop: PdfRect, rotation: number, scale: number, x: number, y: number): [number, number] {
	switch (((rotation % 360) + 360) % 360) {
		case 90:
			return [(y - crop.y0) * scale, (x - crop.x0) * scale];
		case 180:
			return [(crop.x1 - x) * scale, (y - crop.y0) * scale];
		case 270:
			return [(crop.y1 - y) * scale, (crop.x1 - x) * scale];
		default:
			return [(x - crop.x0) * scale, (crop.y1 - y) * scale];
	}
}

/** The width and height a figure is shown at, in points, the page turned as it is shown. */
export function shownSize(figure: Pick<PaperFigure, "rect" | "page_box">): { width: number; height: number } {
	const width = figure.rect.x1 - figure.rect.x0;
	const height = figure.rect.y1 - figure.rect.y0;
	return figure.page_box.rotation % 180 === 90 ? { width: height, height: width } : { width, height };
}

/** The most pixels one figure is drawn with: a page's worth at a good resolution, whatever a PDF says its size is. */
export const MOST_PIXELS = 12_000_000;

/** What to ask the renderer for: the scale of the page, and the pixels of the rendering that are the figure. */
export interface Crop {
	scale: number;
	left: number;
	top: number;
	width: number;
	height: number;
}

/**
 * The crop that draws `figure` `pixels` wide (a column's CSS width times the device's pixel ratio, say), no more
 * than [`MOST_PIXELS`] in all. None for a figure with no area.
 */
export function cropFor(figure: Pick<PaperFigure, "rect" | "page_box">, pixels: number): Crop | null {
	const { width, height } = shownSize(figure);
	if (!(width > 0 && height > 0 && pixels > 0)) return null;
	let scale = pixels / width;
	if (width * height * scale * scale > MOST_PIXELS) scale = Math.sqrt(MOST_PIXELS / (width * height));
	const { crop, rotation } = figure.page_box;
	const corners = [
		toViewport(crop, rotation, scale, figure.rect.x0, figure.rect.y0),
		toViewport(crop, rotation, scale, figure.rect.x1, figure.rect.y1),
	];
	const left = Math.floor(Math.min(corners[0][0], corners[1][0]));
	const top = Math.floor(Math.min(corners[0][1], corners[1][1]));
	const right = Math.ceil(Math.max(corners[0][0], corners[1][0]));
	const bottom = Math.ceil(Math.max(corners[0][1], corners[1][1]));
	return { scale, left, top, width: right - left, height: bottom - top };
}
