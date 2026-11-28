/**
 * What the window says when its page stops working (an exception while drawing it, or a part of it that could not
 * be loaded): the words, kept apart from the screens that show them (app/error.tsx, app/global-error.tsx) so they
 * can be tested.
 */

/** What stopped the page, as far as it can be said in a line. Next hides a server error's message in the built app
 *  and gives a digest instead: the reference to look for in the log. */
export function errorLine(error: { message?: unknown; digest?: unknown } | null | undefined): string {
	const message = typeof error?.message === "string" ? error.message.trim() : "";
	const digest = typeof error?.digest === "string" && error.digest ? ` (reference ${error.digest})` : "";
	if (!message) return `No reason was given${digest}.`;
	// one line, and not a screenful: the rest is in the console
	const first = message.split("\n")[0];
	return `${first.length > 300 ? `${first.slice(0, 300)}…` : first}${digest}`;
}

/** Whether the page stopped because a part of the interface could not be fetched: the app was updated or restarted
 *  under an open window, and loading the window again is all it needs. */
export function isStaleInterface(error: { name?: unknown; message?: unknown } | null | undefined): boolean {
	const text = `${typeof error?.name === "string" ? error.name : ""} ${typeof error?.message === "string" ? error.message : ""}`;
	return /ChunkLoadError|Loading chunk|Failed to fetch dynamically imported module|error loading dynamically imported module/i.test(text);
}

export const PAGE_ERROR = {
	title: "This window stopped working",
	stale:
		"A part of the interface could not be loaded, which happens when the app was updated or restarted while this window was open. Loading the window again fixes it.",
	other:
		"Something went wrong while drawing this page. Your library, runs and marks are not affected: they are kept by the engine and in this window's storage, not by the page.",
	log: "If it keeps happening, the engine's log is in the app's data folder, under logs (engine.log).",
	again: "Try again",
	reload: "Load the window again",
} as const;
