/**
 * The first-start setup, for real but small: a Python environment with one tiny package, made by uv.
 * Needs uv (RP_UV, or `uv` on the path) and the network; skipped when uv is not there.    node --test
 */
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");
const { ensure, state, torchFor, CUDA } = require("./setup.cjs");

const uv = process.env.RP_UV || "uv";
const available = spawnSync(uv, ["--version"], { windowsHide: true }).status === 0;
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

test("which PyTorch a machine gets follows its NVIDIA driver", async () => {
	assert.equal((await torchFor(async () => null)).index, null); // no card, or no driver
	assert.equal((await torchFor(async () => "591.44\r\n")).index, CUDA.index);
	assert.equal((await torchFor(async () => "591.44\n591.44\n")).index, CUDA.index); // two cards
	const old = await torchFor(async () => "552.12\n");
	assert.equal(old.index, null);
	assert.match(old.why, /older than 580/);
	assert.equal((await torchFor(async () => "nonsense")).index, null);
});
