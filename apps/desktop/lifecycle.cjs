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

module.exports = { Lifecycle, TRANSITIONS };
