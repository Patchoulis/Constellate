/**
 * The desktop shell: one window around the app's two local servers (engine.cjs starts both).
 *
 *   engine     constellate (apps/engine)     everything the app does but drawing it: its HTTP API on 127.0.0.1,
 *                                            on a port of its own choosing, answered only with its token
 *   interface  Next's standalone server.js   the pages, on the port remembered in the data folder (port.cjs); it
 *                                            forwards /api and /auth to the engine, adding the token itself
 *
 * Both are child processes of this shell and stop with it. The interface's server is run by Electron's own Node
 * (process.execPath + ELECTRON_RUN_AS_NODE=1), so no Node installation is needed. Nothing is installed at the first
 * start: the app opens at once, and the first run the user makes downloads the embedding model (about 1.2 GB)
 * as one of its steps, shown with the run's progress.
 *
 * Two ways to run:
 *   - built (the default): the engine and the interface come from target/release and apps/web/.next/standalone
 *     when started from the repository (`npm run build` makes them), or from the app's resources when packaged.
 *     CONSTELLATE_DATA, when set to a folder, is the data folder instead of the usual one.
 *   - RP_DEV_URL=http://localhost:3210: nothing is spawned; the window shows the servers `npm run dev` started.
 *
 * Where the app is between opening and closing is lifecycle.cjs: every decision below that depends on it (may a
 * process start, does a window closing mean quit, may the app end yet) asks it rather than a flag.
 */
const { app, BrowserWindow, dialog, nativeImage, session, shell } = require("electron");
const { spawn } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { EngineLog, WINDOW_COOKIE, answers, interfaceEnvironment, startEngine } = require("./engine.cjs");
const { Lifecycle, withParent } = require("./lifecycle.cjs");
const { interfacePort } = require("./port.cjs");
const { holdAppWindow, quietNetwork, readerGuard, question } = require("./reader-guard.cjs");

// How often the starting interface is asked whether it answers, and how many times: the window opens once it
// does. The limit is a number of tries, not a time: where a refused connection itself takes a second or two
// (Windows retries it), a slow first start gets as long as it always had.
const START_POLL_MS = 100;
const START_TRIES = 600;
/** How long a child has to stop once asked (its standard input closed) before it is ended. */
const STOP_GRACE_MS = 4000;
const DEV_URL = process.env.RP_DEV_URL;
const REPO = path.join(__dirname, "..", "..");
const ENGINE_FILE = process.platform === "win32" ? "constellate.exe" : "constellate";
/** This launch's key of the window, for the interface's server (engine.cjs `interfaceEnvironment`). */
const WINDOW_KEY = crypto.randomBytes(32).toString("hex");
/** The oldest C library the engine's ONNX Runtime loads with on Linux (Ubuntu 24.04 has it). */
const LEAST_GLIBC = [2, 38];

/** Where the two servers and the data live: inside the installed app, or in the repository. */
const places = app.isPackaged
	? {
			engine: path.join(process.resourcesPath, "engine", ENGINE_FILE),
			web: path.join(process.resourcesPath, "web", "apps", "web", "server.js"),
			data: dataFolder(path.join(localData(), "data")),
		}
	: {
			engine: path.join(REPO, "target", "release", ENGINE_FILE),
			web: path.join(REPO, "apps", "web", ".next", "standalone", "apps", "web", "server.js"),
			data: dataFolder(path.join(REPO, "data")),
		};

/** The data folder: CONSTELLATE_DATA when it names one (a whole path), else `usual`. */
function dataFolder(usual) {
	const asked = process.env.CONSTELLATE_DATA?.trim();
	return asked && path.isAbsolute(asked) ? asked : usual;
}

/**
 * The installed app's own data. Always the system's folder for one program's local data, which nothing syncs:
 * the library is a database that is open while the app runs, and a sync client copying it mid-write can corrupt
 * it. (Electron's "userData" is the roaming folder on Windows.) The library and runs are in its `data` folder, where
 * they have always been, so an update finds them.
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

/** The window's top bar is the page's own: its height in pixels (apps/web/lib/shell/title-bar.ts BAR_HEIGHT) and
 *  the colours of the page's bar, for the system's window buttons laid over it. The buttons are one pixel
 *  shorter than the bar (BAR_LINE), so that the line under the bar runs under them too, to the window's edge. */
const BAR_HEIGHT = 36;
const BAR_LINE = 1;
const BAR_COLOURS = { color: "#1c1c1c", symbolColor: "#e5e5e5" };

/** How a window of this system hides its own title bar and what it keeps of it. */
function windowChrome(platform) {
	if (platform === "darwin") return { titleBarStyle: "hiddenInset", trafficLightPosition: { x: 14, y: 11 } };
	return { titleBarStyle: "hidden", titleBarOverlay: { ...BAR_COLOURS, height: BAR_HEIGHT - BAR_LINE } };
}

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
const children = []; // the engine and the interface's server

// One Constellate at a time: a second one would open the same library beside the first one's engine. The second
// one does nothing at all but quit.
const first = app.requestSingleInstanceLock();
if (!first) app.quit();
app.on("second-instance", () => {
	const [window] = BrowserWindow.getAllWindows();
	if (!window) return;
	if (window.isMinimized()) window.restore();
	window.focus();
});

/** A child the shell started: kept to be stopped, and reported if it ends once the app is ready (while starting
 *  up, whoever waits for it reports it). */
function adopt(name, child, tail) {
	children.push(child);
	child.once("exit", (code) => {
		if (life.childExitIsFailure) fail(new Error(`${name} stopped (code ${code}).\n\n${tail()}`));
	});
}

