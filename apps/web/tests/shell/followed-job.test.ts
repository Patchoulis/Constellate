/** A feature's job as the page follows it (lib/shell/use-followed-job.ts): its transitions, and the one poller. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Status } from "@rp/contracts";
import {
	MOST_POLL_MS,
	POLL_MS,
	createPoller,
	elsewhereLine,
	jobStep,
	notAskedLine,
	pollDelay,
	started,
	type FollowedJob,
} from "@/lib/shell/use-followed-job";

const status = (state: Status["state"], more: Partial<Status> = {}): Status => ({
	id: 7,
	kind: "the extraction table",
	state,
	result: null,
	error: null,
	log: [],
	total: 0,
	...more,
});
const idle: FollowedJob = { state: "idle" };

test("a click starts a job once: a second click while it starts or runs changes nothing", () => {
	const starting = jobStep(idle, { kind: "start" });
	assert.deepEqual(starting, { state: "starting" });
	assert.equal(jobStep(starting, { kind: "start" }), starting);
	const running = jobStep(starting, { kind: "started", id: 7 });
	assert.equal(running.state, "running");
	assert.equal(jobStep(running, { kind: "start" }), running);
});

test("a refusal of the second click cannot end the first click's job", () => {
	const running = jobStep(jobStep(idle, { kind: "start" }), { kind: "started", id: 7 });
	// the engine answered 409 to a request that should never have been sent: the job running is still followed
	assert.equal(jobStep(running, { kind: "refused", error: "the extraction table is already in progress" }), running);
});

test("a refused start says why; a start that needed nothing goes back to idle", () => {
	const starting = jobStep(idle, { kind: "start" });
	const refused = jobStep(starting, { kind: "refused", error: "busy" });
	assert.equal(refused.state === "failed" && refused.error, "busy");
	assert.deepEqual(jobStep(starting, { kind: "not_started" }), idle);
	// and a new click after a refusal starts again
	assert.deepEqual(jobStep(refused, { kind: "start" }), { state: "starting" });
});

test("what the job is about is kept from its start to its end", () => {
	const running = jobStep(jobStep(idle, { kind: "start" }), { kind: "started", id: 7, about: "q1" });
	const done = jobStep(running, { kind: "seen", status: status("done") });
	assert.equal(done.state === "done" && done.about, "q1");
});

test("a look that is not answered keeps the job running, and the next answer clears it", () => {
	const running = started(7);
	const unanswered = jobStep(running, { kind: "unanswered", error: "Constellate could not be reached." });
	assert.equal(unanswered.state === "running" && unanswered.unanswered, "Constellate could not be reached.");
	const again = jobStep(unanswered, { kind: "seen", status: status("running") });
	assert.equal(again.state === "running" && again.unanswered, null);
	assert.equal(jobStep(idle, { kind: "unanswered", error: "x" }), idle, "nothing to say it of when nothing runs");
});

test("looks and resets do nothing to a job being started, and following picks up a job under way", () => {
	const starting: FollowedJob = { state: "starting" };
	assert.equal(jobStep(starting, { kind: "seen", status: status("running") }), starting);
	assert.equal(jobStep(starting, { kind: "reset" }), starting);
	assert.equal(jobStep(starting, { kind: "follow", id: 3 }), starting);
	const followed = jobStep(idle, { kind: "follow", id: 3 });
	assert.equal(followed.state === "running" && followed.id, 3);
	assert.equal(jobStep(followed, { kind: "follow", id: 3 }), followed, "the same job again: nothing changes");
	assert.deepEqual(jobStep(followed, { kind: "reset" }), idle);
});

test("a job that ended early says how many were not asked, with their noun", () => {
	const running = started(7);
	const failed = jobStep(jobStep(running, { kind: "seen", status: status("running", { progress: { done: 6, of: 10, usd: 0.2 } }) }), {
		kind: "seen",
		status: status("failed", { error: "budget reached" }),
	});
	assert.equal(notAskedLine(failed, "paper"), "6 of 10 papers were asked; the other 4 were not.");
	const whole = jobStep(running, { kind: "seen", status: status("done", { progress: { done: 3, of: 3, usd: 0 } }) });
	assert.equal(notAskedLine(whole, "paper"), null);
	assert.equal(notAskedLine(running, "paper"), null);
});

test("an engine that does not answer is asked less and less often, up to a bound", () => {
	assert.equal(pollDelay(0), POLL_MS);
	assert.equal(pollDelay(1), 2 * POLL_MS);
	assert.ok(pollDelay(3) > pollDelay(2));
	assert.equal(pollDelay(50), MOST_POLL_MS);
});

/** A poller over a scripted engine and a clock the test turns. */
function scripted(answers: (Status | Error)[]) {
	const asked: number[] = [];
	const waits: { ms: number; run: () => void }[] = [];
	let time = 0;
	const poller = createPoller(
		async (since) => {
			asked.push(since);
			const next = answers.shift();
			if (!next) throw new Error("no more answers");
			if (next instanceof Error) throw next;
			return next;
		},
		() => time,
		(run, ms) => {
			const waiting = { ms, run };
			waits.push(waiting);
			return () => waits.splice(waits.indexOf(waiting), 1);
		},
	);
	const tick = async () => {
		const next = waits.shift();
		if (!next) return null;
		time += next.ms;
		next.run();
		await new Promise((resolve) => setTimeout(resolve, 0));
		return next.ms;
	};
	return { poller, asked, waits, tick };
}

