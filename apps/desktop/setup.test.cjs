/**
 * The first-start setup, for real but small: a Python environment with one tiny package, made by uv.
 * Needs uv (RP_UV, or `uv` on the path) and the network; skipped when uv is not there, except in CI, where a
 * missing uv fails the run instead of passing it with the setup untested.    node --test
 */
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");
const { ensure, lockedVersion, physical, state, torchFor, CUDA } = require("./setup.cjs");

const uv = process.env.RP_UV || "uv";
const available = spawnSync(uv, ["--version"], { windowsHide: true }).status === 0;
if (!available && process.env.CI) throw new Error(`uv is needed to test the first-start setup and was not found (${uv}): set RP_UV`);
const uvPath =
	available && !path.isAbsolute(uv)
		? spawnSync(process.platform === "win32" ? "where" : "which", [uv])
				.stdout.toString()
				.split(/\r?\n/)[0]
		: uv;

test(
	"a first start sets Python up, a second start finds it ready, and new requirements set it up again",
	{ skip: !available, timeout: 600_000 },
	async () => {
		const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
		const requirements = path.join(home, "requirements.txt");
		fs.writeFileSync(requirements, "six\n");
		const options = { uv: uvPath, folder: path.join(home, "python"), requirements, torch: false };
		assert.equal(state({ ...options, torchIndex: null }), "needed");

		const lines = [];
		const python = await ensure({ ...options, log: (line) => lines.push(line) });
		assert.ok(fs.existsSync(python));
		assert.equal(spawnSync(python, ["-c", "import six"]).status, 0);
		assert.equal(lines.at(-1), "Ready.");
		assert.equal(state({ ...options, torchIndex: null }), "ready");

		const again = [];
		assert.equal(await ensure({ ...options, log: (line) => again.push(line) }), python);
		assert.deepEqual(again, []); // nothing was done

		fs.writeFileSync(requirements, "six\nidna\n");
		assert.equal(state({ ...options, torchIndex: null }), "needed");
		await ensure(options);
		assert.equal(spawnSync(python, ["-c", "import idna"]).status, 0);
		fs.rmSync(home, { recursive: true, force: true });
	},
);

test(
	"the lock decides the version installed, and a new lock sets Python up again with the version it names",
	{ skip: !available, timeout: 600_000 },
	async () => {
		const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
		const requirements = path.join(home, "requirements.txt");
		const constraints = path.join(home, "requirements.lock.txt");
		fs.writeFileSync(requirements, "six\n");
		fs.writeFileSync(constraints, "six==1.16.0\n");
		const options = { uv: uvPath, folder: path.join(home, "python"), requirements, constraints, torch: false };
		const python = await ensure(options);
		const version = () => spawnSync(python, ["-c", "import six; print(six.__version__)"]).stdout.toString().trim();
		assert.equal(version(), "1.16.0");
		assert.equal(state({ ...options, torchIndex: null }), "ready");

		// an existing setup is not left on the old version: the lock is part of what a finished setup is recognised by
		fs.writeFileSync(constraints, "six==1.17.0\n");
		assert.equal(state({ ...options, torchIndex: null }), "needed");
		await ensure(options);
		assert.equal(version(), "1.17.0");
		fs.rmSync(home, { recursive: true, force: true });
	},
);

test("a lock gives PyTorch's step one version, or says what is wrong with it", () => {
	const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
	const lock = path.join(home, "requirements.lock.txt");
	fs.writeFileSync(lock, "# made by uv\nsympy==1.14.0\ntorch==2.14.1\ntorchvision==0.29.1\n");
	assert.equal(lockedVersion(lock, "torch"), "2.14.1");
	fs.writeFileSync(lock, "torch==2.14.1 ; sys_platform == 'win32'\ntorch==2.14.0 ; sys_platform == 'linux'\n");
	assert.throws(() => lockedVersion(lock, "torch"), /exactly one version/);
	fs.writeFileSync(lock, "sympy==1.14.0\n");
	assert.throws(() => lockedVersion(lock, "torch"), /exactly one version/);
	fs.rmSync(home, { recursive: true, force: true });
});

