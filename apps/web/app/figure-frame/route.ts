import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { BOOTSTRAP, framePage, framePolicy } from "@/lib/reader/figure-frame";

/**
 * The page a paper's figures are drawn in (lib/reader/figure-frame.ts says how it is confined): pdf.js and its
 * worker from the installed package, carried in the page itself so that the frame needs no request of its own.
 * Made once per server and kept.
 */
const HASH = createHash("sha256").update(BOOTSTRAP).digest("base64");
let page: Promise<string> | undefined;

/** A file of pdf.js's build: in the workspace's node_modules when developing, in the standalone server's when
 *  built (next.config.ts traces both files there); the server runs from apps/web in either. */
function pdfjsFile(name: string): string {
	const candidates = [
		path.join(process.cwd(), "..", "..", "node_modules", "pdfjs-dist", "build", name),
		path.join(process.cwd(), "node_modules", "pdfjs-dist", "build", name),
	];
	const found = candidates.find((candidate) => existsSync(candidate));
	if (!found) throw new Error(`pdf.js is not installed (${name})`);
	return found;
}

function built(): Promise<string> {
	page ??= Promise.all([readFile(pdfjsFile("pdf.min.mjs")), readFile(pdfjsFile("pdf.worker.min.mjs"))]).then(([pdfjs, worker]) =>
		framePage(pdfjs.toString("base64"), worker.toString("base64")),
	);
	page.catch(() => (page = undefined));
	return page;
}

export async function GET() {
	return new Response(await built(), {
		headers: {
			"Content-Type": "text/html; charset=utf-8",
			"Content-Security-Policy": framePolicy(HASH),
			"X-Content-Type-Options": "nosniff",
			"Referrer-Policy": "no-referrer",
			"Cache-Control": "no-store",
		},
	});
}
