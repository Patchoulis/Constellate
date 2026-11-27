/**
 * The desktop shell: one window around the app's two local servers.
 *
 *   backend   python -m rp.server           the pipeline and its HTTP API, on 127.0.0.1:8765
 *   web       Next's standalone server.js   the UI, on a free port; it forwards /api to the backend
 *
 * Both are child processes of this shell and die with it. The web server is run by Electron's own Node
 * (process.execPath + ELECTRON_RUN_AS_NODE=1), so no Node installation is needed; Python is.
 *
 * Two ways to run:
 *   - built (the default): the servers come from apps/web/.next/standalone and apps/backend when started from
 *     the repository, or from the app's resources folder when packaged. `npm run build` makes them.
 *   - RP_DEV_URL=http://localhost:3210: nothing is spawned; the window shows servers started by `npm run dev`.
 *
 * Startup is a small state machine: starting -> ready -> quitting, or starting -> failed (an error box, then quit).
 */
const { app, BrowserWindow, dialog, session, shell } = require("electron");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");

const BACKEND_PORT = 8765; // the web build forwards /api here (apps/web/next.config.ts)
const DEV_URL = process.env.RP_DEV_URL;
const REPO = path.join(__dirname, "..", "..");

/** Where the two servers and the data live: inside the installed app, or in the repository. */
const places = app.isPackaged
	? {
			backend: path.join(process.resourcesPath, "backend"),
			web: path.join(process.resourcesPath, "web", "apps", "web", "server.js"),
			data: path.join(app.getPath("userData"), "data"),
		}
	: {
			backend: path.join(REPO, "apps", "backend"),
			web: path.join(REPO, "apps", "web", ".next", "standalone", "apps", "web", "server.js"),
			data: path.join(REPO, "data"),
		};

let state = "starting";
const children = [];

function freePort() {
	return new Promise((resolve, reject) => {
		const probe = net.createServer();
		probe.once("error", reject);
		probe.listen(0, "127.0.0.1", () => {
			const { port } = probe.address();
			probe.close(() => resolve(port));
		});
	});
}

function answers(url) {
	return new Promise((resolve) => {
		const request = http.get(url, (response) => {
			response.resume();
			resolve(true);
		});
		request.once("error", () => resolve(false));
		request.setTimeout(2000, () => request.destroy());
	});
}

