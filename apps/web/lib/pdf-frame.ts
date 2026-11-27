/**
 * What the reader's frame of a paper's PDF (`/api/pdf/<id>`, our own backend) ended up showing. The backend
 * answers a PDF it cannot fetch with an error (`{"error": "..."}`) that a frame would show as raw JSON, or as a
 * blank page: the reader shows the error's words instead.
 *
 *     pdf       the PDF, shown by the browser's viewer
 *     problem   anything else: the backend's words when it gave some, else what the frame was given
 */
export type FramedPdf = { kind: "pdf" } | { kind: "problem"; words: string };

/** What a loaded frame shows, from its document's content type and its text. */
export function framedPdf(contentType: string | null | undefined, text: string | null | undefined): FramedPdf {
	const type = (contentType ?? "").toLowerCase();
	if (type.includes("pdf")) return { kind: "pdf" };
	const body = (text ?? "").trim();
	try {
		const told = JSON.parse(body) as { error?: unknown } | null;
		if (typeof told?.error === "string" && told.error.trim() !== "") return { kind: "problem", words: told.error.trim() };
	} catch {
		// not the backend's JSON: said below by what it is
	}
	if (body === "") return { kind: "problem", words: "The backend sent nothing for this PDF." };
	return { kind: "problem", words: "The backend did not send a PDF." };
}

/** A failed load of the embedded view's page, as Electron's `did-fail-load` tells it. */
export interface FailedLoad {
	errorCode: number;
	errorDescription: string;
	validatedURL: string;
	isMainFrame: boolean;
}

/** Electron's code for a load called off by the next navigation (a redirect, a click): not a failure. */
const ABORTED = -3;

/** Does this `did-fail-load` mean the page shown failed to load (and not a frame of it, or a navigation)? */
export function pageFailed(event: FailedLoad): boolean {
	return event.isMainFrame && event.errorCode !== ABORTED;
}
