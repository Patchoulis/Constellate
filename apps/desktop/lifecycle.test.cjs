/** The desktop app's lifecycle: which steps are legal, and what each state allows.    node --test */
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { Lifecycle, TRANSITIONS } = require("./lifecycle.cjs");

/** A lifecycle taken along `path`; every step must be legal. */
function at(...path) {
	const life = new Lifecycle();
	for (const next of path) assert.ok(life.move(next), `${life.state} -> ${next} should be legal`);
	return life;
}

test("the ordinary paths: an installed first start, a later start, a start from the repository", () => {
	assert.equal(at("installing", "serving", "ready", "quitting").state, "quitting");
	assert.equal(at("serving", "ready", "quitting").state, "quitting");
});

test("every state's legal steps are exactly the listed ones, and an illegal step changes nothing", () => {
	const states = Object.keys(TRANSITIONS);
	const reach = { starting: [], installing: ["installing"], serving: ["serving"], ready: ["serving", "ready"] };
	reach.failed = ["failed"];
	reach.quitting = ["quitting"];
	for (const from of states) {
		for (const to of states) {
			const life = at(...reach[from]);
			const moved = life.move(to);
			assert.equal(moved, TRANSITIONS[from].includes(to), `${from} -> ${to}`);
			assert.equal(life.state, moved ? to : from);
		}
	}
});

test("nothing leaves quitting, and failed only goes on to quitting", () => {
	const life = at("quitting");
	for (const next of Object.keys(TRANSITIONS)) assert.equal(life.move(next), false);
	const failed = at("serving", "failed");
	assert.equal(failed.move("ready"), false);
	assert.equal(failed.move("serving"), false);
	assert.ok(failed.move("quitting"));
});

test("there is no going back: ready never returns to installing or serving", () => {
	const life = at("serving", "ready");
	assert.equal(life.move("installing"), false);
	assert.equal(life.move("serving"), false);
	assert.equal(life.move("starting"), false);
});

test("a process may be started only while starting up", () => {
	assert.ok(at().maySpawn);
	assert.ok(at("installing").maySpawn);
	assert.ok(at("serving").maySpawn);
	assert.equal(at("serving", "ready").maySpawn, false);
	assert.equal(at("serving", "failed").maySpawn, false);
	assert.equal(at("quitting").maySpawn, false); // closed while starting: nothing more is started
});

test("the wizard's window closing means stop only while installing", () => {
	assert.ok(at("installing").wizardCloseMeansStop);
	// the shell closes the wizard itself after moving on: that must not quit the app
	assert.equal(at("installing", "serving").wizardCloseMeansStop, false);
	assert.equal(at("installing", "failed").wizardCloseMeansStop, false);
	assert.equal(at("installing", "quitting").wizardCloseMeansStop, false);
});

test("a server stopping is a failure to report only once the app is ready", () => {
	assert.ok(at("serving", "ready").childExitIsFailure);
	assert.equal(at("serving").childExitIsFailure, false); // whoever waits for it reports it
	assert.equal(at("serving", "ready", "quitting").childExitIsFailure, false); // it was asked to stop
});

test("closing every window quits the app only when there is a main window to have closed", () => {
	assert.equal(at("installing").quitsWhenWindowsClose, false);
	assert.equal(at("installing", "serving").quitsWhenWindowsClose, false); // the wizard just closed
	assert.ok(at("serving", "ready").quitsWhenWindowsClose);
	assert.ok(at("serving", "failed").quitsWhenWindowsClose);
});
