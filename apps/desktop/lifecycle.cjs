/**
 * Where the desktop app is between being opened and being closed, as explicit states.
 *
 *   starting ──▶ serving ──▶ ready ──▶ quitting ──▶ stopped
 *       │           │          │          ▲
 *       └───────────┴──────────┴─▶ failed ┘
 *   (and from starting, serving and ready: ──▶ quitting, when the user closes the app)
 *
 *   starting   opened; nothing decided yet
 *   serving    the engine and the interface's server are being started
 *   ready      the main window is open
 *   failed     something above went wrong; an error is shown, then the app quits
 *   quitting   the children are being stopped (asked first, by closing their standard input; then ended);
 *              nothing new may start
 *   stopped    every child has ended: the app itself may now end
 *
 * What may happen depends on the state alone, and the questions the rest of the shell asks are answered here:
 * may a process be started, is a child ending a failure to report, may the app end yet.
 * Nothing in this file needs Electron.
 */
const TRANSITIONS = {
	starting: ["serving", "failed", "quitting"],
	serving: ["ready", "failed", "quitting"],
	ready: ["failed", "quitting"],
	failed: ["quitting"],
	quitting: ["stopped"],
	stopped: [],
};

class Lifecycle {
	constructor() {
		this.state = "starting";
	}

	/** Go to `next` if that is a legal step from here; says whether it happened. An illegal step changes nothing. */
	move(next) {
		if (!TRANSITIONS[this.state].includes(next)) return false;
		this.state = next;
		return true;
	}

	is(...states) {
		return states.includes(this.state);
	}

	/** May a child process be started now? Not once the app is failing or quitting: nobody would stop it. */
	get maySpawn() {
		return this.is("starting", "serving");
	}

	/** Is a child process ending now a failure? While starting up it is reported by whoever is waiting for it;
	 *  once ready, nobody is waiting, so it must be reported here. While quitting it is what was asked for. */
	get childExitIsFailure() {
		return this.is("ready");
	}

	/** Should all windows being closed quit the app? Only once there was a main window (or an error) to close. */
	get quitsWhenWindowsClose() {
		return this.is("ready", "failed");
	}

	/** May the app's own process end now? Only once its children have been stopped: until then quitting waits. */
	get mayEnd() {
		return this.is("stopped");
	}
}

/**
 * The arguments that run the Node script `script` (an ES module or not) so that it stops when this shell does.
 *
 * Every way out of the shell that it sees coming stops its children (before-quit). One it does not see (the shell
 * ended from the Task Manager, or crashed) leaves them running unless they watch for it: the engine stops when its
 * standard input closes, and a script run this way does the same. Its standard input is a pipe from the shell,
 * which the system closes when the shell is gone, however it went. Without this the interface's server stayed
 * behind, holding the port the next launch wanted.
 */
function withParent(script) {
	const watch = "process.stdin.on('end',()=>process.exit(0)).resume();import(require('node:url').pathToFileURL(process.argv[1]).href)";
	return ["-e", watch, script];
}

module.exports = { Lifecycle, TRANSITIONS, withParent };
