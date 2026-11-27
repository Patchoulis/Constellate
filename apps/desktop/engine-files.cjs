/**
 * What of the engine goes into the app: the program, and the libraries ONNX Runtime loads at run time from beside
 * it. The `ort` crate (=2.0.0-rc.15, default features) links ONNX Runtime itself into the program, and its build
 * puts next to the program, in target/release, the shared libraries the prebuilt runtime cannot link statically
 * ("copy-dylibs"):
 *
 *   Windows   DirectML.dll            DirectML, through which ONNX Runtime reaches any graphics card (DirectX 12);
 *                                     D3D12, DXGI and DXCore are the system's own
 *   Linux     libwebgpu_dawn.so       Dawn, the WebGPU implementation ONNX Runtime's WebGPU provider runs on (it
 *                                     reaches the card through the system's Vulkan driver); the program is linked
 *                                     to look for it in its own folder (scripts/build.mjs)
 *
 * Those are exactly the shared libraries in target/release: staging copies the program and every one of them
 * (links followed: on Linux they are links into ort's download cache), and refuses to go on without the ones
 * named above. Used by scripts/build.mjs --dist; tested by servers.test.cjs.
 */
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const PROGRAM = { win32: "constellate.exe", linux: "constellate", darwin: "constellate" };
/** The files the packaged engine cannot start without, by system. */
const REQUIRED = {
	win32: ["constellate.exe", "DirectML.dll"],
	linux: ["constellate", "libwebgpu_dawn.so"],
	darwin: ["constellate"],
};

/** A shared library, by its name: `x.dll`, `libx.so`, `libx.so.1.2`, `libx.dylib`. */
const SHARED_LIBRARY = /(\.dll|\.so(\.\d+)*|\.dylib)$/i;

/** The engine's files in a build folder: the program and the shared libraries beside it. */
function engineFilesIn(folder, platform = process.platform) {
	const names = fs.readdirSync(folder).filter((name) => SHARED_LIBRARY.test(name) && fs.statSync(path.join(folder, name)).isFile());
	return [PROGRAM[platform], ...names.sort()];
}

/** Of the files the engine needs on `platform`, those missing from `folder`. */
function missingFrom(folder, platform = process.platform) {
	return REQUIRED[platform].filter((name) => !fs.existsSync(path.join(folder, name)));
}

/** Copy the engine from the build folder `from` into `to` (made afresh); the names copied. */
function stage(from, to, platform = process.platform) {
	const lacking = missingFrom(from, platform);
	if (lacking.length) throw new Error(`The engine's build in ${from} lacks ${lacking.join(", ")}. Run: npm run build`);
	fs.rmSync(to, { recursive: true, force: true });
	fs.mkdirSync(to, { recursive: true });
	const names = engineFilesIn(from, platform);
	for (const name of names) {
		fs.copyFileSync(path.join(from, name), path.join(to, name)); // follows a link: the file itself is copied
		if (platform !== "win32") fs.chmodSync(path.join(to, name), 0o755);
	}
	return names;
}

/** Start the engine from `folder` once, in a data folder of its own, and stop it: resolves once it said where it
 *  listens, rejects with what it said if it could not start (a library it cannot load, say). */
async function smokeStart(folder, platform = process.platform) {
	const { EngineLog, startEngine } = require("./engine.cjs");
	const data = fs.mkdtempSync(path.join(os.tmpdir(), "constellate-engine-"));
	const log = new EngineLog(path.join(data, "logs"));
	try {
		const engine = await startEngine({ binary: path.join(folder, PROGRAM[platform]), data, origin: "http://127.0.0.1:9", log });
		engine.child.stdin.end();
		await engine.exited;
	} finally {
		fs.rmSync(data, { recursive: true, force: true });
	}
}

module.exports = { PROGRAM, REQUIRED, engineFilesIn, missingFrom, smokeStart, stage };
