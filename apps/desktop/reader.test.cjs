/** What the embedded reader may open and what its pages may reach.    node --test */
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { READER_SITES, READER_ASSETS, readable, reachable, askable, local } = require("./reader.cjs");
const { internal, question, readerGuard } = require("./reader-guard.cjs");

test("the reader opens https pages of the sites papers live on, their sub-domains included", () => {
	for (const address of [
		"https://openreview.net/forum?id=abc",
		"https://api.openreview.net/notes",
		"https://arxiv.org/pdf/2401.00001#page=3",
		"https://export.arxiv.org/abs/2401.00001",
		"https://OPENREVIEW.NET/forum",
		"https://openreview.net./forum",
	]) {
		assert.ok(readable(address), address);
	}
});

test("a look-alike site, another scheme, another port, a name and password or this computer is not opened", () => {
	for (const address of [
		"https://evil-openreview.net/forum",
		"https://openreview.net.evil.example/forum",
		"https://notarxiv.org/abs/1",
		"http://openreview.net/forum",
		"https://arxiv.org:8443/abs/1",
		"https://arxiv.org@evil.example/abs/1",
		"https://user:secret@arxiv.org/abs/1",
		"file:///C:/Windows/win.ini",
		"https://127.0.0.1:8765/api/settings",
		"https://localhost:3210/",
		"javascript:alert(1)",
		"data:text/html,<script>1</script>",
		"not a url",
		"",
	]) {
		assert.equal(readable(address), false, address);
	}
});

test("the reader never opens a page of a host that only serves the paper sites' scripts and fonts", () => {
	for (const host of READER_ASSETS) assert.equal(readable(`https://${host}/`), false, host);
});

test("every way of writing this computer's address is local", () => {
	for (const address of [
		"http://127.0.0.1:8765/api/runs",
		"http://127.1:8765/",
		"http://2130706433:8765/",
		"http://0x7f000001/",
		"http://127.0.0.2/",
		"http://0.0.0.0:3210/",
		"http://localhost:3210/",
		"http://localhost.:3210/",
		"http://app.localhost/",
		"http://LOCALHOST/",
		"http://[::1]:8765/",
		"http://[::]:8765/",
		"http://[::ffff:127.0.0.1]:8765/",
		"ws://127.0.0.1:9229/",
	]) {
		assert.ok(local(address), address);
	}
});

test("the local network is local, the internet is not", () => {
	for (const address of [
		"http://192.168.1.1/",
		"http://10.0.0.5/",
		"http://172.16.0.1/",
		"http://169.254.169.254/latest",
		"http://[fe80::1]/",
		"http://[fd00::1]/",
	]) {
		assert.ok(local(address), address);
	}
	for (const address of [
		"https://openreview.net/",
		"https://172.32.0.1/",
		"https://8.8.8.8/",
		"https://[2606:4700::1111]/",
		"https://localhost.example.com/",
	]) {
		assert.equal(local(address), false, address);
	}
});

test("a page in the reader reaches the paper sites and the hosts on the list, by https or wss, and nothing else", () => {
	for (const [address, as] of [
		["https://openreview.net/notes?id=1", "xhr"],
		["https://static.arxiv.org/css/arxiv.css", "stylesheet"],
		["https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-chtml.js", "script"],
		["https://fonts.gstatic.com/s/font.woff2", "font"],
		["https://challenges.cloudflare.com/turnstile/v0/api.js", "subFrame"],
		["wss://openreview.net/socket", "webSocket"],
	]) {
		assert.ok(reachable(address, as), address);
	}
	for (const [address, as] of [
		["http://127.0.0.1:8765/api/runs", "xhr"],
		["https://127.0.0.1:8765/api/runs", "xhr"],
		["https://localhost:3210/", "image"],
		["wss://[::1]:8765/", "webSocket"],
		["http://arxiv.org/abs/1", "xhr"],
		["https://arxiv.org:8443/abs/1", "xhr"],
		["https://evil.example/collect", "ping"],
		["https://www.google-analytics.com/collect", "script"],
		["https://sub.cdn.jsdelivr.net/x.js", "script"],
		["https://cdn.jsdelivr.net.evil.example/x.js", "script"],
		["https://192.168.0.1/", "image"],
		["https://8.8.8.8/", "image"],
		["https://[2606:4700::1111]/", "image"],
		["file:///C:/Users/", "subFrame"],
		["ftp://example.com/", "other"],
		["garbage", "xhr"],
	]) {
		assert.equal(reachable(address, as), false, address);
	}
});

