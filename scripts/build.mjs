/**
 * Build the app, and optionally run or package it.
 *
 *   npm run build     build the interface (Turborepo caches it: an unchanged tree builds in about a second)
 *   npm run app       build, then open the desktop app from this repository
 *   npm run dist      build, then make the Windows installer in dist/
 *
 * The steps, in order; each is skipped unless its flag asks for it:
 *   1. turbo run build     typecheck-free production build of apps/web (Next standalone output)
 *   2. assemble            copy the files Next leaves out of the standalone folder (static assets, public/),
 *                          and draw the window icon from apps/web/app/icon.svg
 *   3. --run               start Electron on the result
 *   4. --dist              electron-builder: shell + interface + backend sources -> dist/
 */
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const web = join(repo, "apps", "web");
const standalone = join(web, ".next", "standalone", "apps", "web");
const flags = new Set(process.argv.slice(2));

function run(title, command, cwd = repo) {
	console.log(`\n> ${title}`);
	const started = Date.now();
	// through the shell: npx is a .cmd file on Windows
	const result = spawnSync(command, { cwd, stdio: "inherit", shell: true });
	if (result.status !== 0) {
		console.error(`\n${title} failed.`);
		process.exit(result.status ?? 1);
	}
	console.log(`  done in ${((Date.now() - started) / 1000).toFixed(1)}s`);
}

/** The window and installer icon: the app's SVG icon drawn at 256 px (Windows wants a bitmap). */
function icon() {
	const { Resvg } = createRequire(import.meta.url)("@resvg/resvg-js");
	const svg = readFileSync(join(web, "app", "icon.svg"));
	writeFileSync(join(repo, "apps", "desktop", "icon.png"), new Resvg(svg, { fitTo: { mode: "width", value: 256 } }).render().asPng());
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

run("build", "npx turbo run build");
assemble();
icon();
if (flags.has("--dist")) run("package", "npx electron-builder --win --x64", join(repo, "apps", "desktop"));
if (flags.has("--run")) run("run", "npx electron .", join(repo, "apps", "desktop"));
