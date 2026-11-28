/** Citations: their labels, what hovering says, where they open the reader, and what is highlighted there. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { Evidence, PaperPassages, PassageAddress } from "@rp/contracts";
import { EvidenceLink, EvidenceProvider } from "@/components/reader/evidence-link";
import {
	codePoints,
	evidenceTitle,
	highlighted,
	passageAt,
	pdfAt,
	placeName,
	readerTarget,
	shortLabel,
	scriptParts,
	unreadableParts,
	whollyUnreadable,
} from "@/lib/reader/evidence";
import { UNREADABLE } from "@rp/contracts";

const address = (page: number, start: number, end: number, section = "2. Method"): PassageAddress => ({
	page,
	section,
	paragraph: 1,
	start,
	end_page: page,
	end,
});

const verified: Evidence = {
	paper: "2501.00001",
	quote: "rescale the inflated singular values",
	established: { kind: "verified", address: address(4, 120, 156), matched: { kind: "exact" } },
};

test("a paper is labelled as citations name it", () => {
	assert.equal(
		shortLabel({ authors: ["Ashish Vaswani", "Noam Shazeer", "Niki Parmar"], published: "2017-06-12T00:00:00Z" }),
		"Vaswani et al., 2017",
	);
	assert.equal(shortLabel({ authors: ["Vaswani, A.", "Shazeer, N."], published: "2017" }), "Vaswani and Shazeer, 2017");
	assert.equal(shortLabel({ authors: ["Ada Lovelace"], published: "" }), "Lovelace");
	assert.equal(shortLabel({ authors: [], published: "2026-01-02" }), "Unknown authors, 2026");
});

test("hovering a citation gives the paper's words, where they are, and how they were found", () => {
	const title = evidenceTitle(verified, "Lovelace et al., 2025");
	assert.match(title, /^“rescale the inflated singular values”/);
	assert.match(title, /p\. 4, 2\. Method/);
	assert.match(title, /word for word/);
	const close = evidenceTitle(
		{ ...verified, established: { kind: "verified", address: address(4, 120, 156), matched: { kind: "close", differences: 2 } } },
		"x",
	);
	assert.match(close, /2 letters different from the model's quote/);
	const fromAbstract = evidenceTitle(
		{ ...verified, established: { kind: "abstract", field: "abstract", start: 0, end: 30, matched: { kind: "exact" } } },
		"x",
	);
	assert.match(fromAbstract, /Only the paper's title and abstract were checked/);
});

test("a place across a page break says both pages", () => {
	assert.equal(placeName({ ...address(4, 10, 20), end_page: 5 }), "pp. 4–5, 2. Method");
	assert.equal(placeName(address(1, 0, 5, "")), "p. 1");
});

test("the PDF is opened at the cited page", () => {
	assert.equal(pdfAt("/api/pdf/2501.00001", 4), "/api/pdf/2501.00001#page=4");
	assert.equal(pdfAt("/api/pdf/2501.00001#page=2", 7), "/api/pdf/2501.00001#page=7");
	assert.equal(pdfAt("/api/pdf/2501.00001", undefined), "/api/pdf/2501.00001");
});

test("characters are counted as the engine counts them, by code point", () => {
	const text = "𝐖 is the update";
	assert.equal(codePoints(text, 2, 4), "is");
	assert.notEqual(text.slice(2, 4), "is", "UTF-16 slicing would be off by one");
});

const passages: PaperPassages = {
	reader: 1,
	pages: [{ number: 4, text: "2. Method\n\nWe 𝐖 rescale the inflated singular values." }],
	passages: [
		{ kind: "heading", part: "body", flow: "starts", address: address(4, 0, 9), text: "2. Method", sentences: [] },
		{
			kind: "text",
			part: "body",
			flow: "starts",
			address: address(4, 11, 53),
			text: "We 𝐖 rescale the inflated singular values.",
			sentences: [{ start: 11, end: 53 }],
		},
	],
};

test("the cited passage is found and its words highlighted", () => {
	const place = address(4, 16, 52);
	const index = passageAt(passages, place);
	assert.equal(index, 1);
	assert.deepEqual(highlighted(passages.passages[index], place), ["We 𝐖 ", "rescale the inflated singular values", "."]);
	assert.equal(highlighted(passages.passages[0], place), null);
	assert.equal(passageAt(passages, address(9, 0, 1)), -1);
});

test("a citation is a button that opens the reader there, and finds the paper on the graph when it is in the run", () => {
	const opened: unknown[] = [];
	const actions = { open: (target: unknown) => opened.push(target), inRun: () => true, locate: () => {} };
	const markup = renderToStaticMarkup(
		<EvidenceProvider value={actions}>
			<EvidenceLink evidence={verified} label="Lovelace et al., 2025" />
		</EvidenceProvider>,
	);
	assert.match(markup, />Lovelace et al\., 2025</);
	assert.match(markup, /title="“rescale the inflated singular values”/);
	assert.match(markup, /Show this paper on the graph/);
	assert.deepEqual(readerTarget(verified), { paper: "2501.00001", address: address(4, 120, 156), quote: verified.quote });
	const alone = renderToStaticMarkup(<EvidenceLink evidence={verified} label="x" />);
	assert.match(alone, /disabled/, "outside a workspace there is nowhere to open it");
});

test("a formula the PDF could not say is cut out of the text for a note, and the words around it stay as they are", () => {
	assert.deepEqual(unreadableParts(`the bound ${UNREADABLE} holds`), ["the bound ", null, " holds"]);
	assert.deepEqual(unreadableParts(`${UNREADABLE} (12)`), [null, " (12)"]);
	assert.deepEqual(unreadableParts("we use p, q, r and s"), ["we use p, q, r and s"]);
	assert.equal(whollyUnreadable({ text: `${UNREADABLE} (12)` }), true, "an equation number is not text to explain");
	assert.equal(whollyUnreadable({ text: `the bound ${UNREADABLE} holds` }), false);
	assert.equal(whollyUnreadable({ text: "(1)" }), false, "nothing unreadable in it");
});

test("scripts the engine marks are raised and lowered, their marks left out, and a cut mark is shown as it is", () => {
	assert.deepEqual(scriptParts("CoT^{gt}_{d} and R^{m},"), [
		{ text: "CoT", script: null },
		{ text: "gt", script: "sup" },
		{ text: "d", script: "sub" },
		{ text: " and R", script: null },
		{ text: "m", script: "sup" },
		{ text: ",", script: null },
	]);
	assert.deepEqual(
		scriptParts("(1)/(|T^{gt}_{d}|)")
			.map((part) => part.text)
			.join(""),
		"(1)/(|Tgtd|)",
	);
	assert.deepEqual(scriptParts("CoT^{g"), [{ text: "CoT^{g", script: null }]);
});
