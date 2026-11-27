/**
 * Everything a pull request must pass, in one command:  npm run pr      (npm run pr -- --fix  also formats)
 *
 *   format   cargo fmt (Rust) and prettier (TypeScript, JSON, Markdown, YAML)
 *   lint     clippy with warnings denied (Rust), and the naming rule below
 *   types    tsc
 *   tests    cargo test, the interface's tests, every test of the desktop shell (the two servers it starts, for
 *            real, against the interface's build, which is made first; the embedded reader's confinement, in a
 *            real Electron), and the scripts' and git hooks' tests
 *   advisories   known vulnerabilities in the dependencies, from each ecosystem's own database (needs the network):
 *            npm audit (what ships: moderate and above; everything, build tools included: high and above),
 *            cargo deny (deny.toml: RustSec advisories, licences, sources)
 *
 *   --only=<start>   only the steps whose name starts so (--only=advisories)
 *   --skip=<start>   all steps but those (CI runs the advisories as a job of their own: --skip=advisories)
 *
 * A step whose tool is missing fails, and says how to install it: a check that did not run has not passed.
 *
 * The naming rule: a caught error is called `error`: `catch (error)`, `.catch((error) => ...)`. One name everywhere
 * means a reader never has to look up what `e`, `err`, `ex` or `problem` is this time.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, extname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = join(root, "apps", "desktop");
const fix = process.argv.includes("--fix");

/** The project's own source files: what git tracks or would track, so nothing installed is read. */
function sources() {
	const listed = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], { cwd: root, encoding: "utf8" });
	return listed.stdout
		.split("\n")
		.filter((name) => [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".cjs", ".mjs"].includes(extname(name)))
		.map((name) => join(root, name))
		.filter((path) => existsSync(path));
}

/**
 * The name a line catches an error under, when it does: `catch (e)`, `catch (e: unknown)`, or a promise's
 * `.catch((e) => ...)` / `.catch(e => ...)` / `.catch(function (e) ...)`. Null for any other line, and for a
 * comment (which may show how not to write it).
 */
export function caughtName(line) {
	if (/^\s*(\*|\/\/|\/\*)/.test(line)) return null;
	const pattern =
		/(?<!\.)\bcatch\s*\(\s*(\w+)\s*(?::[^)]*)?\)|\.catch\(\s*(?:async\s+)?(?:function\s*\w*\s*)?\(?\s*(\w+)\s*(?::[^)]*)?\)?\s*(?:=>|\{)/;
	const caught = pattern.exec(line);
	return caught ? (caught[1] ?? caught[2]) : null;
}

/** Lines where a caught error has another name than `error`. */
function misnamedErrors() {
	const found = [];
	for (const path of sources()) {
		readFileSync(path, "utf8")
			.split("\n")
			.forEach((line, index) => {
				const name = caughtName(line);
				if (name !== null && name !== "error") found.push(`${relative(root, path)}:${index + 1}: caught as \`${name}\`, not \`error\``);
			});
	}
	return found;
}

const steps = [
	["format (cargo fmt)", () => run("cargo", ["fmt", "--all", ...(fix ? [] : ["--check"])], root)],
	["format (prettier)", () => run("npx", ["prettier", fix ? "--write" : "--check", "--log-level", "warn", "."], root)],
	[
		"lint (a caught error is named `error`)",
		() => {
			const found = misnamedErrors();
			for (const line of found) console.error(line);
			return found.length === 0;
		},
	],
	["lint (clippy)", () => run("cargo", ["clippy", "--all-targets", "--quiet", "--", "-D", "warnings"], root)],
	["types", () => run("npx", ["turbo", "run", "typecheck"], root)],
	["tests (rust)", () => run("cargo", ["test", "--quiet"], root)],
	["tests (interface)", () => run("npx", ["turbo", "run", "test", "--filter=@rp/web"], root)],
	// the interface's server, which the shell's tests start (apps/desktop/servers.test.cjs); cached when unchanged
	["tests (desktop: the interface built first)", () => run("node", ["scripts/build.mjs", "--interface"], root)],
	// every test file there is, so that a new one cannot be left out of the list by mistake
	["tests (desktop)", () => run("node", ["--test", ...testsIn(desktop, ".test.cjs")], desktop)],
	// a hostile page in the embedded reader, inside a real Electron (a hidden window; about two minutes)
	["tests (desktop, the reader in Electron)", () => run("npx", ["electron", "reader.electron.cjs"], desktop)],
	[
		"tests (scripts and hooks)",
		() => run("node", ["--test", ...testsIn(join(root, "scripts"), ".test.mjs").map((file) => `scripts/${file}`)], root),
	],
	// What ships (the interface's server and its packages; Electron itself) at moderate and above: little ships, and a
	// moderate flaw in it may be reachable from a page the reader shows. The build tools (electron-builder, turbo,
	// tsx...) only ever see this repository's own files, so there high and above.
	["advisories (npm, what ships)", () => run("npm", ["audit", "--omit=dev", "--audit-level=moderate"], root)],
	["advisories (npm, build tools too)", () => run("npm", ["audit", "--audit-level=high"], root)],
	[
		"advisories (cargo)",
		() =>
			needs("cargo", ["deny", "--version"], "cargo install cargo-deny --locked") &&
			run("cargo", ["deny", "--log-level", "error", "check", "advisories", "licenses", "sources"], root),
	],
];

/** Is `command` there? If not, say how to get it: the step then fails rather than passing having checked nothing. */
function needs(command, probe, install) {
	if (spawnSync([command, ...probe].join(" "), { env, stdio: "ignore", shell: true }).status === 0) return true;
	console.error(`${command} ${probe.slice(0, -1).join(" ")} is not installed. Install it with: ${install}`);
	return false;
}

/** The names of the test files in `folder` (none is an error: the step would pass having tested nothing). */
function testsIn(folder, ending) {
	const files = readdirSync(folder).filter((name) => name.endsWith(ending));
	if (files.length === 0) throw new Error(`no ${ending} file in ${folder}`);
	return files.sort();
}

const env = { ...process.env, PATH: `${join(homedir(), ".cargo", "bin")}${process.platform === "win32" ? ";" : ":"}${process.env.PATH}` };

function run(command, args, cwd) {
	// through the shell (npx is a .cmd on Windows); every argument here is written in this file, none comes from outside
	return spawnSync([command, ...args].join(" "), { cwd, env, stdio: "inherit", shell: true }).status === 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
	const only = process.argv.find((arg) => arg.startsWith("--only="))?.slice(7);
	const skip = process.argv.find((arg) => arg.startsWith("--skip="))?.slice(7);
	const names = steps.map(([name]) => name).join(", ");
	// a --skip that names nothing would silently run what it was meant to leave out (or the other way round)
	if (skip !== undefined && !steps.some(([name]) => name.startsWith(skip))) {
		console.error(`--skip=${skip} names no step. The steps: ${names}.`);
		process.exit(1);
	}
	const chosen = steps.filter(([name]) => (!only || name.startsWith(only)) && !(skip && name.startsWith(skip)));
	if (chosen.length === 0) {
		console.error(`--only=${only} names no step. The steps: ${names}.`);
		process.exit(1);
	}
	const failed = [];
	for (const [name, step] of chosen) {
		console.log(`\n> ${name}`);
		if (!step()) failed.push(name);
	}
	if (failed.length) {
		console.error(`\nNot ready: ${failed.join(", ")}.${fix ? "" : "  `npm run pr -- --fix` formats and fixes what it can."}`);
		process.exit(1);
	}
	console.log("\nReady for a pull request.");
}
