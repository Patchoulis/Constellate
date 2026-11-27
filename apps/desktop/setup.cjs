/**
 * The installed app's own Python: set up the first time the app starts, so that installing Constellate needs
 * nothing else on the machine.
 *
 * The installer carries `uv` (https://docs.astral.sh/uv/), one program that can fetch a Python and install
 * packages into it. On first start (and again whenever a new version asks for other packages) this:
 *
 *   1. makes a Python environment in the app's own data folder        uv venv --python 3.13
 *   2. installs PyTorch: the CUDA build on a machine with an NVIDIA    uv pip install torch [--index-url ...]
 *      graphics card, otherwise the ordinary one
 *   3. installs the backend's other packages                           uv pip install -r requirements.txt
 *
 * and leaves a note of what it installed, so the next start goes straight on. It is a small state machine:
 * needed -> installing -> ready, or installing -> failed (the error names the step and keeps uv's last words).
 * Nothing here needs Electron, so it can be run and tested with plain Node.
 */
const { spawn, spawnSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const PYTHON = "3.13";
/** Where PyTorch publishes its build for NVIDIA cards. */
const CUDA_INDEX = "https://download.pytorch.org/whl/cu130";

/** Does this machine have an NVIDIA card with a working driver? (`nvidia-smi` comes with the driver.) */
function hasNvidia() {
	try {
		return spawnSync("nvidia-smi", ["-L"], { windowsHide: true, timeout: 10_000 }).status === 0;
	} catch (error) {
		return false;
	}
}

/** The environment's Python, whether or not it exists yet. */
function pythonIn(folder) {
	return process.platform === "win32" ? path.join(folder, "Scripts", "python.exe") : path.join(folder, "bin", "python");
}

/** What a finished setup is recognised by: these exact requirements, this Python, this PyTorch build. */
function stampOf(requirements, torchIndex) {
	const wanted = `${PYTHON}\n${torchIndex ?? "default"}\n${fs.readFileSync(requirements, "utf8")}`;
	return crypto.createHash("sha256").update(wanted).digest("hex").slice(0, 16);
}

function state({ folder, requirements, torchIndex }) {
	const note = path.join(folder, "installed.txt");
	const done = fs.existsSync(note) && fs.readFileSync(note, "utf8").trim() === stampOf(requirements, torchIndex);
	return done && fs.existsSync(pythonIn(folder)) ? "ready" : "needed";
}

/** Run one step, passing on what it prints line by line; reject with the step's name and its last lines. */
function step(title, command, args, log) {
	return new Promise((resolve, reject) => {
		log(title);
		const child = spawn(command, args, { windowsHide: true, env: { ...process.env, UV_NO_PROGRESS: "1" } });
		let last = "";
		const pass = (chunk) => {
			for (const line of String(chunk).split(/\r?\n/)) {
				if (!line.trim()) continue;
				last = (last + "\n" + line).slice(-1500);
				log(`  ${line.trim()}`);
			}
		};
		child.stdout.on("data", pass);
		child.stderr.on("data", pass);
		child.once("error", (error) => reject(new Error(`${title} could not start: ${error.message}`)));
		child.once("exit", (code) => (code === 0 ? resolve() : reject(new Error(`${title} failed (code ${code}).\n${last.trim()}`))));
	});
}

/**
 * Make sure the environment in `folder` has what `requirements` lists; resolves to its Python.
 *   uv            path of the uv program
 *   torch         install PyTorch as its own step first (false in tests, which install something small)
 *   log           called with each line of progress
 */
async function ensure({ uv, folder, requirements, torch = true, log = () => {} }) {
	const torchIndex = torch && hasNvidia() ? CUDA_INDEX : null;
	const python = pythonIn(folder);
	if (state({ folder, requirements, torchIndex }) === "ready") return python;
	if (!fs.existsSync(uv)) throw new Error(`The installer's package tool is missing (${uv}). Reinstall Constellate.`);

	fs.mkdirSync(path.dirname(folder), { recursive: true });
	await step(`Setting up Python ${PYTHON}`, uv, ["venv", "--python", PYTHON, "--allow-existing", folder], log);
	if (torch) {
		const from = torchIndex ? ["--index-url", torchIndex] : [];
		const which = torchIndex
			? "PyTorch for your NVIDIA graphics card (about 3 GB)"
			: "PyTorch (no NVIDIA graphics card found: embedding will be slow)";
		await step(`Installing ${which}`, uv, ["pip", "install", "--python", python, "torch", ...from], log);
	}
	await step("Installing the other packages", uv, ["pip", "install", "--python", python, "-r", requirements], log);
	fs.writeFileSync(path.join(folder, "installed.txt"), stampOf(requirements, torchIndex));
	log("Ready.");
	return python;
}

module.exports = { ensure, state, pythonIn, PYTHON };
