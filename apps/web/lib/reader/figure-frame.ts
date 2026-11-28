/**
 * The page a paper's figures are drawn in: pdf.js (Mozilla's, Apache-2.0) in a frame of its own, so that a PDF,
 * which is a file from somewhere else, is read by code that cannot reach the app.
 *
 * - The frame is sandboxed with scripts only (`sandbox="allow-scripts"`, no `allow-same-origin`): its origin is
 *   opaque, so it can read no cookie, no storage and nothing of the app's page, and a request it made would be
 *   another site's, which the interface's server refuses for /api (proxy.ts) and the engine never sees.
 * - It may make no request at all: its policy ([`framePolicy`]) allows no connection, no frame, no form, and
 *   scripts only from its own bootstrap (by hash) and the blobs it makes of the code it carries. pdf.js and its
 *   worker come in the page itself (base64, read from the installed package by app/figure-frame/route.ts), never
 *   from the network; no fonts or CMaps are fetched (`standardFontDataUrl` and `cMapUrl` are not given).
 * - pdf.js reads with `isEvalSupported: false` (a crafted font cannot become code), in a worker made from that
 *   code (a data: URL: a module worker made from a blob does not start in an opaque origin).
 * - The app's page sends it the PDF's bytes and, one at a time, what to draw ([`Ask`]); it answers with a PNG of
 *   the pixels asked for, or why not ([`Told`]). The page checks that an answer comes from its frame, gives each
 *   drawing [`DRAW_TIME`] and drops the frame past it; the frame refuses a drawing of more than
 *   lib/reader/figures.ts `MOST_PIXELS` and frees each page once it is drawn.
 */

/** Where the frame's page is served. */
export const FIGURE_FRAME = "/figure-frame";

/** How long one figure may take to draw before the frame is dropped and the figure told as not drawn. */
export const DRAW_TIME = 20_000;

/** What the app's page asks the frame. */
export type Ask =
	| { kind: "open"; pdf: ArrayBuffer }
	| { kind: "draw"; id: number; page: number; scale: number; left: number; top: number; width: number; height: number };

/** What the frame answers. */
export type Told =
	{ kind: "ready" } | { kind: "opened" } | { kind: "drawn"; id: number; png: Blob } | { kind: "failed"; id: number | null; why: string };

/** Is `data` one of the frame's answers, in its shape? Anything else from the frame is ignored. */
export function isTold(data: unknown): data is Told {
	if (typeof data !== "object" || data === null) return false;
	const told = data as Record<string, unknown>;
	switch (told.kind) {
		case "ready":
		case "opened":
			return true;
		case "drawn":
			return typeof told.id === "number" && told.png instanceof Blob && told.png.type === "image/png";
		case "failed":
			return (told.id === null || typeof told.id === "number") && typeof told.why === "string";
		default:
			return false;
	}
}

/** The frame's script, run as a module: it makes pdf.js and its worker of the code the page carries. */
export const BOOTSTRAP = `
const code = (id) => {
	const text = atob(document.getElementById(id).textContent.trim());
	const bytes = new Uint8Array(text.length);
	for (let at = 0; at < text.length; at++) bytes[at] = text.charCodeAt(at);
	return URL.createObjectURL(new Blob([bytes], { type: "text/javascript" }));
};
const MOST_PIXELS = 12000000;
const pdfjs = await import(code("pdfjs"));
// a module worker from a blob does not start in an opaque origin; one from a data: URL of the same code does
pdfjs.GlobalWorkerOptions.workerPort = new Worker("data:text/javascript;base64," + document.getElementById("pdfjs-worker").textContent.trim(), { type: "module" });
let paper = null;
const tell = (told) => parent.postMessage(told, "*");
addEventListener("message", async (event) => {
	if (event.source !== parent) return;
	const ask = event.data;
	try {
		if (ask.kind === "open") {
			paper = await pdfjs.getDocument({ data: ask.pdf, isEvalSupported: false, enableXfa: false, useSystemFonts: false }).promise;
			tell({ kind: "opened" });
		} else if (ask.kind === "draw") {
			if (!paper) throw new Error("no PDF is open");
			if (!(ask.width > 0 && ask.height > 0 && ask.width * ask.height <= MOST_PIXELS)) throw new Error("the figure is too large to draw");
			const page = await paper.getPage(ask.page);
			try {
				// "print": drawn at once, not frame by frame (a hidden frame gets no animation frames)
				// intent "print": drawn at once, not frame by frame (a hidden frame gets no animation frames)
				const viewport = page.getViewport({ scale: ask.scale });
				const canvas = new OffscreenCanvas(ask.width, ask.height);
				const context = canvas.getContext("2d");
				context.fillStyle = "#fff";
				context.fillRect(0, 0, ask.width, ask.height);
				await page.render({ canvas, canvasContext: context, viewport, transform: [1, 0, 0, 1, -ask.left, -ask.top], annotationMode: pdfjs.AnnotationMode.DISABLE, intent: "print" }).promise;
				const png = await canvas.convertToBlob({ type: "image/png" });
				canvas.width = 0;
				canvas.height = 0;
				tell({ kind: "drawn", id: ask.id, png });
			} finally {
				page.cleanup();
			}
		}
	} catch (error) {
		tell({ kind: "failed", id: ask.kind === "draw" ? ask.id : null, why: String(error?.message ?? error) });
	}
});
tell({ kind: "ready" });
`;

/** The frame's Content-Security-Policy: nothing but its bootstrap (by its hash) and the code it makes blobs of. */
export function framePolicy(bootstrapHash: string): string {
	return [
		"default-src 'none'",
		`script-src 'sha256-${bootstrapHash}' blob:`,
		"worker-src data:",
		"img-src blob: data:",
		"font-src blob: data:",
		"style-src 'unsafe-inline'",
		"connect-src 'none'",
		"frame-src 'none'",
		"form-action 'none'",
		"base-uri 'none'",
		"frame-ancestors 'self'",
	].join("; ");
}

/** The frame's page, carrying pdf.js and its worker as base64. */
export function framePage(pdfjs: string, worker: string): string {
	return `<!doctype html><html><head><meta charset="utf-8"><title>figures</title></head><body>
<script type="text/plain" id="pdfjs">${pdfjs}</script>
<script type="text/plain" id="pdfjs-worker">${worker}</script>
<script type="module">${BOOTSTRAP}</script>
</body></html>`;
}
