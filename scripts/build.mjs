/**
 * Build the app, and optionally run or package it.
 *
 *   npm run build     build the interface and the engine (both cached: an unchanged tree builds in seconds)
 *   npm run app       build, then open the desktop app from this repository
 *   npm run dist      build, then make this system's packages in dist/: the Windows installer on Windows, the
 *                     AppImage and the .deb on Linux
 *
 * The steps, in order; each is skipped unless its flag asks for it:
 *   1. turbo run build     production build of apps/web (Next standalone output)
 *   2. assemble            copy the files Next leaves out of the standalone folder (static assets, public/),
 *                          and draw the window icon from apps/web/app/icon.svg
 *   3. cargo build         the engine, `constellate`, for this system (not with --interface)
 *   4. --run               start Electron on the result
 *   5. --dist              stage the engine and what it loads at run time in apps/desktop/engine (engine-files.cjs),
 *                          check that it starts from there, then electron-builder: shell + interface + engine -> dist/
 *
 *   --interface            steps 1 and 2 only (what the shell's tests need, in `npm run pr`)
 */
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { readFileSync, writeFileSync, cpSync, existsSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const web = join(repo, "apps", "web");
const desktop = join(repo, "apps", "desktop");
const standalone = join(web, ".next", "standalone", "apps", "web");
const flags = new Set(process.argv.slice(2));
// cargo is in the user's folder when rustup put it there and the PATH was not changed
const env = { ...process.env, PATH: `${join(homedir(), ".cargo", "bin")}${process.platform === "win32" ? ";" : ":"}${process.env.PATH}` };

function run(title, command, cwd = repo, extra = {}) {
	console.log(`\n> ${title}`);
	const started = Date.now();
	// through the shell: npx is a .cmd file on Windows; every command here is written in this file
	const result = spawnSync(command, { cwd, stdio: "inherit", shell: true, env: { ...env, ...extra } });
	if (result.status !== 0) {
		console.error(`\n${title} failed.`);
		process.exit(result.status ?? 1);
	}
	console.log(`  done in ${((Date.now() - started) / 1000).toFixed(1)}s`);
}

/** The window and installer icon: the app's SVG icon drawn at 256 px (Windows wants a bitmap). */
function icon() {
	const { Resvg } = require("@resvg/resvg-js");
	const svg = readFileSync(join(web, "app", "icon.svg"));
	writeFileSync(join(desktop, "icon.png"), new Resvg(svg, { fitTo: { mode: "width", value: 256 } }).render().asPng());
}

function assemble() {
	console.log("\n> assemble the standalone server");
	for (const [from, to] of [
		[join(web, ".next", "static"), join(standalone, ".next", "static")],
		[join(web, "public"), join(standalone, "public")],
	]) {
		rmSync(to, { recursive: true, force: true });
		if (existsSync(from)) cpSync(from, to, { recursive: true });
	}
}

/**
 * The engine, for this system. On Linux the program is told to look for the libraries it loads (ONNX Runtime's
 * WebGPU provider) in its own folder, where the packages put them; Windows looks there by itself.
 */
function engine() {
	const linux = process.platform === "linux" ? { RUSTFLAGS: `${process.env.RUSTFLAGS ?? ""} -C link-arg=-Wl,-rpath,$ORIGIN`.trim() } : {};
	run("build the engine", "cargo build --release --locked -p constellate", repo, linux);
}

/** The engine staged for packaging, and started from there once: a library it needs and lacks fails here. */
async function stageEngine() {
	const { stage, smokeStart } = require(join(desktop, "engine-files.cjs"));
	console.log("\n> stage the engine");
	for (const file of stage(join(repo, "target", "release"), join(desktop, "engine"))) console.log(`  ${file}`);
	await smokeStart(join(desktop, "engine"));
	console.log("  it starts from the staged folder");
}

run("build the interface", "npx turbo run build --filter=@rp/web");
assemble();
icon();
if (!flags.has("--interface")) engine();
if (flags.has("--dist")) {
	await stageEngine();
	// the packages carry the root package's version, the one a release is tagged with
	const { version } = JSON.parse(readFileSync(join(repo, "package.json"), "utf8"));
	const target = process.platform === "win32" ? "--win --x64" : process.platform === "linux" ? "--linux --x64" : null;
	if (!target) {
		console.error(`No package is made for ${process.platform}: the app is packaged for Windows and Linux.`);
		process.exit(1);
	}
	run(
		"package",
		`npx electron-builder ${target} --publish never -c.extraMetadata.version=${version} -c.extraMetadata.name=constellate`,
		desktop,
	);
}
if (flags.has("--run")) run("run", "npx electron .", desktop);
