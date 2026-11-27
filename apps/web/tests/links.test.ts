/** Addresses from records before they are opened (lib/links.ts), and what the reader shows when a page fails
 *  (lib/reader/pdf-frame.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import { isOpenReviewPage, safeLink } from "@/lib/links";
import { framedPdf, pageFailed } from "@/lib/reader/pdf-frame";
import { outsideLink } from "@/lib/reader/citations";

test("only https addresses from a record are opened; plain http only to this computer and only when asked", () => {
	assert.equal(safeLink("https://arxiv.org/abs/2401.00001")?.href, "https://arxiv.org/abs/2401.00001");
	assert.equal(safeLink("  https://openreview.net/forum?id=x  ")?.href, "https://openreview.net/forum?id=x");
	for (const refused of [
		"javascript:alert(1)",
		"JAVASCRIPT:alert(1)",
		"data:text/html,<script>1</script>",
		"file:///C:/Windows/win.ini",
		"http://example.org/paper.pdf",
		"//example.org/paper",
		"not an address",
		"",
		null,
		undefined,
		"https://openreview.net@evil.example/forum",
	])
		assert.equal(safeLink(refused), null, String(refused));
	assert.equal(safeLink("http://127.0.0.1:3210/api/pdf/x"), null, "this computer, but not asked for");
	assert.equal(safeLink("http://127.0.0.1:3210/api/pdf/x", { local: true })?.host, "127.0.0.1:3210");
	assert.equal(safeLink("http://localhost:3210/", { local: true })?.hostname, "localhost");
	assert.equal(safeLink("http://evil.example/", { local: true }), null, "another site over http, even with local");
});

test("a paper's page is embedded as its reviews only when its host is openreview.net or a sub-domain of it", () => {
	assert.equal(isOpenReviewPage("https://openreview.net/forum?id=abc"), true);
	assert.equal(isOpenReviewPage("https://api.openreview.net/notes?id=abc"), true);
	assert.equal(isOpenReviewPage("https://OpenReview.net/forum?id=abc"), true);
	assert.equal(isOpenReviewPage("https://evil.example/?openreview.net"), false, "the name in the query");
	assert.equal(isOpenReviewPage("https://openreview.net.evil.example/forum"), false, "a longer host");
	assert.equal(isOpenReviewPage("https://notopenreview.net/forum"), false);
	assert.equal(isOpenReviewPage("http://openreview.net/forum?id=abc"), false, "not over https");
	assert.equal(isOpenReviewPage("javascript://openreview.net/%0Aalert(1)"), false);
	assert.equal(isOpenReviewPage(null), false);
});

test("a work of a reference list whose address is not https is looked up by its title instead", () => {
	const work = { title: "Attention is all you need", url: "javascript:alert(1)" } as Parameters<typeof outsideLink>[0];
	assert.equal(outsideLink(work), "https://scholar.google.com/scholar?q=Attention%20is%20all%20you%20need");
});

test("the PDF frame shows the engine's words when it answered an error instead of a PDF", () => {
	assert.deepEqual(framedPdf("application/pdf", ""), { kind: "pdf" });
	assert.deepEqual(framedPdf("application/json", '{"error": "the PDF could not be fetched: 404 from the publisher"}'), {
		kind: "problem",
		words: "the PDF could not be fetched: 404 from the publisher",
	});
	assert.deepEqual(framedPdf("text/html", ""), { kind: "problem", words: "Constellate sent nothing for this PDF." });
	assert.deepEqual(framedPdf("text/html", "<h1>Bad gateway</h1>"), { kind: "problem", words: "Constellate did not send a PDF." });
	assert.deepEqual(framedPdf("application/json", '{"error": ""}'), { kind: "problem", words: "Constellate did not send a PDF." });
});

test("a page of the embedded view failed only when its main frame did, and not when a navigation called it off", () => {
	const failed = {
		errorCode: -105,
		errorDescription: "ERR_NAME_NOT_RESOLVED",
		validatedURL: "https://openreview.net/forum?id=x",
		isMainFrame: true,
	};
	assert.equal(pageFailed(failed), true);
	assert.equal(pageFailed({ ...failed, isMainFrame: false }), false, "a frame inside the page");
	assert.equal(pageFailed({ ...failed, errorCode: -3 }), false, "called off by the next navigation");
});
