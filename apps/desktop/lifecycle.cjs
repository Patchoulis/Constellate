/**
 * Where the desktop app is between being opened and being closed, as one explicit state machine.
 *
 *   starting ──▶ installing ──▶ serving ──▶ ready ──▶ quitting
 *       │             │            │          │
 *       └─────────────┴────────────┴──────────┴──▶ failed ──▶ quitting
 *   (and from any state but quitting: ──▶ quitting, when the user closes the app)
 *
 *   starting     opened; nothing decided yet
 *   installing   the installed app's own Python is being set up (the Installation Wizard window is showing)
 *   serving      the backend and the interface's server are being started
 *   ready        the main window is open
 *   failed       something above went wrong; an error is shown, then the app quits
 *   quitting     everything is being stopped; nothing new may start
 *
 * What may happen depends on the state alone, and the questions the rest of the shell asks are answered here:
 * may a process be started, does closing the wizard's window mean "stop", is an error worth showing.
 * Nothing in this file needs Electron.
 */
const TRANSITIONS = {
	starting: ["installing", "serving", "failed", "quitting"],
	installing: ["serving", "failed", "quitting"],
	serving: ["ready", "failed", "quitting"],
	ready: ["failed", "quitting"],
	failed: ["quitting"],
	quitting: [],
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
		return this.is("starting", "installing", "serving");
	}

	/** Does the wizard's window closing mean the user wants to stop? Only while it is installing: when the
	 *  shell closes that window itself, it has already moved on. */
	get wizardCloseMeansStop() {
		return this.is("installing");
	}

	/** Is a child process ending now a failure? While starting up it is reported by whoever is waiting for it;
	 *  once ready, nobody is waiting, so it must be reported here. While quitting it is what was asked for. */
	get childExitIsFailure() {
		return this.is("ready");
	}

	/** Should all windows being closed quit the app? Not while starting up: the wizard's window comes and goes. */
	get quitsWhenWindowsClose() {
		return this.is("ready", "failed");
	}
}

/**
 * The arguments that run the Node script `script` (an ES module or not) so that it stops when this shell does.
 *
 * Every way out of the shell that it sees coming stops its children (before-quit). One it does not see (the shell
 * ended from the Task Manager, or crashed) leaves them running: the backend watches its standard input for that
 * (RP_EXIT_WITH_PARENT), and a script run this way does the same. Its standard input is a pipe from the shell,
 * which the system closes when the shell is gone, however it went. Without this the interface's server stayed
 * behind, holding the port the next launch wanted.
 */
function withParent(script) {
	const watch = "process.stdin.on('end',()=>process.exit(0)).resume();import(require('node:url').pathToFileURL(process.argv[1]).href)";
	return ["-e", watch, script];
}

module.exports = { Lifecycle, TRANSITIONS, withParent };
