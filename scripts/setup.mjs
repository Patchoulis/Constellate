/**
 * Get a fresh clone ready to work on, in one command:    npm run setup
 *
 * It checks for what the project is built with and installs what is missing, asking nothing:
 *
 *   Rust        the engine. Installed with rustup (https://rustup.rs), the Rust project's own installer, into
 *               your user folder; no administrator rights. On Windows it also needs Microsoft's C++ build
 *               tools, which this cannot install for you: it says how if they are missing.
 *   rustfmt, clippy   the formatter and linter `npm run pr` runs
 *   cargo-deny  the check of the engine's crates for known vulnerabilities and licences (deny.toml)
 *   packages    `npm ci` for the interface and the desktop shell
 *   git hooks   the checks on commit messages and against committing secrets (.githooks)
 *
 * Running it again is safe: every step first looks whether it is already done.   --check only reports.
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const windows = process.platform === "win32";
const checkOnly = process.argv.includes("--check");
const cargoBin = join(homedir(), ".cargo", "bin");
// a toolchain installed a moment ago is not on this process's PATH yet
const env = { ...process.env, PATH: `${cargoBin}${windows ? ";" : ":"}${process.env.PATH}` };

function has(command, args = ["--version"]) {
	const found = windows
		? spawnSync([command, ...args].join(" "), { env, shell: true, encoding: "utf8" })
		: spawnSync(command, args, { env, encoding: "utf8" });
	return found.status === 0 ? found.stdout.trim().split("\n")[0] : null;
}

function run(title, command, args, cwd = root) {
	console.log(`\n> ${title}`);
	// through the shell on Windows (npm and rustup are .cmd or need the PATH); every argument is written in this file
	const done = windows
		? spawnSync([command, ...args].join(" "), { cwd, env, stdio: "inherit", shell: true })
		: spawnSync(command, args, { cwd, env, stdio: "inherit" });
	if (done.status !== 0) {
		console.error(`\n${title} failed.`);
		process.exit(done.status ?? 1);
	}
}

/** Download a file with what the system already has (no dependency is installed yet when this runs). */
async function download(url, to) {
	const response = await fetch(url);
	if (!response.ok) throw new Error(`${url} answered ${response.status}`);
	writeFileSync(to, Buffer.from(await response.arrayBuffer()));
}

async function rust() {
	const version = has("cargo");
	if (version) return console.log(`  Rust: ${version}`);
	if (checkOnly) return missing.push("Rust (run: npm run setup)");
	if (windows && !msvc()) {
		console.error(
			"\nRust on Windows needs Microsoft's C++ build tools, which are not installed.\n" +
				'Install them (free): winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"\n' +
				"then run npm run setup again.",
		);
		process.exit(1);
	}
	console.log("\n> install Rust with rustup (into your user folder)");
	const folder = mkdtempSync(join(tmpdir(), "rustup-"));
	if (windows) {
		const installer = join(folder, "rustup-init.exe");
		await download("https://win.rustup.rs/x86_64", installer);
		run("rustup-init", `"${installer}"`, ["-y", "--default-toolchain", "stable", "--profile", "default", "--no-modify-path"]);
		console.log(
			`  Rust is in ${cargoBin}. Add that folder to your PATH to use cargo from a new terminal (rustup can: rustup-init without --no-modify-path).`,
		);
	} else {
		const script = join(folder, "rustup-init.sh");
		await download("https://sh.rustup.rs", script);
		run("rustup-init", "sh", [script, "-y", "--default-toolchain", "stable", "--profile", "default"]);
	}
}

/** Are Microsoft's C++ build tools installed? (Visual Studio's own locator answers.) */
function msvc() {
	const locator = join(process.env["ProgramFiles(x86)"] ?? "", "Microsoft Visual Studio", "Installer", "vswhere.exe");
	if (!existsSync(locator)) return false;
	const found = spawnSync(
		locator,
		["-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
		{
			encoding: "utf8",
		},
	);
	return found.status === 0 && found.stdout.trim() !== "";
}

function components() {
	if (!has("cargo")) return;
	for (const [component, probe] of [
		["rustfmt", ["fmt", "--version"]],
		["clippy", ["clippy", "--version"]],
	]) {
		if (has("cargo", probe)) console.log(`  ${component}: present`);
		else if (checkOnly) missing.push(`${component} (run: npm run setup)`);
		else run(`add ${component}`, "rustup", ["component", "add", component]);
	}
	// the advisory and licence check of the engine's crates (deny.toml); built from source, a few minutes once
	if (has("cargo", ["deny", "--version"])) console.log("  cargo-deny: present");
	else if (checkOnly) missing.push("cargo-deny (run: npm run setup)");
	else run("install cargo-deny", "cargo", ["install", "cargo-deny", "--locked"]);
}

function packages() {
	if (existsSync(join(root, "node_modules", ".package-lock.json"))) return console.log("  npm packages: installed");
	if (checkOnly) return missing.push("npm packages (run: npm run setup)");
	run("install npm packages", "npm", ["ci"]);
}

function hooks() {
	const set = spawnSync("git", ["config", "--get", "core.hooksPath"], { cwd: root, encoding: "utf8" }).stdout.trim();
	if (set === ".githooks") return console.log("  git hooks: on");
	if (checkOnly) return missing.push("git hooks (run: npm run setup)");
	run("switch the git hooks on", "git", ["config", "core.hooksPath", ".githooks"]);
}

const missing = [];
console.log(`Setting up ${root}${checkOnly ? " (checking only)" : ""}`);
await rust();
components();
packages();
hooks();
if (missing.length) {
	console.error(`\nMissing:\n${missing.map((line) => `  - ${line}`).join("\n")}`);
	process.exit(1);
}
console.log("\nReady.  npm run app  builds and opens the app;  npm run pr  runs every check.");
