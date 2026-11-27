/**
 * The desktop shell: one window around the app's two local servers.
 *
 *   backend   python -m rp.server           the pipeline and its HTTP API, on 127.0.0.1:8765
 *   web       Next's standalone server.js   the UI, on a free port; it forwards /api to the backend
 *
 * Both are child processes of this shell and die with it. The web server is run by Electron's own Node
 * (process.execPath + ELECTRON_RUN_AS_NODE=1), so no Node installation is needed. The installed app also brings
 * its own Python: setup.cjs makes it on the first start, shown in a small window. From the repository, the
 * Python on the path is used (RP_PYTHON to choose another).
 *
 * Two ways to run:
 *   - built (the default): the servers come from apps/web/.next/standalone and apps/backend when started from
 *     the repository, or from the app's resources folder when packaged. `npm run build` makes them.
 *   - RP_DEV_URL=http://localhost:3210: nothing is spawned; the window shows servers started by `npm run dev`.
 *
 * Where the app is between opening and closing is one explicit state machine, lifecycle.cjs: every decision
 * below that depends on it (may a process start, does a window closing mean stop) asks it rather than a flag.
 */
const { app, BrowserWindow, dialog, session, shell } = require("electron");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { Lifecycle } = require("./lifecycle.cjs");
const setup = require("./setup.cjs");

const BACKEND_PORT = 8765; // the web build forwards /api here (apps/web/next.config.ts)
const DEV_URL = process.env.RP_DEV_URL;
const REPO = path.join(__dirname, "..", "..");

/** Where the two servers and the data live: inside the installed app, or in the repository. */
const places = app.isPackaged
	? {
			backend: path.join(process.resourcesPath, "backend"),
			uv: path.join(process.resourcesPath, "uv", process.platform === "win32" ? "uv.exe" : "uv"),
			python: path.join(app.getPath("userData"), "python"),
			web: path.join(process.resourcesPath, "web", "apps", "web", "server.js"),
			data: path.join(app.getPath("userData"), "data"),
		}
	: {
			backend: path.join(REPO, "apps", "backend"),
			web: path.join(REPO, "apps", "web", ".next", "standalone", "apps", "web", "server.js"),
			data: path.join(REPO, "data"),
		};

const life = new Lifecycle();
const children = []; // the two servers
let installer = null; // the process setting Python up, while there is one