test("one poller answers every watcher alike, asking only for the log lines not seen yet", async () => {
	const { poller, asked, tick } = scripted([status("running", { log: ["a"] }), status("running", { log: ["b"] })]);
	const one: unknown[] = [];
	const two: unknown[] = [];
	poller.watch((look) => one.push(look));
	poller.watch((look) => two.push(look));
	await tick();
	await tick();
	assert.deepEqual(asked, [0, 1], "one request per look, whoever watches; since the lines already seen");
	assert.deepEqual(one, two);
	assert.deepEqual(one.at(-1), { seen: status("running", { log: ["a", "b"] }) });
});

test("nothing is asked once nobody watches", async () => {
	const { poller, waits } = scripted([status("running")]);
	const stop = poller.watch(() => undefined);
	assert.equal(waits.length, 1);
	stop();
	assert.equal(waits.length, 0, "the next look is called off");
});

test("an unreachable engine is looked at after longer and longer waits, and at the usual pace once it answers", async () => {
	const down = new Error("Constellate could not be reached.");
	const { poller, tick } = scripted([down, down, down, status("running")]);
	const looks: unknown[] = [];
	poller.watch((look) => looks.push(look));
	const waited = [await tick(), await tick(), await tick(), await tick(), await tick()];
	assert.deepEqual(waited.slice(0, 4), [POLL_MS, 2 * POLL_MS, 4 * POLL_MS, 8 * POLL_MS]);
	assert.equal(waited[4], POLL_MS);
	assert.deepEqual(looks[0], { unanswered: "Constellate could not be reached." });
});

test("another job since the last look is read from its first log line", async () => {
	const { poller, asked, tick } = scripted([
		status("running", { id: 7, log: ["a", "b"] }),
		status("running", { id: 8, log: ["z"] }),
		status("running", { id: 8, log: ["x", "y", "z"] }),
	]);
	const looks: Status[] = [];
	poller.watch((look) => "seen" in look && looks.push(look.seen));
	await tick();
	await tick();
	assert.deepEqual(asked, [0, 2, 0]);
	assert.deepEqual(looks.at(-1)?.log, ["x", "y", "z"]);
});

test("a probe right after a look answers from it, without asking again", async () => {
	const { poller, asked } = scripted([status("idle")]);
	await poller.probe();
	await poller.probe();
	assert.deepEqual(asked, [0]);
});

test("after a job is started, the last job's end is never told as the new one's, and its log is asked from the start", async () => {
	// a status without job numbers: only the start tells one job from the next
	const ended = { ...status("done", { log: ["old 1", "old 2"] }), id: undefined };
	const fresh = { ...status("running", { log: ["new 1"] }), id: undefined };
	const { poller, asked } = scripted([ended, fresh]);
	assert.deepEqual(await poller.probe(), { seen: ended });
	poller.restart();
	const look = await poller.probe();
	assert.deepEqual(look, { seen: fresh }, "asked again, not the kept look of the job before");
	assert.deepEqual(asked, [0, 0], "from the new job's first line");
});

test("a look under way when a job is started is asked again before anyone is told", async () => {
	let answer: (told: Status) => void = () => {};
	const asked: number[] = [];
	const answers = [status("done", { id: 7 }), status("running", { id: 8, log: ["a"] })];
	let first = true;
	const poller = createPoller(
		(since) => {
			asked.push(since);
			if (first) {
				first = false;
				return new Promise<Status>((resolve) => (answer = resolve));
			}
			return Promise.resolve(answers[1]);
		},
		() => 0,
		() => () => undefined, // no next look: the test is about this one
	);
	const told: unknown[] = [];
	const looking = poller.probe();
	poller.watch((look) => told.push(look));
	poller.restart(); // the job was started while that look was on its way
	answer(answers[0]);
	assert.deepEqual(await looking, { seen: answers[1] });
	assert.deepEqual(told, [{ seen: answers[1] }], "the job before's end was not told");
	assert.deepEqual(asked, [0, 0]);
});

test("a job just started is looked at after the usual wait, whatever the engine's silence before", async () => {
	const down = new Error("The engine is not reachable.");
	const { poller, tick, waits } = scripted([down, down, down, status("running")]);
	const stop = poller.watch(() => undefined);
	await tick();
	await tick();
	await tick();
	assert.equal(waits[0]?.ms, 8 * POLL_MS, "backing off while it did not answer");
	stop();
	poller.restart();
	poller.watch(() => undefined);
	assert.equal(waits.at(-1)?.ms, POLL_MS);
});

test("a job running for another part of the app is named, and nothing new is offered until it ends", () => {
	assert.equal(elsewhereLine({ kind: "a run" }), "Something else is being done (a run): nothing new can be asked until it ends.");
});
