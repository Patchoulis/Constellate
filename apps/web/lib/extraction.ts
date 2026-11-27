/**
 * The extraction table's logic (engine analysis/extraction.rs): what a cell says in words, sorting and filtering
 * the rows by a column, how many papers state something, the CSV a user saves, and which papers have which value
 * (for colouring the graph). Plain functions, tested in tests/extraction.test.ts; components/extraction-table.tsx
 * draws them.
 */
import type { Evidence, ExtractionCell, ExtractionColumn, ExtractionRow, ExtractionTable, ExtractionValue } from "@rp/contracts";
import { count } from "@/lib/units";

/** What a cell says when the paper does not say: an answer, not a gap. */
export const NOT_STATED = "not stated in the paper";
/** What a cell says when the model gave a value whose quotes were not found: the value is not shown. */
export const LEFT_OUT = "not verified: left out";

/** What each kind of answer is called where a column is written. */
export const KIND_NAMES: Record<ExtractionColumn["kind"], string> = {
	text: "Free text",
	yes_no: "Yes, no or not stated",
	number: "Number with its unit",
	list: "List of names",
};

/** A stated value in words: "84.3 % top-1 accuracy (ImageNet)", "ImageNet, CIFAR-10", "yes". */
export function valueText(value: ExtractionValue): string {
	switch (value.kind) {
		case "text":
			return value.text;
		case "yes_no":
			return value.answer;
		case "number": {
			const unit = value.unit ? ` ${value.unit}` : "";
			return `${value.written}${unit}${value.context ? ` (${value.context})` : ""}`;
		}
		case "list":
			return value.items.join(", ");
	}
}

/** A cell in words, whatever it is. */
export function cellText(cell: ExtractionCell): string {
	switch (cell.kind) {
		case "stated":
			return valueText(cell.value);
		case "not_stated":
			return NOT_STATED;
		case "not_verified":
			return cell.why === "number_not_in_quote"
				? `${LEFT_OUT} (the number is not in its quotes)`
				: cell.why === "value_not_in_quote"
					? `${LEFT_OUT} (the value is not in its quotes)`
					: LEFT_OUT;
		case "malformed":
			return cell.why === "omitted" ? "the model gave no answer for this column" : "the answer did not fit the column: left out";
		case "not_asked":
			return "not asked yet";
	}
}

/** Where a sort puts a cell: values first (numbers by value, the rest alphabetically), then "not stated", then
 *  what is left out, then what was not asked. */
function rank(cell: ExtractionCell): number {
	return { stated: 0, not_stated: 1, not_verified: 2, malformed: 2, not_asked: 3 }[cell.kind];
}

/**
 * The rows ordered by column `column`: numbers by their value, everything else alphabetically; whichever way
 * (`descending`), "not stated" and the cells without a value come last. Ties keep the rows' order (most central
 * first).
 */
export function sortRows(rows: ExtractionRow[], column: number, descending = false): ExtractionRow[] {
	const sign = descending ? -1 : 1;
	return rows
		.map((row, index) => ({ row, index }))
		.sort((a, b) => {
			const [x, y] = [a.row.cells[column], b.row.cells[column]];
			const order = rank(x) - rank(y);
			if (order !== 0) return order;
			if (x.kind === "stated" && y.kind === "stated") {
				const by =
					x.value.kind === "number" && y.value.kind === "number"
						? x.value.value - y.value.value
						: valueText(x.value).localeCompare(valueText(y.value), undefined, { sensitivity: "base", numeric: true });
				if (by !== 0) return sign * by;
			}
			return a.index - b.index;
		})
		.map(({ row }) => row);
}

/** The keys a cell can be filtered by: "stated" or "not_stated", and its value (each item of a list). */
function keysOf(cell: ExtractionCell): string[] {
	if (cell.kind === "not_stated") return ["not_stated"];
	if (cell.kind !== "stated") return [];
	const value = cell.value;
	const values = value.kind === "list" ? value.items : value.kind === "number" ? [] : [value.kind === "yes_no" ? value.answer : value.text];
	return ["stated", ...values.map((item) => `value:${item.toLowerCase()}`)];
}

/** One choice of a column's filter, with how many papers it keeps. */
export interface FilterOption {
	key: string;
	label: string;
	papers: number;
}

/** What a column can be filtered by: "states it", "not stated", and its most frequent values (at most `most`). */
export function filterOptions(rows: ExtractionRow[], column: number, most = 12): FilterOption[] {
	const counts = new Map<string, { label: string; papers: number }>();
	for (const row of rows) {
		const cell = row.cells[column];
		const labels = new Map<string, string>();
		if (cell.kind === "stated" && cell.value.kind === "list")
			cell.value.items.forEach((item) => labels.set(`value:${item.toLowerCase()}`, item));
		if (cell.kind === "stated" && cell.value.kind === "yes_no") labels.set(`value:${cell.value.answer}`, cell.value.answer);
		if (cell.kind === "stated" && cell.value.kind === "text") labels.set(`value:${cell.value.text.toLowerCase()}`, cell.value.text);
		for (const key of new Set(keysOf(cell))) {
			const label = key === "stated" ? "states it" : key === "not_stated" ? NOT_STATED : (labels.get(key) ?? key);
			const known = counts.get(key);
			counts.set(key, { label: known?.label ?? label, papers: (known?.papers ?? 0) + 1 });
		}
	}
	const fixed = ["stated", "not_stated"].flatMap((key) => (counts.has(key) ? [{ key, ...counts.get(key)! }] : []));
	const values = [...counts.entries()]
		.filter(([key]) => key.startsWith("value:"))
		.map(([key, found]) => ({ key, ...found }))
		.sort((a, b) => b.papers - a.papers || a.label.localeCompare(b.label))
		.slice(0, most);
	return [...fixed, ...values];
}

