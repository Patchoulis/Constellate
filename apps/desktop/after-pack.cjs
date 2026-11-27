/**
 * electron-builder hook, run once the app folder is laid out and before the installer is made.
 *
 * 1. The interface's server (Next's standalone output) goes into the app's resources here rather than through
 *    `extraResources`: electron-builder leaves every folder named node_modules out of extra resources, and the
 *    server cannot start without its own.
 * 2. The licences of everything that ships are gathered into resources/licenses: the standalone output keeps
 *    packages' code but drops their licence files, and those licences ask to travel with the code.
 */
const fs = require("node:fs");
const path = require("node:path");

const repo = path.join(__dirname, "..", "..");
const LICENCE = /^(licen[sc]e|copying|notice)/i;

/** Every package folder under a node_modules folder: "react", "@next/env", ... */
function packagesIn(modules) {
	if (!fs.existsSync(modules)) return [];
	return fs.readdirSync(modules).flatMap((name) => {
		if (name.startsWith(".")) return [];
		return name.startsWith("@") ? fs.readdirSync(path.join(modules, name)).map((inner) => `${name}/${inner}`) : [name];
	});
}

function copyLicences(from, to) {
	if (!fs.existsSync(from)) return 0;
	const files = fs.readdirSync(from).filter((file) => LICENCE.test(file) && fs.statSync(path.join(from, file)).isFile());
	for (const file of files) {
		fs.mkdirSync(to, { recursive: true });
		fs.copyFileSync(path.join(from, file), path.join(to, file));
	}
	return files.length;
}

/** A file that must never ship: an environment file (keys live there), a private key or certificate store. */
const SECRET = /^(\.env(\..*)?|.*\.env|.*\.pem|.*\.key|.*\.p12|.*\.pfx)$/i;

/** Every file under `folder` named like a secret (the list of names, .env.example, holds none and is kept). */
function secretFilesIn(folder) {
	return fs
		.readdirSync(folder, { recursive: true, withFileTypes: true })
		.filter((entry) => entry.isFile() && SECRET.test(entry.name) && entry.name !== ".env.example")
		.map((entry) => path.join(entry.parentPath, entry.name));
}

exports.secretFilesIn = secretFilesIn;

exports.default = async function afterPack({ appOutDir }) {
	const from = path.join(__dirname, "..", "web", ".next", "standalone");
	const resources = path.join(appOutDir, "resources");
	if (!fs.existsSync(path.join(from, "node_modules"))) throw new Error(`The interface is not built (${from}). Run: npm run build`);
	fs.rmSync(path.join(resources, "web"), { recursive: true, force: true });
	fs.cpSync(from, path.join(resources, "web"), { recursive: true, dereference: true });
	// Next's standalone output can carry the interface's .env files, where a developer's keys may be: the
	// interface needs none of them, and an installer is handed to other people.
	for (const file of secretFilesIn(resources)) {
		console.log(`  left out of the installer: ${path.relative(resources, file)}`);
		fs.rmSync(file);
	}
	// The image library is traced in although the interface resizes no images (next.config.ts: unoptimized), and
	// it brings LGPL code along: it is not shipped.
	for (const unused of ["sharp", "@img"]) {
		fs.rmSync(path.join(resources, "web", "node_modules", unused), { recursive: true, force: true });
	}

	const licences = path.join(resources, "licenses");
	// what the server runs on, plus what is compiled into the pages (the bundler keeps no licence files either)
	const shipped = new Set([
		...packagesIn(path.join(resources, "web", "node_modules")),
		"katex",
		"d3-force",
		"d3-zoom",
		"d3-selection",
		"lucide-react",
		"radix-ui",
	]);
	const without = [];
	for (const name of shipped) {
		const found = [path.join(repo, "node_modules", name), path.join(from, "node_modules", name)].some((folder) =>
			copyLicences(folder, path.join(licences, name)),
		);
		if (found) continue;
		// a package without a licence file still names its licence in its package.json: that much travels with it
		const manifest = [path.join(repo, "node_modules", name), path.join(from, "node_modules", name)]
			.map((folder) => path.join(folder, "package.json"))
			.find((file) => fs.existsSync(file));
		const licence = manifest && JSON.parse(fs.readFileSync(manifest, "utf8")).license;
		if (!licence) {
			without.push(name);
			continue;
		}
		fs.mkdirSync(path.join(licences, name), { recursive: true });
		const note = `${name} is distributed under the ${licence} licence (from its package.json; it ships no licence file).\n`;
		fs.writeFileSync(path.join(licences, name, "LICENSE.txt"), note);
	}
	copyLicences(path.join(__dirname, "bin"), path.join(licences, "uv"));
	fs.copyFileSync(path.join(repo, "THIRD_PARTY_NOTICES.md"), path.join(licences, "THIRD_PARTY_NOTICES.md"));
	if (without.length) console.log(`  licences: none found for ${without.join(", ")}`);
};
