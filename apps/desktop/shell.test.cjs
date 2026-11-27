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

test("the shell's tests are not packaged", () => {
	assert.deepEqual(
		packaged().filter((name) => name.endsWith(".test.cjs")),
		[],
	);
});