test("the backend's lock pins every package the backend asks for, and one PyTorch for every system", () => {
	const backend = path.join(__dirname, "..", "backend");
	const lock = fs.readFileSync(path.join(backend, "requirements.lock.txt"), "utf8");
	const normal = (name) => name.toLowerCase().replace(/[-_.]+/g, "-");
	const pinned = new Set([...lock.matchAll(/^([A-Za-z0-9._-]+)==/gm)].map((match) => normal(match[1])));
	const asked = fs
		.readFileSync(path.join(backend, "requirements.txt"), "utf8")
		.split(/\r?\n/)
		.map((line) => line.replace(/#.*/, "").trim())
		.filter(Boolean)
		.map((line) => normal(/^[A-Za-z0-9._-]+/.exec(line)[0]));
	assert.ok(asked.length > 5);
	assert.deepEqual(
		asked.filter((name) => !pinned.has(name)),
		[],
		"run: uv pip compile requirements.txt --universal --python-version 3.13 --no-annotate -o requirements.lock.txt",
	);
	assert.match(lockedVersion(path.join(backend, "requirements.lock.txt"), "torch"), /^\d+\.\d+\.\d+$/);
});

test("a step that fails says which step and why", { skip: !available, timeout: 600_000 }, async () => {
	const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
	const requirements = path.join(home, "requirements.txt");
	fs.writeFileSync(requirements, "this-package-does-not-exist-anywhere-1234567\n");
	const options = { uv: uvPath, folder: path.join(home, "python"), requirements, torch: false };
	await assert.rejects(ensure(options), /Installing the other packages failed/);
	assert.equal(state({ ...options, torchIndex: null }), "needed"); // a half-done setup is not taken for a finished one
	fs.rmSync(home, { recursive: true, force: true });
});

test("a missing package tool is reported, not crashed on", async () => {
	const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
	fs.writeFileSync(path.join(home, "requirements.txt"), "six\n");
	await assert.rejects(
		ensure({
			uv: path.join(home, "no-uv.exe"),
			folder: path.join(home, "python"),
			requirements: path.join(home, "requirements.txt"),
			torch: false,
		}),
		/package tool is missing/,
	);
	fs.rmSync(home, { recursive: true, force: true });
});

/** A folder `real` and a link to it (a junction on Windows, which needs no administrator), in a fresh folder. */
function linked() {
	const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
	const real = path.join(home, "real");
	fs.mkdirSync(real);
	const link = path.join(home, "link");
	fs.symlinkSync(real, link, "junction");
	return { home, real: fs.realpathSync.native(real), link };
}

test("a folder is known by where it really is, made first when it is not there yet", () => {
	const { home, real, link } = linked();
	assert.equal(physical(path.join(link, "app")), path.join(real, "app"));
	assert.ok(fs.statSync(path.join(real, "app")).isDirectory());
	assert.equal(physical(path.join(real, "app")), path.join(real, "app")); // a real folder is itself
	fs.rmSync(home, { recursive: true, force: true });
});

test(
	"a data folder reached under another address is set up, and its Python found, where it really is",
	{ skip: !available, timeout: 600_000 },
	async () => {
		// Stands in for a folder Windows shows a packaged app's programs in one place and keeps in another: uv's own
		// link to the Python it fetches must hold the real address, or it leads nowhere for uv itself.
		const { home, real, link } = linked();
		const requirements = path.join(home, "requirements.txt");
		fs.writeFileSync(requirements, "six\n");
		const options = { uv: uvPath, folder: path.join(link, "python"), requirements, torch: false };
		const python = await ensure(options);
		assert.ok(python.startsWith(path.join(real, "python")), python);
		assert.equal(spawnSync(python, ["-c", "import six"]).status, 0);
		assert.equal(await ensure(options), python); // and the next start finds it ready under either address
		fs.rmSync(home, { recursive: true, force: true });
	},
);

test("which PyTorch a machine gets follows its NVIDIA driver", async () => {
	assert.equal((await torchFor(async () => null)).index, null); // no card, or no driver
	assert.equal((await torchFor(async () => "591.44\r\n")).index, CUDA.index);
	assert.equal((await torchFor(async () => "591.44\n591.44\n")).index, CUDA.index); // two cards
	const old = await torchFor(async () => "552.12\n");
	assert.equal(old.index, null);
	assert.match(old.why, /older than 580/);
	assert.equal((await torchFor(async () => "nonsense")).index, null);
});

test(
	"two setups in one folder at the same time: the second is told to wait, and a dead claim is taken over",
	{ skip: !available, timeout: 600_000 },
	async () => {
		const home = fs.mkdtempSync(path.join(os.tmpdir(), "rp-setup-"));
		const requirements = path.join(home, "requirements.txt");
		fs.writeFileSync(requirements, "six\n");
		const options = { uv: uvPath, folder: path.join(home, "python"), requirements, torch: false };
		// a claim held by a process that is alive (this one's parent stands in for "another window")
		fs.writeFileSync(path.join(home, "installing.lock"), String(process.ppid));
		await assert.rejects(ensure(options), /already being installed/);
		// a claim left by a process that is gone
		fs.writeFileSync(path.join(home, "installing.lock"), "999999999");
		await ensure(options);
		assert.equal(fs.existsSync(path.join(home, "installing.lock")), false); // given back when done
		fs.rmSync(home, { recursive: true, force: true });
	},
);
