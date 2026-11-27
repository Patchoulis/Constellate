/**
 * What the embedded reader may show and reach. The reader is a view inside the window that shows somebody else's
 * web page (a paper's page, its reviews), so everything here is about keeping that page away from this app.
 *
 *   readable(address)   may the reader open this page? Only https, only on the sites papers live on (READER_SITES),
 *                       a site's own sub-domains included; "evil-openreview.net" is not "openreview.net".
 *   refused(address)    must a request made by a page in the reader (a frame, a script's fetch, an image, a form)
 *                       be stopped? Every one that is not https or wss, and every one to this computer or to the
 *                       local network: that is where this app's interface, its API and the engine listen, and a
 *                       page must not be able to send them anything (they would take it for the app).
 *
 * Nothing in this file needs Electron.
 */

/** The embedded reader's storage partition (set by the interface: apps/web/components/paper-reader.tsx). */
const READER = "reader";

/** The sites the reader opens pages of: where papers and their reviews live. */
const READER_SITES = [
	"openreview.net",
	"arxiv.org",
	"proceedings.neurips.cc",
	"proceedings.mlr.press",
	"openaccess.thecvf.com",
	"aclanthology.org",
];

function parsed(address) {
	try {
		return new URL(address);
	} catch {
		return null;
	}
}

function readable(address) {
	const url = parsed(address);
	if (!url || url.protocol !== "https:") return false;
	const host = url.hostname.replace(/\.$/, "").toLowerCase();
	return READER_SITES.some((site) => host === site || host.endsWith(`.${site}`));
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
	const host = url.hostname.replace(/\.$/, "").toLowerCase();
	if (host === "localhost" || host.endsWith(".localhost")) return true; // the browser sends every *.localhost here
	if (host.startsWith("[")) return localIpv6(host.slice(1, -1));
	const four = ipv4(host);
	return four !== null && localIpv4(four);
}

function refused(address) {
	const url = parsed(address);
	return !url || !["https:", "wss:"].includes(url.protocol) || local(address);
}

module.exports = { READER, READER_SITES, readable, local, refused };
