/**
 * electron-builder hook, run once the app folder is laid out and before the installer is made.
 *
 * The interface's server (Next's standalone output) goes into the app's resources here rather than through
 * `extraResources`: electron-builder leaves every folder named node_modules out of extra resources, and the
 * server cannot start without its own.
 */
const fs = require("node:fs");
const path = require("node:path");

exports.default = async function afterPack({ appOutDir }) {
	const from = path.join(__dirname, "..", "web", ".next", "standalone");
	const to = path.join(appOutDir, "resources", "web");
	if (!fs.existsSync(path.join(from, "node_modules"))) throw new Error(`The interface is not built (${from}). Run: npm run build`);
	fs.rmSync(to, { recursive: true, force: true });
	fs.cpSync(from, to, { recursive: true, dereference: true });
};
