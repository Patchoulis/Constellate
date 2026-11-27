/**
 * The installed app's own Python: set up the first time the app starts, so that installing Constellate needs
 * nothing else on the machine.
 *
 * The installer carries `uv` (https://docs.astral.sh/uv/), one program that can fetch a Python and install
 * packages into it. On first start (and again whenever a new version asks for other packages, or the machine
 * gains or loses a usable NVIDIA graphics card) this:
 *
 *   1. makes a Python environment in the app's own data folder        uv venv --python 3.13
 *   2. installs PyTorch: the CUDA build on a machine with an NVIDIA    uv pip install torch [--index-url ...]
 *      graphics card and a recent enough driver, otherwise the ordinary one
 *   3. installs the backend's other packages                           uv pip install -r requirements.txt
 *
 * and leaves a note of what it installed, so the next start goes straight on. Everything uv fetches (the Python
 * itself, its download cache) is kept under the app's data folder too, and the machine's own Pythons and uv
 * settings are ignored: the app must keep working when those change, and uninstalling must be able to remove
 * all of it.
 *
 * It is a small state machine: needed -> installing -> ready, or installing -> failed (the error names the step
 * and keeps uv's last words). Nothing here needs Electron, so it can be run and tested with plain Node.
 */
const { spawn } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const PYTHON = "3.13";
/** Where PyTorch publishes its build for NVIDIA cards, and the oldest driver that build runs on. */
const CUDA = { index: "https://download.pytorch.org/whl/cu130", driver: 580 };

/** Run a program and resolve to what it printed, or null if it is missing, fails or takes too long. */
function ask(command, args, milliseconds) {
	return new Promise((resolve) => {
		let said = "";
		let child;
		try {
			child = spawn(command, args, { windowsHide: true });
		} catch (error) {
			return resolve(null);
		}
		const timer = setTimeout(() => child.kill(), milliseconds);
		child.stdout.on("data", (chunk) => (said += chunk));
		child.once("error", () => resolve(null));
		child.once("close", (code) => {
			clearTimeout(timer);
			resolve(code === 0 ? said : null);
		});
	});
}

/**
 * Which PyTorch this machine should get: {index, why}. `index` is null for the ordinary build.
 * `nvidia-smi` comes with NVIDIA's driver and reports its version.
 */
async function torchFor(report = () => ask("nvidia-smi", ["--query-gpu=driver_version", "--format=csv,noheader"], 8000)) {
	const said = await report();
	const driver = said === null ? null : Number.parseInt(said.trim().split(/\r?\n/)[0], 10);
	if (driver === null || Number.isNaN(driver)) return { index: null, why: "no NVIDIA graphics card found: embedding will be slow" };
	if (driver < CUDA.driver)
		return {
			index: null,
			why: `the NVIDIA driver (${driver}) is older than ${CUDA.driver}: update it for GPU speed; until then embedding will be slow`,
		};
	return { index: CUDA.index, why: "for your NVIDIA graphics card (about 3 GB)" };
}

/**
 * Claim the right to set Python up: two setups in one folder at the same time break each other's downloads.
 * The claim is a file holding the claimant's process id; a claim whose process is gone (a crash, a power cut)
 * is taken over. Returns the function that gives the claim back.
 */
function claim(file) {
	const alive = (pid) => {
		try {
			process.kill(pid, 0);
			return true;
		} catch (error) {
			return error.code === "EPERM";
		}
	};
	for (let attempt = 0; attempt < 2; attempt++) {
		try {
			fs.writeFileSync(file, String(process.pid), { flag: "wx" });
			return () => fs.rmSync(file, { force: true });
		} catch (error) {
			if (error.code !== "EEXIST") throw error;
			const holder = Number.parseInt(fs.readFileSync(file, "utf8"), 10);
			if (holder !== process.pid && alive(holder)) {
				throw new Error("Constellate is already being installed by another window. Wait for it to finish, then open Constellate again.");
			}
			fs.rmSync(file, { force: true }); // left behind by a process that is gone: take it over
		}
	}
	throw new Error(`Could not claim ${file}.`);
}

/** The environment's Python, whether or not it exists yet. */
function pythonIn(folder) {
	return process.platform === "win32" ? path.join(folder, "Scripts", "python.exe") : path.join(folder, "bin", "python");
}

/** What a finished setup is recognised by: these exact requirements and this Python. */
function stampOf(requirements) {
	return crypto
		.createHash("sha256")
		.update(`${PYTHON}\n${fs.readFileSync(requirements, "utf8")}`)
		.digest("hex")
		.slice(0, 16);
}

/** The note a finished setup leaves: {stamp, torch: the index its PyTorch came from, or "default"}; null if none. */
function noteIn(folder) {
	try {
		return JSON.parse(fs.readFileSync(path.join(folder, "installed.json"), "utf8"));
	} catch (error) {
		return null;
	}
}

/** Is the environment's Python still there? (It points at a Python outside itself, named in pyvenv.cfg.) */
function intact(folder) {
	try {
		const home = /^home\s*=\s*(.+)$/m.exec(fs.readFileSync(path.join(folder, "pyvenv.cfg"), "utf8"));
		return fs.existsSync(pythonIn(folder)) && Boolean(home) && fs.existsSync(home[1].trim());
	} catch (error) {
		return false;
	}
}

