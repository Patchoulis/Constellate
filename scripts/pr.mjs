/**
 * Everything a pull request must pass, in one command:  npm run pr      (npm run pr -- --fix  also formats)
 *
 *   format   prettier (TypeScript, JSON, Markdown, YAML) and ruff format (Python)
 *   lint     ruff check, and the naming rule below
 *   types    tsc
 *   tests    pytest, the interface's tests, and the desktop app's first-start setup
 *
 * The naming rule: a caught error is called `error`, in both languages: `except X as error`, `catch (error)`.
 * One name everywhere means a reader never has to look up what `e`, `err`, `ex` or `problem` is this time.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const backend = join(root, "apps", "backend");
const fix = process.argv.includes("--fix");
const python = process.env.RP_PYTHON || "python";

/** The project's own source files: what git tracks or would track, so nobody's virtual environment is read. */
function sources() {
	const listed = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], { cwd: root, encoding: "utf8" });
	return listed.stdout
		.split("\n")
		.filter((name) => [".py", ".ts", ".tsx", ".cjs", ".mjs"].includes(extname(name)))
		.map((name) => join(root, name))
		.filter((path) => existsSync(path));
}

/** Lines where a caught error has another name than `error`. */
function misnamedErrors() {
	const found = [];
	for (const path of sources()) {
		const pattern = extname(path) === ".py" ? /\bexcept\b[^:\n]*\bas\s+(\w+)\s*:/ : /\bcatch\s*\(\s*(\w+)\s*\)/;
		readFileSync(path, "utf8")
			.split("\n")
			.forEach((line, index) => {
				const caught = pattern.exec(line);
				if (caught && caught[1] !== "error") found.push(`${relative(root, path)}:${index + 1}: caught as \`${caught[1]}\`, not \`error\``);
			});
	}
	return found;
}

const steps = [
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
	["types", () => run("npx", ["turbo", "run", "typecheck"], root)],
	["tests (backend)", () => run(python, ["-W", "ignore", "-m", "pytest", "-q"], backend)],
	["tests (interface)", () => run("npx", ["turbo", "run", "test", "--filter=@rp/web"], root)],
	["tests (desktop)", () => run("node", ["--test", "setup.test.cjs", "lifecycle.test.cjs"], join(root, "apps", "desktop"))],
];

function run(command, args, cwd) {
	// through the shell (npx is a .cmd on Windows); every argument here is written in this file, none comes from outside
	return spawnSync([command, ...args].join(" "), { cwd, stdio: "inherit", shell: true }).status === 0;
}

const only = process.argv.find((arg) => arg.startsWith("--only="))?.slice(7);
const failed = [];
for (const [name, step] of steps) {
	if (only && !name.startsWith(only)) continue;
	console.log(`\n> ${name}`);
	if (!step()) failed.push(name);
}
if (failed.length) {
	console.error(`\nNot ready: ${failed.join(", ")}.${fix ? "" : "  `npm run pr -- --fix` formats and fixes what it can."}`);
	process.exit(1);
}
console.log("\nReady for a pull request.");
