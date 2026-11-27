/** LaTeX in titles and abstracts: what readers should and should not see. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MathText, plainText } from "@/components/math-text";

const shown = (text: string) => renderToStaticMarkup(<MathText>{text}</MathText>);
/** What a reader sees: the markup without its tags. */
const read = (text: string) =>
	shown(text)
		.replace(/<[^>]+>/g, "")
		.replace(/&amp;/g, "&")
		.replace(/&#x27;/g, "'")
		.replace(/&quot;/g, '"');

test("escaped characters lose their backslash", () => {
	assert.equal(plainText("improves by 12\\% on A\\&B with snake\\_case and \\#1"), "improves by 12% on A&B with snake_case and #1");
});

test("styling commands leave their content", () => {
	assert.equal(
		plainText("we establish \\textbf{the first benchmark} in \\emph{collaborative} settings"),
		"we establish the first benchmark in collaborative settings",
	);
	assert.equal(plainText("\\textsc{NeedleInATable} and {\\bf bold} and \\textcolor{red}{warm}"), "NeedleInATable and bold and warm");
	assert.equal(plainText("\\textbf{nested \\emph{inside}}"), "nested inside");
});

test("quotes, dashes, ties and accents become the characters they stand for", () => {
	assert.equal(plainText("``quoted'' text, pages 3--5 --- and Fig.~2"), "\u201cquoted\u201d text, pages 3\u20135 \u2014 and Fig.\u00a02");
	assert.equal(
		plainText("Erd\\H{o}s is left alone but Poincar\\'e and G\\\"odel are not"),
		"Erd\\H{o}s is left alone but Poincaré and Gödel are not",
	);
});

test("citations and labels disappear; a footnote is said in brackets", () => {
	assert.equal(
		plainText("as shown before \\cite{smith2020} it works.\\footnote{Code is public.}"),
		"as shown before it works. (Code is public.)",
	);
});

test("an author's own macro is left as written rather than guessed at", () => {
	assert.equal(plainText("we call it \\ours"), "we call it \\ours");
});

test("a \\url becomes a link to that address", () => {
	const html = shown("Project page: \\url{https://lck666666.github.io/plana3r/}.");
	assert.match(html, /<a href="https:\/\/lck666666\.github\.io\/plana3r\/"[^>]*>https:\/\/lck666666\.github\.io\/plana3r\/<\/a>\./);
	assert.ok(!html.includes("\\url"));
});

test("a \\href shows its text and links to its address; an address written out is a link too", () => {
	assert.match(shown("code \\href{https://github.com/a/b}{here}"), /<a href="https:\/\/github\.com\/a\/b"[^>]*>here<\/a>/);
	const bare = shown("see https://example.org/a--b~c, then more");
	assert.match(bare, /<a href="https:\/\/example\.org\/a--b~c"/); // dashes and tildes inside an address are kept
	assert.ok(bare.endsWith(", then more"));
});

test("a plain http address in a record is shown, not made a link (lib/links.ts: https only)", () => {
	const plain = shown("data at http://example.org/data and \\href{http://example.org/x}{there}");
	assert.ok(!plain.includes("<a "), plain);
	assert.ok(plain.includes("http://example.org/data"));
	assert.ok(plain.includes("there"));
});

test("only web addresses are made clickable", () => {
	assert.ok(!shown("\\url{javascript:alert(1)}").includes("<a "));
});

test("math is typeset, in both spellings, and text around it is cleaned", () => {
	const html = shown("Explainable $k$-medians under $\\ell_p$ Norm: \\(2.2\\times\\) faster, 5\\% better");
	assert.equal((html.match(/class="katex"/g) ?? []).length, 3);
	assert.ok(!html.includes("$") && !html.includes("\\("));
	assert.ok(html.endsWith(" faster, 5% better"));
});

test("an escaped dollar sign is money, not math", () => {
	assert.equal(read("costs \\$5 and \\$10 per run"), "costs $5 and $10 per run");
});

test("plain titles pass through untouched", () => {
	assert.equal(read("Deep Residual Learning for Image Recognition"), "Deep Residual Learning for Image Recognition");
});

test("Markdown emphasis is set, and stars that are not emphasis are left alone", () => {
	assert.equal(
		shown("We propose **CALM** (*Corrective Adaptation with Lightweight Modification*), a method."),
		"We propose <strong>CALM</strong> (<em>Corrective Adaptation with Lightweight Modification</em>), a method.",
	);
	assert.equal(read("a * b and 2*3*4 and a lone * star"), "a * b and 2*3*4 and a lone * star");
	assert.equal(shown("costs **$5** less"), "costs <strong>$5</strong> less"); // a lone dollar sign is not math
	assert.equal(
		shown("**bold** then a link https://example.org/x*y"),
		'<strong>bold</strong> then a link <a href="https://example.org/x*y" target="_blank" rel="noopener noreferrer" class="underline decoration-muted-foreground underline-offset-2 hover:decoration-foreground">https://example.org/x*y</a>',
	);
});

test("stars that are arithmetic, arguments or footnote marks are not emphasis", () => {
	for (const text of ["x**2 + y**2", "Use **kwargs and **options", "(*) marks and (*) again", "a ** b ** c"])
		assert.equal(shown(text), text);
	assert.equal(shown("**Results.** We find"), "<strong>Results.</strong> We find");
});
