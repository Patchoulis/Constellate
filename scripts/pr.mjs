/**
 * Everything a pull request must pass, in one command:  npm run pr      (npm run pr -- --fix  also formats)
 *
 *   format   cargo fmt (Rust), prettier (TypeScript, JSON, Markdown, YAML) and ruff format (Python)
 *   lint     clippy with warnings denied (Rust), ruff check (Python), and the naming rule below
 *   types    tsc
 *   tests    cargo test, pytest, the interface's tests, every test of the desktop shell (the embedded reader's
 *            confinement too, in a real Electron), and the scripts' and git hooks' tests
 *
 * The naming rule: a caught error is called `error`, in both languages: `except X as error`, `catch (error)`.
 * One name everywhere means a reader never has to look up what `e`, `err`, `ex` or `problem` is this time.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, extname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const backend = join(root, "apps", "backend");
const desktop = join(root, "apps", "desktop");
const fix = process.argv.includes("--fix");
const python = process.env.RP_PYTHON || "python";

/** The project's own source files: what git tracks or would track, so nobody's virtual environment is read. */
function sources() {
	const listed = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], { cwd: root, encoding: "utf8" });
	return listed.stdout
		.split("\n")
		.filter((name) => [".py", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".cjs", ".mjs"].includes(extname(name)))
		.map((name) => join(root, name))
		.filter((path) => existsSync(path));
}

/**
 * The name a line catches an error under, when it does: `except X as e:`, `catch (e)`, `catch (e: unknown)`, or
 * a promise's `.catch((e) => ...)` / `.catch(e => ...)` / `.catch(function (e) ...)`. Null for any other line,
 * and for a comment (which may show how not to write it).
 */
export function caughtName(line, python) {
	if (/^\s*(\*|\/\/|\/\*|#)/.test(line)) return null;
	const pattern = python
		? /\bexcept\b[^:\n]*\bas\s+(\w+)\s*:/
		: /(?<!\.)\bcatch\s*\(\s*(\w+)\s*(?::[^)]*)?\)|\.catch\(\s*(?:async\s+)?(?:function\s*\w*\s*)?\(?\s*(\w+)\s*(?::[^)]*)?\)?\s*(?:=>|\{)/;
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
				const name = caughtName(line, extname(path) === ".py");
				if (name !== null && name !== "error") found.push(`${relative(root, path)}:${index + 1}: caught as \`${name}\`, not \`error\``);
			});
	}
	return found;
}

const steps = [
	["format (cargo fmt)", () => run("cargo", ["fmt", "--all", ...(fix ? [] : ["--check"])], root)],
	["format (prettier)", () => run("npx", ["prettier", fix ? "--write" : "--check", "--log-level", "warn", "."], root)],
	["format (ruff)", () => run(python, ["-m", "ruff", "format", ...(fix ? [] : ["--check"]), "."], backend)],
	["lint (ruff)", () => run(python, ["-m", "ruff", "check", ...(fix ? ["--fix"] : []), "."], backend)],
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
	["tests (backend)", () => run(python, ["-W", "ignore", "-m", "pytest", "-q"], backend)],
	["tests (interface)", () => run("npx", ["turbo", "run", "test", "--filter=@rp/web"], root)],
	// every test file there is, so that a new one cannot be left out of the list by mistake
	["tests (desktop)", () => run("node", ["--test", ...testsIn(desktop, ".test.cjs")], desktop)],
	// a hostile page in the embedded reader, inside a real Electron (a hidden window; about two minutes)
	["tests (desktop, the reader in Electron)", () => run("npx", ["electron", "reader.electron.cjs"], desktop)],
	[
		"tests (scripts and hooks)",
		() => run("node", ["--test", ...testsIn(join(root, "scripts"), ".test.mjs").map((file) => `scripts/${file}`)], root),
	],
];

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
	const chosen = steps.filter(([name]) => !only || name.startsWith(only));
	if (chosen.length === 0) {
		console.error(`--only=${only} names no step. The steps: ${steps.map(([name]) => name).join(", ")}.`);
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