/** Start a child server and resolve once `url` answers; reject if it exits first or takes too long. */
async function serve(name, command, args, options, url) {
	if (await answers(url)) return; // already running (e.g. the backend started by `npm run dev`)
	const child = spawn(command, args, { ...options, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
	children.push(child);
	let output = "";
	const keep = (chunk) => (output = (output + chunk).slice(-4000));
	child.stdout.on("data", keep);
	child.stderr.on("data", keep);
	const exited = new Promise((_, reject) => {
		child.once("error", (problem) => reject(new Error(`${name} could not start: ${problem.message}`)));
		child.once("exit", (code) => state === "starting" && reject(new Error(`${name} stopped (code ${code}).\n\n${output}`)));
	});
	const up = (async () => {
		for (let attempt = 0; attempt < 240; attempt++) {
			if (await answers(url)) return;
			await new Promise((resolve) => setTimeout(resolve, 250));
		}
		throw new Error(`${name} did not answer at ${url}.\n\n${output}`);
	})();
	await Promise.race([up, exited]);
}

async function startServers() {
	if (DEV_URL) return DEV_URL;
	if (!fs.existsSync(places.web)) throw new Error(`The app is not built yet (${places.web} is missing). Run: npm run build`);
	const port = await freePort();
	const url = `http://127.0.0.1:${port}`;
	await Promise.all([
		serve(
			"The backend (python -m rp.server)",
			process.env.RP_PYTHON || "python",
			["-W", "ignore", "-m", "rp.server"],
			{ cwd: places.backend, env: { ...process.env, RP_PORT: String(BACKEND_PORT), RP_DATA: places.data, RP_ORIGIN: url } },
			`http://127.0.0.1:${BACKEND_PORT}/api/settings`,
		),
		serve(
			"The interface",
			process.execPath,
			[places.web],
			{
				cwd: path.dirname(places.web),
				env: { ...process.env, ELECTRON_RUN_AS_NODE: "1", PORT: String(port), HOSTNAME: "127.0.0.1" },
			},
			url,
		),
	]);
	return url;
}

function openWindow(url) {
	const window = new BrowserWindow({
		width: 1500,
		height: 940,
		minWidth: 1000,
		minHeight: 640,
		backgroundColor: "#141414",
		autoHideMenuBar: true,
		title: app.getName(),
		icon: path.join(__dirname, "icon.png"),
		// the reader shows a paper's own page (and its reviews) inside the window, in an embedded view
		webPreferences: { webviewTag: true },
	});
	// An embedded view shows somebody else's web page: it gets no access to this app at all, its own storage
	// (the "reader" partition, set by the interface), no permissions, and only the sites papers live on.
	window.webContents.on("will-attach-webview", (event, preferences, params) => {
		delete preferences.preload;
		delete preferences.preloadURL;
		preferences.nodeIntegration = false;
		preferences.nodeIntegrationInSubFrames = false;
		preferences.contextIsolation = true;
		preferences.sandbox = true;
		preferences.webSecurity = true;
		if (!readable(params.src) || params.partition !== READER) event.preventDefault();
	});
	window.webContents.on("did-attach-webview", (_event, guest) => {
		// The view itself only goes to the sites papers live on. A page cannot send the user's browser anywhere
		// by navigating: only a link the user opens in a new window (below) leaves the app.
		const stay = (event, target) => {
			if (event.isMainFrame !== false && !readable(target)) event.preventDefault();
		};
		guest.on("will-navigate", stay);
		guest.on("will-redirect", stay);
		// A frame inside a shown page must not load this app's own interface or API.
		guest.on("will-frame-navigate", (event) => {
			if (!event.isMainFrame && local(event.url)) event.preventDefault();
		});
		guest.setWindowOpenHandler(({ url: target }) => {
			if (/^https:\/\//.test(target)) void shell.openExternal(target);
			return { action: "deny" };
		});
	});
	// Nothing opens a second app window: a link that leaves the app goes to the user's browser.
	window.webContents.setWindowOpenHandler(({ url: target }) => {
		if (/^https:\/\//.test(target)) void shell.openExternal(target);
		return { action: "deny" };
	});
	void window.loadURL(url);
}

/** The embedded reader's storage partition, and the sites it may show: where papers and their reviews live. */
const READER = "reader";
const READER_SITES = ["openreview.net", "arxiv.org", "proceedings.neurips.cc", "proceedings.mlr.press", "openaccess.thecvf.com", "aclanthology.org"];

function readable(address) {
	try {
		const { protocol, hostname } = new URL(address);
		return protocol === "https:" && READER_SITES.some((site) => hostname === site || hostname.endsWith(`.${site}`));
	} catch {
		return false;
	}
}

function local(address) {
	try {
		return ["127.0.0.1", "localhost", "[::1]", "::1"].includes(new URL(address).hostname);
	} catch {
		return false;
	}
}

/** Nothing in this app needs the camera, the microphone, the location...: every permission request is refused. */
function refusePermissions() {
	for (const each of [session.defaultSession, session.fromPartition(READER)]) {
		each.setPermissionRequestHandler((_contents, _permission, answer) => answer(false));
		each.setPermissionCheckHandler(() => false);
	}
}

function stopServers() {
	state = "quitting";
	for (const child of children) child.kill();
}

app.whenReady().then(async () => {
	try {
		const url = await startServers();
		state = "ready";
		refusePermissions();
		openWindow(url);
	} catch (problem) {
		state = "failed";
		dialog.showErrorBox(`${app.getName()} could not start`, problem.message);
		app.quit();
	}
});
app.on("window-all-closed", () => app.quit());
app.on("before-quit", stopServers);
