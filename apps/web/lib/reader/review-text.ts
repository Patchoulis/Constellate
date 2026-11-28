/**
 * A reviewer's words as they are shown. The text is the reviews page's own (the engine keeps it exactly, so that
 * every place in it can be pointed at), and a page that draws a formula puts each piece of it on a line of its own:
 * "depend on key hyperparameters\n(\nλ\n,\nα\n)\n, but guidance…". Those breaks are the page's layout, not the
 * reviewer's: they are taken out here, for showing only.
 */

/** Marks that begin a list item: a line that is only one of them starts an item, it is not a piece of a formula. */
const LIST_MARK = /^(?:\d{1,3}[.)]|[-*•–—])$/;
/** A piece of a formula: short, and with no word in it (four letters or more in a row). */
const MOST_PIECE = 12;

function isPiece(line: string): boolean {
	return line.length > 0 && line.length <= MOST_PIECE && !/[A-Za-z]{4,}/.test(line) && !LIST_MARK.test(line);
}

/** Whether the break between two lines is the page's doing: one of them is a piece of a formula, or the second
 *  carries on a sentence (it begins with the punctuation that closes or continues one). */
function joins(before: string, after: string): boolean {
	if (before.length === 0 || after.length === 0) return false; // an empty line separates paragraphs
	if (LIST_MARK.test(before) || LIST_MARK.test(after)) return false;
	return isPiece(before) || isPiece(after) || /^[,.;:)\]}]/.test(after);
}

/** What goes between two joined lines: nothing after an opening bracket or before closing punctuation, else a space. */
function between(before: string, after: string): string {
	return /[([{]$/.test(before) || /^[,.;:)\]}]/.test(after) ? "" : " ";
}

/**
 * The text with a formula's pieces back on the line they belong to. Line breaks between whole lines of words, empty
 * lines and list items are kept; text with no such pieces comes back as it was.
 */
export function mendFormulaBreaks(text: string): string {
	const lines = text.split("\n");
	const out: string[] = [];
	// the line last read, as the page had it: what the next one is weighed against (a line put together from pieces
	// is no longer a piece itself, but the words after a formula still belong to it)
	let last = "";
	for (const raw of lines) {
		const line = raw.trim();
		if (out.length > 0 && joins(last, line)) out[out.length - 1] = `${out[out.length - 1].trimEnd()}${between(last, line)}${line}`;
		else out.push(raw);
		last = line;
	}
	return out.join("\n");
}
