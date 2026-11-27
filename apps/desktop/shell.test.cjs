/** The installed shell has every file it needs: what main.cjs requires, and what those require.    node --test */
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");

/** The shell's own files main.cjs loads, directly or through another of them. */
function required(file, found = new Set()) {
	for (const [, name] of fs.readFileSync(path.join(__dirname, file), "utf8").matchAll(/require\("\.\/([^"]+)"\)/g)) {
		if (found.has(name)) continue;
		found.add(name);
		required(name, found);
	}
	return found;
}

/** The files electron-builder.yml puts in the app: the entries of its top-level `files:` list. */
function packaged() {
	const text = fs.readFileSync(path.join(__dirname, "electron-builder.yml"), "utf8");
	const list = /^files:\n((?: {2}- .*\n)+)/m.exec(text);
	assert.ok(list, "electron-builder.yml has a files: list");
	return list[1]
		.split("\n")
		.map((line) =>
			line
				.replace(/^ {2}- /, "")
				.replace(/^"|"$/g, "")
				.trim(),
		)
		.filter(Boolean);
}

test("every file the shell requires is packaged into the app", () => {
	const files = packaged();
	const needed = [...required("main.cjs")];
	assert.ok(needed.length > 0);
	for (const name of ["main.cjs", ...needed])
		assert.ok(files.includes(name), `${name} is required but not in electron-builder.yml's files`);
});

test("environment files and keys are found anywhere in what is packaged, and the list of names is not", () => {
	const { secretFilesIn } = require("./after-pack.cjs");
	const folder = fs.mkdtempSync(path.join(require("node:os").tmpdir(), "rp-pack-"));
	const nested = path.join(folder, "web", "apps", "web");
	fs.mkdirSync(nested, { recursive: true });
	for (const name of [".env", ".env.local", "production.env", "server.pem", "signing.key", ".env.example", "server.js"]) {
		fs.writeFileSync(path.join(nested, name), "");
	}
	assert.deepEqual(
		secretFilesIn(folder)
			.map((file) => path.basename(file))
			.sort(),
		[".env", ".env.local", "production.env", "server.pem", "signing.key"],
	);
	fs.rmSync(folder, { recursive: true, force: true });
});

test("the licences gathered include every package compiled into the pages, their own dependencies too", () => {
	const { compiledIntoPages } = require("./after-pack.cjs");
	const shipped = compiledIntoPages();
	// the interface's own dependencies, and some it only has through them
	for (const name of [
		"katex",
		"clsx",
		"tailwind-merge",
		"class-variance-authority",
		"d3-zoom",
		"d3-drag",
		"d3-transition",
		"@floating-ui/dom",
	]) {
		assert.ok(shipped.includes(name), name);
	}
	assert.equal(shipped.includes("next"), false); // the server's part of Next is in the standalone output
	assert.equal(
		shipped.some((name) => name.startsWith("@rp/")),
		false,
	);
});

test("the shell's tests are not packaged", () => {
	assert.deepEqual(
		packaged().filter((name) => name.endsWith(".test.cjs")),
		[],
	);
});

test("every dialog and window the shell shows carries the app's own name and icon", () => {
	const main = fs.readFileSync(path.join(__dirname, "main.cjs"), "utf8");
	assert.match(main, /app\.setName\(NAME\)/, "the app is named before anything is shown");
	assert.match(main, /app\.setAppUserModelId\(APP_ID\)/, "Windows is told whose windows these are");
	assert.doesNotMatch(main, /showErrorBox/, "an error box has no icon and names the program that runs the app");
	assert.doesNotMatch(main, /app\.getName\(\)/, "the name shown is the app's, not the running program's");
	// each dialog is made by appDialog, which sets the title and the icon
	const shown = main.match(/dialog\.showMessageBox(Sync)?\(/g) ?? [];
	const made = main.match(/appDialog\("/g) ?? [];
	assert.ok(shown.length > 0);
	assert.ok(made.length >= 2, "the question about a site and the failure to start both use appDialog");
	const appId = /^appId: (.+)$/m.exec(fs.readFileSync(path.join(__dirname, "electron-builder.yml"), "utf8"))[1];
	assert.ok(main.includes(`const APP_ID = "${appId}"`), "the identity given to Windows is the installed app's");
});

test("the installed program file names the product, so that Windows names Constellate wherever it shows the app", () => {
	const builder = fs.readFileSync(path.join(__dirname, "electron-builder.yml"), "utf8");
	assert.match(builder, /^productName: Constellate$/m, "ProductName and FileDescription of the program file");
	assert.match(builder, /^copyright: .*Constellate$/m, "its copyright");
	assert.match(builder, /^win:\n(?: {2}.*\n)*? {2}executableName: Constellate$/m, "Constellate.exe, never electron.exe");
	assert.match(builder, /^ {2}icon: icon\.png$/m, "its icon");
});

test("the Linux packages are named, pictured and filed as the app", () => {
	const builder = fs.readFileSync(path.join(__dirname, "electron-builder.yml"), "utf8");
	const linux = /^linux:\n((?: {2}.*\n)+)/m.exec(builder)?.[1] ?? "";
	assert.match(linux, /^ {4}- AppImage$/m);
	assert.match(linux, /^ {4}- deb$/m);
	assert.match(linux, /^ {2}icon: icon\.png$/m);
	assert.match(linux, /^ {2}category: Science$/m);
});

test("the engine is what goes into the app's resources, and nothing of a Python setup does", () => {
	const builder = fs.readFileSync(path.join(__dirname, "electron-builder.yml"), "utf8");
	assert.match(builder, /^extraResources:\n {2}# .*\n {2}- from: engine\n {4}to: engine$/m);
	assert.doesNotMatch(builder, /backend|uv\b|requirements/);
});

test("the licences gathered for the engine are the crates compiled into it, for the system packaged", (t) => {
	const { cratesInEngine, engineMetadata } = require("./after-pack.cjs");
	let windows;
	try {
		windows = cratesInEngine(engineMetadata("win32"));
	} catch (error) {
		if (process.env.CI) throw error;
		return t.skip(`cargo is not on the PATH (${error.message.split("\n")[0]})`);
	}
	const linux = cratesInEngine(engineMetadata("linux"));
	const names = (crates) => new Set(crates.map((crate) => crate.name));
	for (const name of ["ort", "ort-sys", "axum", "rusqlite", "tokenizers", "serde"]) {
		assert.ok(names(windows).has(name), `${name} on Windows`);
		assert.ok(names(linux).has(name), `${name} on Linux`);
	}
	assert.ok(names(windows).has("windows-sys") && !names(linux).has("windows-sys"), "each system's own crates");
	// what runs only while building, and the tests' own crates, are not shipped
	for (const name of ["serde_derive", "thiserror-impl", "cc", "tempfile", "rcgen", "constellate"]) {
		assert.equal(names(windows).has(name), false, name);
	}
	for (const crate of windows) assert.ok(crate.license || crate.license_file, `${crate.name} names its licence`);
});