/** "ready" or "needed", for the PyTorch build `torchIndex` (null = the ordinary one). */
function state({ folder, requirements, torchIndex }) {
	const note = noteIn(folder);
	const done = note && note.stamp === stampOf(requirements) && note.torch === (torchIndex ?? "default");
	return done && intact(folder) ? "ready" : "needed";
}

/**
 * Run one step, passing on what it prints line by line; reject with the step's name and its last lines.
 * `started` is told the running process, so that whoever asked can stop it.
 */
function step(title, command, args, { log, env, started }) {
	return new Promise((resolve, reject) => {
		log(title);
		const child = spawn(command, args, { windowsHide: true, env });
		started(child);
		let last = "";
		const pass = (chunk) => {
			for (const line of String(chunk).split(/\r?\n/)) {
				if (!line.trim()) continue;
				last = `${last}\n${line}`.slice(-1500);
				log(`  ${line.trim()}`);
			}
		};
		child.stdout.on("data", pass);
		child.stderr.on("data", pass);
		child.once("error", (error) => reject(new Error(`${title} could not start: ${error.message}`)));
		// "close", not "exit": only then has everything the step printed arrived, its error message included
		child.once("close", (code, signal) =>
			code === 0 ? resolve() : reject(new Error(`${title} ${signal ? "was stopped" : `failed (code ${code})`}.\n${last.trim()}`)),
		);
	});
}

/**
 * Where `folder` really is on disk, which it is made first if need be.
 *
 * Windows can show a program a folder somewhere it is not: a program started from inside a packaged (MSIX) app,
 * such as a terminal in one, sees its new folders under %LOCALAPPDATA% where it made them, while Windows keeps them
 * in that app's own folder (%LOCALAPPDATA%\Packages\<app>\LocalCache). uv links each Python it fetches to a second
 * name (cpython-3.13-... -> cpython-3.13.N-...), and a link holds the address it was given: the one that exists
 * only for that program. The link then leads nowhere and uv stops with "Missing expected target directory for
 * Python minor version link". Given the real address, the link leads to a real folder for everyone. Elsewhere this
 * is the folder itself (or the folder its symbolic links lead to).
 */
function physical(folder) {
	fs.mkdirSync(folder, { recursive: true });
	return fs.realpathSync.native(folder);
}

/**
 * Make sure the environment in `folder` has what `requirements` lists; resolves to its Python.
 *   uv            path of the uv program
 *   torch         install PyTorch as its own step first (false in tests, which install something small)
 *   log           called with each line of progress; must not throw
 *   started       called with each process this starts, so the caller can stop the one that is running
 */
async function ensure({ uv, folder, requirements, torch = true, log = () => {}, started = () => {} }) {
	const wanted = torch ? await torchFor() : { index: null, why: "" };
	// every address handed to uv, and the Python handed back, is where the folder really is (see `physical`)
	const home = physical(path.dirname(folder));
	folder = path.join(home, path.basename(folder));
	const python = pythonIn(folder);
	if (state({ folder, requirements, torchIndex: wanted.index }) === "ready") return python;
	if (!fs.existsSync(uv)) throw new Error(`The installer's package tool is missing (${uv}). Reinstall Constellate.`);

	const release = claim(path.join(home, "installing.lock"));
	// uv keeps to this app: its own Python, its own cache, and no settings from the rest of the machine
	const env = {
		...process.env,
		UV_NO_PROGRESS: "1",
		UV_NO_CONFIG: "1",
		UV_PYTHON_PREFERENCE: "only-managed",
		UV_PYTHON_INSTALL_DIR: path.join(home, "uv", "python"),
		UV_CACHE_DIR: path.join(home, "uv", "cache"),
	};
	const run = (title, args) => step(title, uv, args, { log, env, started });
	const before = noteIn(folder);
	try {
		await run(`Setting up Python ${PYTHON}`, ["venv", "--python", PYTHON, "--allow-existing", folder]);
		if (torch) {
			// a PyTorch of the other build already satisfies "torch", so it has to be replaced by name
			const replace = before && before.torch !== (wanted.index ?? "default") ? ["--reinstall-package", "torch"] : [];
			const from = wanted.index ? ["--index-url", wanted.index] : [];
			await run(`Installing PyTorch (${wanted.why})`, ["pip", "install", "--python", python, "torch", ...from, ...replace]);
		}
		await run("Installing the other packages", ["pip", "install", "--python", python, "-r", requirements]);
	} catch (error) {
		release();
		error.message +=
			"\n\nThe installation needs a connection to the internet. Constellate will carry on with it the next time it is opened.";
		throw error;
	}
	fs.writeFileSync(path.join(folder, "installed.json"), JSON.stringify({ stamp: stampOf(requirements), torch: wanted.index ?? "default" }));
	release();
	log("Ready.");
	return python;
}

module.exports = { ensure, physical, state, torchFor, pythonIn, PYTHON, CUDA };