/** The rows whose cell of `column` matches `key` (one of `filterOptions`' keys). */
export function filterRows(rows: ExtractionRow[], column: number, key: string): ExtractionRow[] {
	return rows.filter((row) => keysOf(row.cells[column]).includes(key));
}

/** How many papers state the column, of those asked: "48 of 60 papers state Compute (12 not asked yet)". */
export function coverage(rows: ExtractionRow[], column: number, label: string): string {
	const asked = rows.filter((row) => row.cells[column].kind !== "not_asked");
	const stated = asked.filter((row) => row.cells[column].kind === "stated").length;
	const waiting = rows.length - asked.length;
	if (asked.length === 0) return `No paper asked about ${label.toLowerCase()} yet (${count(waiting, "paper")} not asked)`;
	return `${stated} of ${count(asked.length, "paper")} state ${label.toLowerCase()}${waiting > 0 ? ` (${count(waiting, "paper")} not asked yet)` : ""}`;
}

/** Which papers have which value of a column, for colouring the graph by it: by value (in words), the papers'
 *  ids; "not stated" is a group of its own, and cells without a value are in none. */
export function paperGroups(rows: ExtractionRow[], column: number): Map<string, string[]> {
	const groups = new Map<string, string[]>();
	for (const row of rows) {
		const cell = row.cells[column];
		const names =
			cell.kind === "not_stated"
				? [NOT_STATED]
				: cell.kind !== "stated"
					? []
					: cell.value.kind === "list"
						? cell.value.items
						: [valueText(cell.value)];
		for (const name of names) groups.set(name, [...(groups.get(name) ?? []), row.paper]);
	}
	return groups;
}

/** Where a piece of evidence is: "p. 4", or "abstract" when the PDF was not read. */
export function evidencePlace(evidence: Evidence): string {
	const established = evidence.established;
	if (established.kind === "abstract") return established.field;
	return established.address.end_page > established.address.page
		? `pp. ${established.address.page}-${established.address.end_page}`
		: `p. ${established.address.page}`;
}

/** One CSV field: quoted when it must be, and kept from being read as a formula by a spreadsheet (anything that
 *  starts with a formula's first character and is not simply a number). */
function field(text: string | number | null): string {
	let value = text === null ? "" : String(text);
	if (/^\s*[=+\-@\t\r]/.test(value) && !/^[+-]?\d+(\.\d+)?$/.test(value)) value = `'${value}`;
	return /[",\n\r]/.test(value) ? `"${value.replace(/"/g, '""')}"` : value;
}

const READ_FROM: Record<ExtractionRow["text"], string> = {
	pdf: "PDF",
	no_pdf: "title and abstract",
	unreadable: "title and abstract",
	not_in_library: "not in the library",
};

/**
 * The table as CSV: for each paper its id, title, year and acceptance, the model that read it, then for each
 * column its value as the model read it (or what stands in for one), the quotes it rests on in the paper's words,
 * and their pages. The headers say which is the model's reading and which the paper's words.
 */
export function toCsv(table: ExtractionTable, rows: ExtractionRow[] = table.rows): string {
	const header = ["paper", "title", "year", "acceptance", "read from", "values read by the model"];
	for (const column of table.columns)
		header.push(`${column.label} (the model's reading)`, `${column.label}: quotes (the paper's words)`, `${column.label}: pages`);
	const lines = [header.map(field).join(",")];
	for (const row of rows) {
		const fields: (string | number | null)[] = [row.paper, row.title, row.year, row.acceptance, READ_FROM[row.text], table.model];
		for (const cell of row.cells) {
			const evidence = cell.kind === "stated" ? cell.evidence : [];
			fields.push(cellText(cell), evidence.map((piece) => piece.quote).join(" | "), evidence.map(evidencePlace).join(" | "));
		}
		lines.push(fields.map(field).join(","));
	}
	return `${lines.join("\r\n")}\r\n`;
}

/** What the table leaves out, in words; null when nothing is. */
export function leftOutNote(table: ExtractionTable): string | null {
	const { quotes_not_found, value_not_in_quote, malformed } = table.left_out;
	const parts = [
		quotes_not_found > 0 && `${count(quotes_not_found, "cell")} left out: their quotes were not found in the paper`,
		value_not_in_quote > 0 && `${count(value_not_in_quote, "value")} left out: not in the quotes given for them`,
		malformed > 0 && `${count(malformed, "answer")} left out: they did not fit their column`,
	].filter((part): part is string => Boolean(part));
	return parts.length > 0 ? `${parts.join("; ")}.` : null;
}

/** A new column's id from its label: "Out-of-distribution data?" -> "out-of-distribution-data", made unique. */
export function columnId(label: string, taken: string[]): string {
	const base =
		label
			.toLowerCase()
			.normalize("NFKD")
			.replace(/[^a-z0-9]+/g, "-")
			.replace(/^-+|-+$/g, "")
			.slice(0, 32) || "column";
	let id = base;
	for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
	return id;
}

/** `columns` with the one at `index` moved by `by` places (kept inside the list). */
export function moved<T>(columns: T[], index: number, by: number): T[] {
	const to = Math.max(0, Math.min(columns.length - 1, index + by));
	const next = [...columns];
	const [column] = next.splice(index, 1);
	next.splice(to, 0, column);
	return next;
}
