/**
 * What the embedded reader may show and reach. The reader is a view inside the window that shows somebody else's
 * web page (a paper's page, its reviews), so everything here is about keeping that page away from this app, this
 * computer and everything else.
 *
 * The rule is a list of what is allowed, not of what is forbidden: a page in the reader reaches the sites papers
 * live on, the few hosts those sites load their scripts, styles and fonts from, and nothing else at all. Nothing
 * on this computer or on the local network is on that list, however its address is written, and a name that an
 * attacker controls is not on it either, so pointing such a name at this computer (DNS rebinding) leads nowhere.
 *
 *   readable(address)        may the reader open this page? Only https, on the usual port, on the sites papers
 *                            live on (READER_SITES), a site's own sub-domains included; "evil-openreview.net" is
 *                            not "openreview.net".
 *   reachable(address, as)   may a page in the reader make this request? `as` is what the request is for
 *                            (Electron's resourceType). The page itself ("mainFrame") must be readable; anything
 *                            a page loads must be on a paper site or on READER_ASSETS, by https or wss.
 *   askable(address)         a page on no list: may the user be asked whether to open it in the reader all the
 *                            same? Only a plain https address by name, never this computer or the local network.
 *                            A site the user said yes to is `approved` for as long as the app runs: the reader
 *                            opens its pages and lets them reach that one host and the lists, nothing more, so
 *                            parts of such a page may be missing. It runs in the same sandbox as every other.
 *   local(address)           does this address lead to this computer or the local network? Not what the rule
 *                            rests on (the list is), but refused first and tested on its own, so that no later
 *                            addition to the lists can open the way to this computer.
 *
 * Nothing in this file needs Electron: reader-guard.cjs applies it.
 */

/** The embedded reader's storage partition (set by the interface: apps/web/components/paper-reader.tsx). */
const READER = "reader";

/** The sites the reader opens pages of: where papers and their reviews live. Their sub-domains count. */
const READER_SITES = [
	"openreview.net",
	"arxiv.org",
	"proceedings.neurips.cc",
	"proceedings.mlr.press",
	"openaccess.thecvf.com",
	"aclanthology.org",
];

/**
 * The hosts those sites load their own scripts, styles, fonts and images from, as seen in their pages (exactly
 * these names, no sub-domains), and the one that draws OpenReview's "verify you are human" box, which the user
 * answers themself. The reader never opens a page of any of them. A host a site uses only to count its visitors
 * is left out on purpose.
 */
const READER_ASSETS = [
	"cdn.jsdelivr.net",
	"cdnjs.cloudflare.com",
	"code.jquery.com",
	"ajax.googleapis.com",
	"fonts.googleapis.com",
	"fonts.gstatic.com",
	"use.fontawesome.com",
	"stackpath.bootstrapcdn.com",
	"maxcdn.bootstrapcdn.com",
	"i.creativecommons.org",
	"challenges.cloudflare.com",
];

function parsed(address) {
	try {
		return new URL(address);
	} catch {
		return null;
	}
}

/** The host of an address, as compared here: lower case, without the dot a full name may end with. */
function hostOf(url) {
	return url.hostname.replace(/\.$/, "").toLowerCase();
}

/** Is this an address a page may be fetched from at all: the given schemes, the usual port, no name and password? */
function plain(url, schemes) {
	return schemes.includes(url.protocol) && url.port === "" && url.username === "" && url.password === "";
}

function onPaperSite(host) {
	return READER_SITES.some((site) => host === site || host.endsWith(`.${site}`));
}

/** No site the user approved: the lists alone. */
const NONE = new Set();

function readable(address, approved = NONE) {
	const url = parsed(address);
	if (url === null || !plain(url, ["https:"]) || local(address)) return false;
	const host = hostOf(url);
	return !numeric(host) && (onPaperSite(host) || approved.has(host));
}

/** The host to ask the user about, when `address` is one they may be asked about; null when it is never opened. */
function askable(address) {
	const url = parsed(address);
	if (url === null || !plain(url, ["https:"]) || local(address)) return null;
	const host = hostOf(url);
	return numeric(host) || !host.includes(".") ? null : host;
}

/** An IPv4 address in dotted form (the URL parser writes every other form of one, 0x7f.1 or 2130706433, so). */
function ipv4(host) {
	const parts = host.split(".");
	if (parts.length !== 4 || !parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)) return null;
	return parts.map(Number);
}

/** Is this IPv4 address this computer, nowhere, or the local network (private, link-local, shared)? */
function localIpv4([a, b]) {
	return (
		a === 0 ||
		a === 127 ||
		a === 10 ||
		(a === 172 && b >= 16 && b <= 31) ||
		(a === 192 && b === 168) ||
		(a === 169 && b === 254) ||
		(a === 100 && b >= 64 && b <= 127)
	);
}

/** Is this IPv6 address (as the URL parser writes it, without brackets) this computer, nowhere, or the local network? */
function localIpv6(host) {
	if (host === "::" || host === "::1") return true;
	// an IPv4 address written as IPv6: ::ffff:7f00:1 is 127.0.0.1
	const mapped = /^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(host);
	if (mapped) {
		const [high, low] = [Number.parseInt(mapped[1], 16), Number.parseInt(mapped[2], 16)];
		return localIpv4([high >> 8, high & 255, low >> 8, low & 255]);
	}
	const first = Number.parseInt(host.split(":")[0] || "0", 16);
	return (first & 0xfe00) === 0xfc00 || (first & 0xffc0) === 0xfe80; // unique local fc00::/7, link-local fe80::/10
}

/** Does `address` lead to this computer or the local network? */
function local(address) {
	const url = parsed(address);
	if (!url) return false;
	const host = hostOf(url);
	if (host === "localhost" || host.endsWith(".localhost")) return true; // the browser sends every *.localhost here
	if (host.startsWith("[")) return localIpv6(host.slice(1, -1));
	const four = ipv4(host);
	return four !== null && localIpv4(four);
}

/** Is `host` an address written as numbers rather than a name? No list here has one, and none is ever reached. */
function numeric(host) {
	return host.startsWith("[") || ipv4(host) !== null;
}

/**
 * May a page in the reader make this request? `as` is what it is for, in Electron's words ("mainFrame",
 * "subFrame", "script", "image", "xhr", "webSocket"...). Whatever is not plainly allowed is refused.
 */
function reachable(address, as, approved = NONE) {
	if (as === "mainFrame") return readable(address, approved);
	const url = parsed(address);
	if (!url || !plain(url, ["https:", "wss:"]) || local(address)) return false;
	const host = hostOf(url);
	if (numeric(host)) return false;
	return onPaperSite(host) || READER_ASSETS.includes(host) || approved.has(host);
}

module.exports = { READER, READER_SITES, READER_ASSETS, readable, reachable, askable, local };