// One Constellate at a time: a second one would set Python up a second time alongside the first, share the
// first one's backend and lose it when the first closes. The second one does nothing at all but quit.
const first = app.requestSingleInstanceLock();
if (!first) app.quit();
app.on("second-instance", () => {
	const [window] = BrowserWindow.getAllWindows();
	if (!window) return;
	if (window.isMinimized()) window.restore();
	window.focus();
});

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
	if (await answers(url)) {
		// From the repository that is the backend `npm run dev` started. The installed app starts its own, and one
		// that is already there belongs to something else and would refuse this window's requests.
		if (!app.isPackaged) return;
		throw new Error(
			`${name} cannot start: something else is already answering at ${url}. Close it (an earlier Constellate may still be running: end "python.exe" in Task Manager) and open Constellate again.`,
		);
	}
	if (!life.maySpawn) throw new Error(`${name} was not started: the app is closing.`);
	// stdin stays open as long as this shell lives: the backend watches it and stops when it closes (RP_EXIT_WITH_PARENT)
	const child = spawn(command, args, { ...options, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
	children.push(child);
	let output = "";
	const keep = (chunk) => (output = (output + chunk).slice(-4000));
	child.stdout.on("data", keep);
	child.stderr.on("data", keep);
	const exited = new Promise((_, reject) => {
		child.once("error", (error) => reject(new Error(`${name} could not start: ${error.message}`)));
		child.once("exit", (code) => {
			const stopped = new Error(`${name} stopped (code ${code}).\n\n${output}`);
			if (life.childExitIsFailure)
				fail(stopped); // nobody is waiting for it any more: say so and close
			else reject(stopped); // still starting up: whoever is waiting below reports it
		});
	});
	const up = (async () => {
		for (let attempt = 0; attempt < 240; attempt++) {
			if (await answers(url)) return;
			if (!life.maySpawn) throw new Error(`${name} is no longer waited for: the app is closing.`);
			await new Promise((resolve) => setTimeout(resolve, 250));
		}
		throw new Error(`${name} did not answer at ${url}.\n\n${output}`);
	})();
	await Promise.race([up, exited]);
}

/** The Python the backend runs on: the installed app's own (set up by the Installation Wizard the first time),
 *  or the one on the path. */
async function backendPython() {
	if (!app.isPackaged) return process.env.RP_PYTHON || "python";
	const wanted = { uv: places.uv, folder: places.python, requirements: path.join(places.backend, "requirements.txt") };
	let wizard = null; // the window that shows the setup, opened only if there is something to set up
	const log = (line) => {
		if (!life.is("installing")) return; // stopping: nothing more to show
		if (!wizard) {
			wizard = new BrowserWindow({
				width: 660,
				height: 440,
				backgroundColor: "#141414",
				autoHideMenuBar: true,
				resizable: false,
				title: `${app.getName()} Installation Wizard`,
				icon: path.join(__dirname, "icon.png"),
			});
			void wizard.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(WIZARD_PAGE)}`);
			// The user closing it means "not now": stop the download and quit; the next start carries on from there.
			// When the shell closes it (below), the lifecycle has already left "installing", so this does nothing.
			wizard.once("closed", () => {
				if (!life.wizardCloseMeansStop) return;
				life.move("quitting");
				installer?.kill();
				app.quit();
			});
		}
		if (wizard.isDestroyed()) return;
		const show = `document.getElementById("log").textContent += ${JSON.stringify(`${line}\n`)}; window.scrollTo(0, document.body.scrollHeight);`;
		wizard.webContents.executeJavaScript(show).catch(() => {});
	};
	life.move("installing");
	try {
		const python = await setup.ensure({ ...wanted, log, started: (child) => (installer = life.maySpawn ? child : (child.kill(), null)) });
		life.move("serving"); // before the wizard is closed, so that closing it is not taken for the user's doing
		return python;
	} finally {
		installer = null;
		if (wizard && !wizard.isDestroyed()) wizard.close();
	}
}

const WIZARD_PAGE = `<body style="margin:0;padding:20px;background:#141414;color:#d4d4d4;font:13px/1.5 ui-monospace,Consolas,monospace">
<h1 style="font-size:15px;margin:0 0 6px">Installation Wizard</h1>
<p style="margin:0 0 12px;color:#8a8a8a">Constellate is downloading the Python packages it runs on: a few gigabytes, once. It can take several minutes. Closing this window stops it; opening Constellate again carries on where it stopped.</p>
<pre id="log" style="margin:0;white-space:pre-wrap;color:#a3a3a3"></pre></body>`;

/** The environment without API keys: the interface's server shows pages and has no use for them. */
function withoutKeys(env) {
	return Object.fromEntries(
		Object.entries(env).filter(([name]) => !/^(ANTHROPIC|OPENAI|HF|HUGGING)/i.test(name) && !/(KEY|TOKEN|SECRET)$/i.test(name)),
	);
}

async function startServers() {
	if (DEV_URL) return DEV_URL;
	if (!fs.existsSync(places.web)) throw new Error(`The app is not built yet (${places.web} is missing). Run: npm run build`);
	const python = await backendPython();
	life.move("serving"); // from the repository there was nothing to install: straight to serving
	if (!life.is("serving")) throw new Error("The app is closing.");
	const port = await freePort();
	const url = `http://127.0.0.1:${port}`;
	await Promise.all([
		serve(
			"The backend (python -m rp.server)",
			python,
			["-W", "ignore", "-m", "rp.server"],
			{
				cwd: places.backend,
				env: { ...process.env, RP_PORT: String(BACKEND_PORT), RP_DATA: places.data, RP_ORIGIN: url, RP_EXIT_WITH_PARENT: "1" },
			},
			`http://127.0.0.1:${BACKEND_PORT}/api/settings`,
		),
		serve(
			"The interface",
			process.execPath,
			[places.web],
			{
				cwd: path.dirname(places.web),
				env: { ...withoutKeys(process.env), ELECTRON_RUN_AS_NODE: "1", PORT: String(port), HOSTNAME: "127.0.0.1" },
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
const READER_SITES = [
	"openreview.net",
	"arxiv.org",
	"proceedings.neurips.cc",
	"proceedings.mlr.press",
	"openaccess.thecvf.com",
	"aclanthology.org",
];

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

/** Stop everything this shell started. Whatever state it was in, nothing new starts after this. */
function stopEverything() {
	life.move("quitting");
	installer?.kill();
	for (const child of children) child.kill();
}

/** Something went wrong that the user has to know about: say it once, then close. */
function fail(error) {
	if (!life.move("failed")) return; // already failing or quitting: one message is enough, and a quit needs none
	dialog.showErrorBox(app.getName(), error.message);
	app.quit();
}

if (first) {
	app.whenReady().then(async () => {
		try {
			const url = await startServers();
			if (!life.move("ready")) return; // closed while the servers were starting
			refusePermissions();
			openWindow(url);
		} catch (error) {
			fail(error);
		}
	});
}
app.on("window-all-closed", () => life.quitsWhenWindowsClose && app.quit());
app.on("before-quit", stopEverything);
