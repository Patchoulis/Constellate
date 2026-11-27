/** The desktop app's lifecycle: which steps are legal, and what each state allows.    node --test */
const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");
const { Lifecycle, TRANSITIONS, withParent } = require("./lifecycle.cjs");

/** A lifecycle taken along `path`; every step must be legal. */
function at(...path) {
	const life = new Lifecycle();
	for (const next of path) assert.ok(life.move(next), `${life.state} -> ${next} should be legal`);
	return life;
}

test("the ordinary paths: opened and closed; closed while starting; a failure", () => {
	assert.equal(at("serving", "ready", "quitting", "stopped").state, "stopped");
	assert.equal(at("serving", "quitting", "stopped").state, "stopped");
	assert.equal(at("quitting", "stopped").state, "stopped");
	assert.equal(at("serving", "failed", "quitting", "stopped").state, "stopped");
});

test("every state's legal steps are exactly the listed ones, and an illegal step changes nothing", () => {
	const reach = {
		starting: [],
		serving: ["serving"],
		ready: ["serving", "ready"],
		failed: ["failed"],
		quitting: ["quitting"],
		stopped: ["quitting", "stopped"],
	};
	const states = Object.keys(TRANSITIONS);
	assert.deepEqual(states.sort(), Object.keys(reach).sort());
	for (const from of states) {
		for (const to of states) {
			const life = at(...reach[from]);
			const moved = life.move(to);
			assert.equal(moved, TRANSITIONS[from].includes(to), `${from} -> ${to}`);
			assert.equal(life.state, moved ? to : from);
		}
	}
});

test("nothing leaves stopped, quitting only ends, and failed only goes on to quitting", () => {
	const stopped = at("quitting", "stopped");
	for (const next of Object.keys(TRANSITIONS)) assert.equal(stopped.move(next), false);
	const quitting = at("quitting");
	for (const next of ["starting", "serving", "ready", "failed", "quitting"]) assert.equal(quitting.move(next), false, next);
	const failed = at("serving", "failed");
	assert.equal(failed.move("ready"), false);
	assert.equal(failed.move("serving"), false);
	assert.ok(failed.move("quitting"));
});

test("there is no going back: ready never returns to serving", () => {
	const life = at("serving", "ready");
	assert.equal(life.move("serving"), false);
	assert.equal(life.move("starting"), false);
});

test("a process may be started only while starting up", () => {
	assert.ok(at().maySpawn);
	assert.ok(at("serving").maySpawn);
	assert.equal(at("serving", "ready").maySpawn, false);
	assert.equal(at("serving", "failed").maySpawn, false);
	assert.equal(at("quitting").maySpawn, false); // closed while starting: nothing more is started
});

test("a server stopping is a failure to report only once the app is ready", () => {
	assert.ok(at("serving", "ready").childExitIsFailure);
	assert.equal(at("serving").childExitIsFailure, false); // whoever waits for it reports it
	assert.equal(at("serving", "ready", "quitting").childExitIsFailure, false); // it was asked to stop
});

test("closing every window quits the app only when there is a main window or an error to have closed", () => {
	assert.equal(at().quitsWhenWindowsClose, false);
	assert.equal(at("serving").quitsWhenWindowsClose, false);
	assert.ok(at("serving", "ready").quitsWhenWindowsClose);
	assert.ok(at("serving", "failed").quitsWhenWindowsClose);
});

test("the app ends only once its children have been stopped", () => {
	for (const path of [[], ["serving"], ["serving", "ready"], ["serving", "failed"], ["quitting"]]) {
		assert.equal(at(...path).mayEnd, false, path.join(" -> "));
	}
	assert.ok(at("serving", "ready", "quitting", "stopped").mayEnd);
});

test("a server run with the shell's watch on it runs, and stops once the shell's end of its input is gone", async () => {
	// an ES module that would run for ever, as the interface's server.js does, and says when it has started
	const folder = fs.mkdtempSync(path.join(os.tmpdir(), "rp-parent-"));
	const script = path.join(folder, "server.mjs");
	fs.writeFileSync(script, "setInterval(() => {}, 1000);\nconsole.log(`started ${import.meta.url.length > 0}`);\n");
	const child = spawn(process.execPath, withParent(script), { stdio: ["pipe", "pipe", "inherit"] });
	const exited = new Promise((resolve) => child.once("exit", (code) => resolve(code)));
	await new Promise((resolve) => child.stdout.once("data", resolve));
	child.stdin.end(); // what the system does to the pipe when the shell ends, however it ends
	const timer = setTimeout(() => child.kill(), 10_000);
	assert.equal(await exited, 0); // killed by the timer instead, it would have no exit code
	clearTimeout(timer);
	fs.rmSync(folder, { recursive: true, force: true });
});