test("the page the reader shows must itself be a paper site's, whatever else a page may load", () => {
	assert.ok(reachable("https://arxiv.org/abs/1", "mainFrame"));
	assert.equal(reachable("https://cdn.jsdelivr.net/page.html", "mainFrame"), false);
	assert.equal(reachable("https://challenges.cloudflare.com/", "mainFrame"), false);
});

test("no list holds this computer, the local network or an address written as numbers", () => {
	for (const host of [...READER_SITES, ...READER_ASSETS]) {
		assert.equal(local(`https://${host}/`), false, host);
		assert.match(host, /^[a-z0-9-]+(\.[a-z0-9-]+)+$/, host);
		assert.doesNotMatch(host, /^[\d.]+$/, host);
	}
});

test("the user may be asked about a plain https site by name, and about nothing that could be this computer", () => {
	assert.equal(askable("https://journal.example.org/article/1"), "journal.example.org");
	assert.equal(askable("https://Journal.Example.org./a"), "journal.example.org");
	for (const address of [
		"http://journal.example.org/",
		"https://journal.example.org:8443/",
		"https://user@journal.example.org/",
		"https://127.0.0.1/",
		"https://localhost/",
		"https://app.localhost/",
		"https://192.168.1.1/",
		"https://8.8.8.8/",
		"https://[::1]/",
		"https://intranet/",
		"file:///C:/Windows/win.ini",
		"javascript:alert(1)",
		"garbage",
	]) {
		assert.equal(askable(address), null, address);
	}
});

test("a site the user approved is opened and reached by its own name only, and still never this computer", () => {
	const approved = new Set(["journal.example.org"]);
	assert.ok(readable("https://journal.example.org/article/1", approved));
	assert.ok(reachable("https://journal.example.org/figure.png", "image", approved));
	assert.ok(reachable("https://arxiv.org/abs/1", "xhr", approved), "the lists still hold");
	for (const address of [
		"https://cdn.journal.example.org/x.js",
		"https://other.example.org/",
		"http://journal.example.org/",
		"https://journal.example.org:8443/",
	]) {
		assert.equal(reachable(address, "script", approved), false, address);
	}
	// even a wrongly approved name cannot open the way to this computer
	const wrong = new Set(["localhost", "127.0.0.1", "app.localhost", "192.168.1.1"]);
	for (const address of ["https://localhost/", "https://127.0.0.1/", "https://app.localhost/", "https://192.168.1.1/"]) {
		assert.equal(readable(address, wrong), false, address);
		assert.equal(reachable(address, "xhr", wrong), false, address);
	}
});

test("only the page's own data and the built-in PDF viewer count as inside the browser", () => {
	for (const address of [
		"data:image/png;base64,AAAA",
		"blob:https://arxiv.org/1234",
		"about:blank",
		"chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html",
		"chrome://resources/js/load_time_data.js",
	]) {
		assert.ok(internal(address), address);
	}
	for (const address of [
		"chrome://settings/",
		"chrome://gpu",
		"chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/page.html",
		"devtools://devtools/bundled/inspector.html",
		"file:///C:/Windows/win.ini",
		"http://127.0.0.1:8765/",
		"https://evil.example/",
	]) {
		assert.equal(internal(address), false, address);
	}
});