/**
 * On Linux, the engine's ONNX Runtime needs glibc 2.38 or newer: said plainly, rather than as the system loader's
 * complaint about a missing symbol. Null when it can run here.
 */
function tooOldForTheEngine() {
	if (process.platform !== "linux") return null;
	const version = process.report?.getReport?.().header?.glibcVersionRuntime;
	if (typeof version !== "string") return null; // not glibc (or not told): let the engine try
	const [major, minor] = version.split(".").map(Number);
	if (major > LEAST_GLIBC[0] || (major === LEAST_GLIBC[0] && minor >= LEAST_GLIBC[1])) return null;
	return `${NAME} needs a Linux with glibc ${LEAST_GLIBC.join(".")} or newer (Ubuntu 24.04, Debian 13, Fedora 39 or later). This system has glibc ${version}.`;
}

/** Start the interface's server and resolve once it answers at `url`; reject if it ends first or never answers. */
async function startInterface(port, url, engine) {
	if (!life.maySpawn) throw new Error("The interface was not started: the app is closing.");
	const child = spawn(process.execPath, withParent(places.web), {
		cwd: path.dirname(places.web),
		env: interfaceEnvironment(process.env, { port, engine: engine.url, token: engine.token, windowKey: WINDOW_KEY }),
		stdio: ["pipe", "pipe", "pipe"],
		windowsHide: true,
	});
	let output = "";
	const keep = (chunk) => (output = (output + chunk).slice(-4000));
	child.stdout.on("data", keep);
	child.stderr.on("data", keep);
	adopt("The interface", child, () => output);
	const exited = new Promise((_, reject) => {
		child.once("error", (error) => reject(new Error(`The interface could not start: ${error.message}`)));
		child.once("exit", (code) => reject(new Error(`The interface stopped (code ${code}).\n\n${output}`)));
	});
	const up = (async () => {
		for (let attempt = 0; attempt < START_TRIES; attempt++) {
			if (await answers(url)) return;
			if (!life.maySpawn) throw new Error("The interface is no longer waited for: the app is closing.");
			await new Promise((resolve) => setTimeout(resolve, START_POLL_MS));
		}
		throw new Error(`The interface did not answer at ${url}.\n\n${output}`);
	})();
	await Promise.race([up, exited]);
}

async function startServers() {
	if (DEV_URL) return DEV_URL;
	if (!life.move("serving")) throw new Error("The app is closing.");
	const old = tooOldForTheEngine();
	if (old) throw new Error(old);
	for (const [what, file] of [
		["The interface", places.web],
		["The engine", places.engine],
	]) {
		if (!fs.existsSync(file)) throw new Error(`${what} is not built yet (${file} is missing). Run: npm run build`);
	}
	fs.mkdirSync(places.data, { recursive: true });
	// the same port as last time, so the window still has what it remembered (marked papers, panel sizes)
	const port = await interfacePort(path.join(places.data, "interface-port"));
	const url = `http://127.0.0.1:${port}`;
	const log = new EngineLog(path.join(places.data, "logs"));
	const engine = await startEngine({
		binary: places.engine,
		data: places.data,
		origin: url,
		log,
		admit: (child) => life.maySpawn && (adopt("The engine", child, () => log.recent), true),
	});
	await startInterface(port, url, engine);
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
		// No title bar of the system's: the page draws the window's top bar itself (apps/web components/shell/
		// title-bar.tsx) and is what the window is dragged by. Windows and Linux keep their own minimise, maximise
		// and close buttons, laid over the bar's right end in its colours; a Mac keeps its three lights, set in.
		...windowChrome(process.platform),
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

/** Has this child ended? */
const ended = (child) => child.exitCode !== null || child.signalCode !== null;

/**
 * Stop every child: each is asked first, by closing its standard input (the engine then finishes what it is
 * answering and saves nothing half-written), and ended if it has not stopped within STOP_GRACE_MS.
 */
async function stopChildren() {
	await Promise.all(
		children.map(async (child) => {
			if (ended(child)) return;
			const gone = new Promise((resolve) => child.once("exit", resolve));
			child.stdin?.end();
			const late = setTimeout(() => child.kill(), STOP_GRACE_MS);
			await gone;
			clearTimeout(late);
		}),
	);
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
			// the window's key (engine.cjs interfaceEnvironment): a cookie no page can read and no other site sends;
			// set while still starting, so that a failure is reported and a quit meanwhile opens no window
			if (!DEV_URL) {
				await session.defaultSession.cookies.set({ url, name: WINDOW_COOKIE, value: WINDOW_KEY, httpOnly: true, sameSite: "strict" });
			}
			if (!life.move("ready")) return; // closed while the servers were starting
			refusePermissions();
			openWindow(url);
		} catch (error) {
			fail(error);
		}
	});
}
app.on("window-all-closed", () => life.quitsWhenWindowsClose && app.quit());
// Every way the app quits passes here: the children are stopped first, and only then does the app end.
app.on("before-quit", (event) => {
	if (life.mayEnd) return;
	event.preventDefault();
	if (!life.move("quitting")) return; // already stopping them: this quit happens when they have stopped
	void stopChildren().then(() => {
		life.move("stopped");
		app.quit();
	});
});
// Ctrl+C in the terminal that ran `npm run app`, or the system asking the app to end: quit the ordinary way, so
// that before-quit stops the servers (by default Node would end this process alone).
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => app.quit());
