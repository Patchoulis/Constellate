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
const { app, BrowserWindow, dialog, nativeImage, session, shell } = require("electron");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { Lifecycle, withParent } = require("./lifecycle.cjs");
const { interfacePort } = require("./port.cjs");
const { holdAppWindow, quietNetwork, readerGuard, question } = require("./reader-guard.cjs");
const setup = require("./setup.cjs");

const BACKEND_PORT = 8765; // the web build forwards /api here (apps/web/next.config.ts)
// How often a starting server is asked whether it answers, and how many times: the window opens once both answer,
// so a server that is up waits half the interval on average before anyone notices. Asking is one local request.
// The limit is a number of tries, not a time: where a refused connection itself takes a second or two (Windows
// retries it), a slow first start gets as long as it always had.
const START_POLL_MS = 100;
const START_TRIES = 600;
const DEV_URL = process.env.RP_DEV_URL;
const REPO = path.join(__dirname, "..", "..");

/** Where the two servers and the data live: inside the installed app, or in the repository. */
const places = app.isPackaged
	? {
			backend: path.join(process.resourcesPath, "backend"),
			uv: path.join(process.resourcesPath, "uv", process.platform === "win32" ? "uv.exe" : "uv"),
			python: path.join(localData(), "python"),
			web: path.join(process.resourcesPath, "web", "apps", "web", "server.js"),
			data: path.join(localData(), "data"),
		}
	: {
			backend: path.join(REPO, "apps", "backend"),
			web: path.join(REPO, "apps", "web", ".next", "standalone", "apps", "web", "server.js"),
			data: path.join(REPO, "data"),
		};

/**
 * The installed app's own data: its library and its Python. Always the system's folder for one program's local
 * data, which nothing syncs: the library is a database that is open while the app runs, and a sync client
 * copying it mid-write can corrupt it. (Electron's "userData" is the roaming folder on Windows.)
 */
function localData() {
	if (process.platform === "win32" && process.env.LOCALAPPDATA) return path.join(process.env.LOCALAPPDATA, "Constellate");
	if (process.platform === "darwin") return path.join(app.getPath("home"), "Library", "Application Support", "Constellate");
	if (process.platform === "linux") {
		const shared = process.env.XDG_DATA_HOME;
		return path.join(shared && path.isAbsolute(shared) ? shared : path.join(app.getPath("home"), ".local", "share"), "constellate");
	}
	return app.getPath("userData");
}

/** The app's name, as everything it shows says it: never "Electron", whatever program runs it. */
const NAME = "Constellate";
/** The app's identity to Windows (electron-builder.yml appId): what the taskbar and the system's own prompts group
 *  and name its windows by. Without it, a copy run from the repository is shown as "Electron". */
const APP_ID = "app.constellate.desktop";
app.setName(NAME);
if (process.platform === "win32") app.setAppUserModelId(APP_ID);
// Nothing of Chromium's listens beyond this computer (reader-guard.cjs): set before the app is ready, for all of it.
quietNetwork(app);
// No page is ever handed a certificate of the user's: Electron's default would give the first one there is.
app.on("select-client-certificate", (event, _contents, _url, _certificates, callback) => {
	event.preventDefault();
	callback();
});

const ICON_FILE = path.join(__dirname, "icon.png");
/** The app's icon for a window or a dialog; nothing when it has not been drawn yet (npm run build draws it). */
function appIcon() {
	if (!fs.existsSync(ICON_FILE)) return undefined;
	const icon = nativeImage.createFromPath(ICON_FILE);
	return icon.isEmpty() ? undefined : icon;
}

/**
 * Ask or tell the user something in a dialog of the app's own: its title is the app's name and its picture the
 * app's icon, so that it is plain which program is asking. `kind` is the system's picture when there is no icon.
 */
function appDialog(kind, options) {
	const icon = appIcon();
	return { type: icon ? "none" : kind, icon, title: NAME, noLink: true, ...options };
}

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
		for (let attempt = 0; attempt < START_TRIES; attempt++) {
			if (await answers(url)) return;
			if (!life.maySpawn) throw new Error(`${name} is no longer waited for: the app is closing.`);
			await new Promise((resolve) => setTimeout(resolve, START_POLL_MS));
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
				title: `${NAME} Installation Wizard`,
				icon: appIcon(),
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
	// the same port as last time, so the window still has what it remembered (marked papers, panel sizes)
	const port = await interfacePort(path.join(places.data, "interface-port"));
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
			withParent(places.web),
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
		title: NAME,
		icon: appIcon(),
		// the reader shows a paper's own page (and its reviews) inside the window, in an embedded view; the
		// app's own page needs nothing of this computer either: no Node, a sandboxed process
		webPreferences: { webviewTag: true, sandbox: true, contextIsolation: true, nodeIntegration: false },
	});
	// An embedded view shows somebody else's web page: it gets no access to this app at all (reader-guard.cjs).
	reader.guardWindow(window);
	// The app's own window shows the app and nothing else, its frames included (reader-guard.cjs).
	holdAppWindow(window, url, (address) => void shell.openExternal(address));
	void window.loadURL(url);
}

/**
 * The embedded reader's guard (reader-guard.cjs). A site on no list is opened only when the user, asked in a
 * dialog of the app's own that no page can reach, chooses to; the safe answer is the default.
 */
const reader = readerGuard({
	openOutside: (address) => void shell.openExternal(address),
	ask: async (host) => {
		const asked = question(host);
		const [window] = BrowserWindow.getAllWindows();
		const options = appDialog("warning", {
			message: asked.message,
			detail: asked.detail,
			buttons: asked.buttons,
			defaultId: 0,
			cancelId: 0,
		});
		const { response } = window ? await dialog.showMessageBox(window, options) : await dialog.showMessageBox(options);
		return asked.answers[response] ?? "no";
	},
});

/**
 * Nothing in this app needs the camera, the microphone, the location...: every permission request is refused,
 * in the app's own window and in the reader, whose pages are also kept to the paper sites (reader-guard.cjs).
 */
function refusePermissions() {
	session.defaultSession.setPermissionRequestHandler((_contents, _permission, answer) => answer(false));
	session.defaultSession.setPermissionCheckHandler(() => false);
	reader.confineSession(session);
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
	dialog.showMessageBoxSync(appDialog("error", { message: `${NAME} could not start.`, detail: error.message, buttons: ["Close"] }));
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
// Ctrl+C in the terminal that ran `npm run app`, or the system asking the app to end: quit the ordinary way, so
// that before-quit stops the servers (by default Node would end this process alone).
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => app.quit());