test("the question about an unknown site says it is not a paper site, that it may not work, and defaults to no", () => {
	const asked = question("journal.example.org");
	assert.match(asked.message, /journal\.example\.org is not one of the paper sites/);
	assert.match(asked.detail, /sandbox/);
	assert.match(asked.detail, /may not load or may not work/);
	assert.deepEqual(asked.answers, ["no", "browser", "reader"]);
	assert.equal(asked.buttons.length, asked.answers.length);
	assert.equal(asked.buttons[0], "Cancel");
});

test("an embedded view gets no Node, preload, pop-up or dialog, whatever the page embedding it asks for", () => {
	const { VIEW } = require("./reader-guard.cjs");
	const attached = [];
	const guard = readerGuard({ ask: async () => "no", openOutside: () => {} });
	const window = { webContents: { on: (name, listener) => name === "will-attach-webview" && attached.push(listener) } };
	guard.guardWindow(window);
	const preferences = {
		preload: "file:///evil.js",
		nodeIntegration: true,
		enableBlinkFeatures: "Serial",
		disablePopups: false,
		disableDialogs: false,
		webSecurity: false,
		partition: "persist:main",
		zoomFactor: 1.25,
	};
	let refused = false;
	attached[0]({ preventDefault: () => (refused = true) }, preferences, { src: "https://arxiv.org/abs/1", partition: "reader" });
	assert.equal(refused, false);
	assert.deepEqual(preferences, { ...VIEW, zoomFactor: 1.25 });
	assert.equal(VIEW.disableDialogs, true);
	assert.equal(VIEW.disablePopups, true);
	assert.equal(VIEW.sandbox, true);
	assert.equal(VIEW.nodeIntegration, false);
});

test("the app keeps Chromium's WebRTC from UDP and from answering for names on the local network", () => {
	const { quietNetwork } = require("./reader-guard.cjs");
	const switches = [];
	quietNetwork({ commandLine: { appendSwitch: (name, value) => switches.push(`${name}=${value}`) } });
	assert.ok(switches.includes("force-webrtc-ip-handling-policy=disable_non_proxied_udp"));
	assert.ok(switches.includes("disable-features=WebRtcHideLocalIpsWithMdns"));
});

test("a host the user is asked about is written in plain letters: a look-alike shows as what it is", () => {
	// a Cyrillic "а" in place of the Latin one: the question names xn--rxiv-..., not something that reads "arxiv"
	assert.match(askable("https://аrxiv.org/abs/1"), /^xn--/);
	assert.equal(readable("https://аrxiv.org/abs/1"), false);
});

test("the reader's connections go directly only to the sites on the lists; everything else, this computer too, nowhere", () => {
	const { proxyRules, NOWHERE } = require("./reader.cjs");
	const rules = proxyRules(new Set(["journal.example.org"]));
	assert.equal(rules.mode, "fixed_servers");
	assert.equal(rules.proxyRules, NOWHERE);
	assert.match(NOWHERE, /^127\.0\.0\.1:1$/);
	const direct = rules.proxyBypassRules.split(";");
	// Chromium's own exception for this computer is taken away, so that it too goes to the proxy that is not there
	assert.equal(direct[0], "<-loopback>");
	for (const host of ["arxiv.org", "*.arxiv.org", "cdn.jsdelivr.net", "journal.example.org"]) assert.ok(direct.includes(host), host);
	for (const host of direct.slice(1)) {
		assert.doesNotMatch(host, /localhost|^\*$|^[\d.]+$|\[|^\*\.[^.]+$/, host);
	}
	assert.equal(proxyRules().proxyBypassRules.includes("journal.example.org"), false);
});

test("a guard starts with nothing approved and nothing declined", () => {
	const guard = readerGuard({ ask: async () => "no", openOutside: () => {} });
	assert.equal(guard.approved.size, 0);
	assert.equal(guard.declined.size, 0);
});
