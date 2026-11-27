/**
 * Addresses taken from records (a paper's page, its PDF, a work of a reference list read from a PDF) are the
 * records' words, not ours: one could be `javascript:`, `file:` or a page of this computer. Every one that is
 * opened in the browser or shown in the reader's embedded view goes through here first.
 */

/** Names this computer is reached by. */
const THIS_COMPUTER = new Set(["localhost", "127.0.0.1", "[::1]"]);

export interface LinkRules {
	/** allow `http:` to this computer (the interface's own pages); never another site over plain http */
	local?: boolean;
}

/** The address, parsed, when it may be opened: `https:` anywhere, `http:` only to this computer when allowed. */
export function safeLink(address: string | null | undefined, rules: LinkRules = {}): URL | null {
	if (!address) return null;
	let url: URL;
	try {
		url = new URL(address.trim());
	} catch {
		return null;
	}
	if (url.username || url.password) return null; // "https://openreview.net@elsewhere.example" and its like
	if (url.protocol === "https:") return url;
	if (url.protocol === "http:" && rules.local && THIS_COMPUTER.has(url.hostname.toLowerCase())) return url;
	return null;
}

/** Open the address in the user's browser when it may be; answers whether it was opened. */
export function openOutside(address: string | null | undefined, rules: LinkRules = {}): boolean {
	const url = safeLink(address, rules);
	if (!url) return false;
	window.open(url.href, "_blank", "noopener,noreferrer");
	return true;
}

/** Is this address a page of OpenReview (openreview.net or one of its sub-domains), over https? */
export function isOpenReviewPage(address: string | null | undefined): boolean {
	const host = safeLink(address)?.hostname.toLowerCase();
	return host === "openreview.net" || (host?.endsWith(".openreview.net") ?? false);
}
