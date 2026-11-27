import katex from "katex";

/**
 * Paper titles and abstracts as their authors wrote them: plain text with LaTeX in it.
 *
 *   math        "$k$-medians under $\ell_p$", "\(2.2\times\)"         typeset with KaTeX
 *   links       "\url{https://...}", "\href{https://...}{code}",      become links
 *               and web addresses written out
 *   text marks  "\textbf{x}", "\emph{x}", "\%", "``quoted''", "--",   become what they stand for
 *               "\'e", "\footnote{x}"
 *   emphasis    "**CALM** (*Corrective Adaptation ...*)"                 Markdown's bold and italic
 *
 * Anything not recognised (an author's own macro, say "\ours") is left exactly as written.
 */

/** `$...$` and `$$...$$` spans. */
const MATH = /(\$\$[^$]+\$\$|\$[^$\n]+\$)/g;
/** `\url{address}`, `\href{address}{text}`, or an address written out (not ending in punctuation). */
const LINK = /\\url\{([^{}\s]+)\}|\\href\{([^{}\s]+)\}\{([^{}]*)\}|(https?:\/\/[^\s{}<>"]*[^\s{}<>".,;:)])/g;
/** Stands in for an escaped dollar sign ("\$5") while the text is cut at the real ones. */
const DOLLAR = "\u0000";

const ACCENTS: Record<string, string> = { "'": "́", "`": "̀", "^": "̂", '"': "̈", "~": "̃" };
const STYLED = /\\(?:textbf|textit|textsc|texttt|textrm|textsf|text|emph|underline|mbox|mathrm)\{([^{}]*)\}/g;

/** LaTeX's text-mode markup in a stretch of text with no math and no links in it, as plain text. */
export function plainText(text: string): string {
	return (
		text
			// \'e, \"{o}, \~n: the letter with its accent
			.replace(/\\(['`^"~])\{?([A-Za-z])\}?/g, (_, mark: string, letter: string) => (letter + ACCENTS[mark]).normalize("NFC"))
			.replace(/\\c\{([cC])\}/g, (_, letter: string) => (letter === "c" ? "ç" : "Ç"))
			// \textbf{x}, \emph{x}, {\bf x}, \textcolor{red}{x}: just x (twice, for one level of nesting)
			.replace(STYLED, "$1")
			.replace(STYLED, "$1")
			.replace(/\{\\(?:bf|it|em|tt|sc|rm|sf)\s+([^{}]*)\}/g, "$1")
			.replace(/\\textcolor\{[^{}]*\}\{([^{}]*)\}/g, "$1")
			// a footnote is said in brackets; references that mean nothing outside the paper are dropped
			.replace(/\\footnote\{([^{}]*)\}/g, " ($1)")
			.replace(/\s*\\(?:cite[tp]?|ref|eqref|label)\{[^{}]*\}/g, "")
			.replace(/\\eg\b\.?/g, "e.g.")
			.replace(/\\ie\b\.?/g, "i.e.")
			.replace(/\\(?:newline|par|noindent|linebreak)\b\s*|\\\\\s*|\\[,;! ]/g, " ")
			// \%, \&, \_, \#, \{, \}: the character itself
			.replace(/\\([%&_#{}])/g, "$1")
			.replace(/``|''/g, (quote) => (quote === "``" ? "“" : "”"))
			.replace(/---/g, "—")
			.replace(/--/g, "–")
			.replace(/~/g, " ")
			.replaceAll(DOLLAR, "$")
	);
}

function Formula({ source }: { source: string }) {
	const html = katex.renderToString(source.replaceAll(DOLLAR, "\\$"), { throwOnError: false, output: "html" });
	// KaTeX builds this markup itself from the parsed expression; nothing of the source is passed through as HTML.
	return <span dangerouslySetInnerHTML={{ __html: html }} />;
}

/** Markdown's `**bold**` and `*italic*`, as OpenReview abstracts use them ("**CALM** (*Corrective Adaptation*)").
 *  A star that touches a letter or digit on its outer side, or a space on its inner side, is not emphasis
 *  ("a * b", "2*3*4"). */
const EMPHASIS = /\*\*([^*\n]+?)\*\*|(?<![\w*])\*([^*\s](?:[^*\n]*[^*\s])?)\*(?![\w*])/g;

/** A stretch of text with no math and no links: LaTeX's marks resolved, Markdown's emphasis set. */
function emphasised(text: string, key: number) {
	const plain = plainText(text);
	if (!plain.includes("*")) return plain;
	const parts = [];
	let from = 0;
	for (const found of plain.matchAll(EMPHASIS)) {
		parts.push(plain.slice(from, found.index));
		parts.push(
			found[1] !== undefined ? <strong key={`${key}b${found.index}`}>{found[1]}</strong> : <em key={`${key}i${found.index}`}>{found[2]}</em>,
		);
		from = found.index + found[0].length;
	}
	parts.push(plain.slice(from));
	return parts;
}

/** Text with no math in it: its links as links, the rest as plain text. */
function Text({ text }: { text: string }) {
	const parts = [];
	let from = 0;
	for (const found of text.matchAll(LINK)) {
		const address = found[1] ?? found[2] ?? found[4];
		parts.push(emphasised(text.slice(from, found.index), from));
		parts.push(
			/^https?:\/\//.test(address) ? (
				<a
					key={found.index}
					href={address}
					target="_blank"
					rel="noopener noreferrer"
					className="underline decoration-muted-foreground underline-offset-2 hover:decoration-foreground"
					onClick={(event) => event.stopPropagation()}
				>
					{found[3] ? plainText(found[3]) : address}
				</a>
			) : (
				(found[3] ?? address) // not a web address: show it, do not make it clickable
			),
		);
		from = found.index + found[0].length;
	}
	parts.push(emphasised(text.slice(from), from));
	return <>{parts}</>;
}

export function MathText({ children }: { children: string }) {
	if (!/[\\$`~*]|--|''|https?:/.test(children)) return children; // nothing to do for most titles
	return children
		.replaceAll("\\$", DOLLAR)
		.replace(/\\\((.+?)\\\)/g, "$$$1$$") // \( ... \) is $ ... $
		.split(MATH)
		.map((part, index) =>
			// split() with one capture group: odd entries are the math
			index % 2 ? (
				<Formula key={index} source={part.replace(/^\$\$?|\$\$?$/g, "")} />
			) : (
				<Text key={index} text={part} />
			),
		);
}
