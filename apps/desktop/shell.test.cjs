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
