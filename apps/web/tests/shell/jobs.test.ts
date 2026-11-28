/** Following the engine's jobs: which finished job opens a run (lib/shell/use-job.ts), and a feature's job followed to
 *  its end (lib/shell/use-followed-job.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Status } from "@rp/contracts";
import { type Job, finishedRun, isFeatureJob, madeRun, reduce } from "@/lib/shell/use-job";
import { claims, runSubject, started, step } from "@/lib/shell/use-followed-job";

const status = (state: Status["state"], more: Partial<Status> = {}): Status => ({
	id: 7,
	kind: "a run",
	state,
	result: null,
	error: null,
	log: [],
	total: 0,
	...more,
});
const idle: Job = { state: "idle", log: [], error: null };
const running: Job = { state: "running", log: [], error: null };

test("a run is opened only when this page saw its job go from running to done", () => {
	const made = status("done", { result: "neurips2025", outcome: { kind: "run", run: "neurips2025" } });
	assert.equal(finishedRun(running, made), "neurips2025");
	assert.equal(finishedRun(idle, made), null, "finished before the page asked: the last run is not opened again");
	// the page opens, sees the run under way, then sees it finish
	const seen = reduce(idle, { type: "reported", status: status("running") });
	assert.equal(finishedRun(seen, made), "neurips2025");
});

test("a run is said to be made only by a job seen making it, not by naming its communities again", () => {
	const made = status("done", { outcome: { kind: "run", run: "neurips2025" } });
	const embedding = { step: "embed" as const, title: "Embed", number: 3, of: 10, count: null, detail: null };
	// started here, seen at a step of the run, then done
	const making = reduce(reduce(idle, { type: "started" }), { type: "reported", status: status("running", { step: embedding }) });
	assert.equal(making.making, true);
	// a step that no longer says where it is does not undo it
	const later = reduce(making, { type: "reported", status: status("running", { step: null }) });
	assert.equal(madeRun(later, made), true);
	// naming again (or reading declarations): running, never at a step of the run
	const naming = reduce(reduce(idle, { type: "started" }), { type: "reported", status: status("running", { kind: "naming" }) });
	assert.equal(madeRun(naming, made), false);
	assert.equal(finishedRun(naming, made), "neurips2025", "it still opens the run");
	// a job that had finished before this page looked made nothing this page saw
	assert.equal(madeRun(idle, made), false);
	// the next job starts knowing nothing of the last one
	const done = reduce(later, { type: "reported", status: made });
	assert.equal(done.making, true);
	assert.equal(reduce(done, { type: "started" }).making, false);
	assert.equal(reduce(done, { type: "reported", status: status("running") }).making, false, "another window's job, not seen making a run");
	// a feature's job opens nothing, so makes nothing
	assert.equal(madeRun(making, status("done", { outcome: { kind: "nothing" } })), false);
});

test("a feature's finished job, or one that made nothing, opens no run", () => {
	const critique = status("done", {
		kind: "a paper's weaknesses",
		outcome: { kind: "feature", feature: "critique", subject: { paper: "2601.00001" } },
	});
	assert.equal(finishedRun(running, critique), null);
	assert.equal(finishedRun(running, status("done", { outcome: { kind: "nothing" } })), null);
	assert.equal(finishedRun(running, status("failed", { error: "no" })), null);
	assert.equal(finishedRun(running, status("done", { outcome: { kind: "run", run: "iclr2026" } })), "iclr2026");
});

test("the run's log is the whole log the shared poller saw, not lines added twice", () => {
	const first = reduce(running, { type: "reported", status: status("running", { log: ["reading", "embedding"] }) });
	const again = reduce(first, { type: "reported", status: status("running", { log: ["reading", "embedding", "clustering"] }) });
	assert.deepEqual(again.log, ["reading", "embedding", "clustering"]);
	assert.deepEqual(reduce(again, { type: "started" }).log, [], "a new job starts with an empty log");
});

test("a feature's failure is not said as the run's, a run's is", () => {
	const extraction = status("failed", {
		kind: "the extraction table",
		error: "the budget was reached",
		feature: "extraction",
		subject: { run: "r" },
	});
	assert.equal(reduce(idle, { type: "reported", status: extraction }).error, null);
	assert.equal(reduce(running, { type: "reported", status: extraction }).error, null);
	assert.equal(reduce(running, { type: "reported", status: status("failed", { error: "only 3 papers" }) }).error, "only 3 papers");
	assert.ok(isFeatureJob({ outcome: { kind: "feature", feature: "critique", subject: { paper: "p" } } }));
	assert.ok(isFeatureJob({ feature: "stances", outcome: null }), "a feature's job is told by its feature from its start");
	assert.ok(!isFeatureJob({ feature: null, outcome: null }));
});

test("a section follows its own job whichever window started it, and only its own", () => {
	const asking = status("running", { feature: "answers", subject: { community: { run: "r", community: "c1" } }, item: "q1" });
	assert.ok(claims({ feature: "answers", subject: runSubject("r", "c1"), item: "some" }, asking));
	assert.ok(!claims({ feature: "answers", subject: runSubject("r", "c2"), item: "some" }, asking), "another community's");
	assert.ok(!claims({ feature: "answers", subject: runSubject("r"), item: "some" }, asking), "the run's own questions");
	assert.ok(!claims({ feature: "stances", subject: runSubject("r", "c1") }, asking), "another feature's");
	const proposing = status("running", { feature: "stances", subject: { community: { run: "r", community: "c1" } }, item: null });
	assert.ok(claims({ feature: "stances", subject: runSubject("r", "c1"), item: "none" }, proposing));
	assert.ok(!claims({ feature: "stances", subject: runSubject("r", "c1"), item: "some" }, proposing), "proposing is not asking");
	assert.ok(claims({ feature: "critique", subject: { paper: "p" } }, status("running", { feature: "critique", subject: { paper: "p" } })));
	assert.ok(!claims({ feature: "critique", subject: { paper: "p" } }, status("running")), "a run claims no section");
});

test("a followed job keeps the job's log and how far it got, and ends with what it made or why it failed", () => {
	const job = started(7);
	assert.equal(job.state, "running");
	if (job.state !== "running") return;
	const progress = { done: 1, of: 3, usd: 0 };
	const on = step(job, status("running", { log: ["one"], progress, papers: { done: 2, of: 9 } }));
	assert.deepEqual(on.state === "running" && [on.log, on.progress, on.papers], [["one"], progress, { done: 2, of: 9 }]);
	if (on.state !== "running") return;
	const outcome = { kind: "feature" as const, feature: "explanation" as const, subject: { paper: "p" } };
	assert.deepEqual(step(on, status("done", { log: ["one", "two"], outcome })), {
		state: "done",
		id: 7,
		about: null,
		outcome,
		progress,
		log: ["one", "two"],
	});
	assert.deepEqual(step(on, status("failed", { error: "budget", log: ["one"] })), {
		state: "failed",
		id: 7,
		about: null,
		error: "budget",
		progress,
		log: ["one"],
	});
	// another job has started since: ours is over, with nothing known of what it made
	assert.deepEqual(step(on, status("running", { id: 8 })), { state: "done", id: 7, about: null, outcome: null, progress, log: ["one"] });
});
