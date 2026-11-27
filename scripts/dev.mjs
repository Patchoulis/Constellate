/**
 * Work on the interface with its changes shown at once:    npm run dev
 *
 * Starts the engine (cargo run, the release build) as the desktop shell does (apps/desktop/engine.cjs): in the data
 * folder (data/, or CONSTELLATE_DATA), on this computer only, with a token it makes up; then `next dev` on
 * http://localhost:3210, told the engine's address and token, which its server adds to what it forwards (proxy.ts).
 * Open http://localhost:3210 in a browser, or the desktop window on it: RP_DEV_URL=http://localhost:3210 npm start
 * -w @rp/desktop. Ctrl+C stops both.
 */
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { EngineLog, PROVIDER_VARIABLES, startEngine } = require("../apps/desktop/engine.cjs");

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const windows = process.platform === "win32";
const env = { ...process.env, PATH: `${join(homedir(), ".cargo", "bin")}${windows ? ";" : ":"}${process.env.PATH}` };
const asked = process.env.CONSTELLATE_DATA?.trim();
const data = asked && isAbsolute(asked) ? asked : join(repo, "data");
const ORIGIN = "http://localhost:3210";
const withoutKeys = (source) => Object.fromEntries(Object.entries(source).filter(([name]) => !PROVIDER_VARIABLES.includes(name)));

console.log("> build the engine");
if (spawnSync("cargo build --release -p constellate", { cwd: repo, env, stdio: "inherit", shell: true }).status !== 0) process.exit(1);
mkdirSync(data, { recursive: true });
const log = new EngineLog(join(data, "logs"));
const engine = await startEngine({
	binary: join(repo, "target", "release", windows ? "constellate.exe" : "constellate"),
	data,
	origin: ORIGIN,
	env,
	log,
});
console.log(`> the engine listens at ${engine.url} (its log: ${join(data, "logs", "engine.log")})`);
// Next's own program run by this Node, with no shell between (as `npm run dev -w @rp/web` would run it): stopping it
// then stops it, where on Windows it would stop only the shell
const next = require.resolve("next/dist/bin/next", { paths: [join(repo, "apps", "web")] });
const web = spawn(process.execPath, [next, "dev", "-H", "localhost", "-p", "3210"], {
	cwd: join(repo, "apps", "web"),
	stdio: "inherit",
	// the interface's server has no use for a provider's key, here as in the app
	env: { ...withoutKeys(process.env), CONSTELLATE_ENGINE: engine.url, CONSTELLATE_ENGINE_TOKEN: engine.token },
});
const stop = () => {
	engine.child.stdin.end();
	web.kill();
};
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, stop);
web.once("exit", (code) => {
	engine.child.stdin.end();
	process.exitCode = code ?? 0;
});
engine.exited.then((code) => {
	console.error(`> the engine stopped (code ${code})`);
	web.kill();
});
