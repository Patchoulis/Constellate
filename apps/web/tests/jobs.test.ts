/** Following the engine's jobs: which finished job opens a run (lib/use-job.ts), and a feature's job followed to
 *  its end (lib/use-followed-job.ts). */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Status } from "@rp/contracts";
import { type Job, finishedRun, isFeatureJob, reduce } from "@/lib/use-job";
import { started, step } from "@/lib/use-followed-job";

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

test("a feature's failure is not said as the run's, a run's is", () => {
	const extraction = status("failed", { kind: "the extraction table", error: "the budget was reached" });
	assert.equal(reduce(idle, { type: "reported", status: extraction }).error, null);
	assert.equal(reduce(running, { type: "reported", status: extraction }).error, null);
	assert.equal(reduce(running, { type: "reported", status: status("failed", { error: "only 3 papers" }) }).error, "only 3 papers");
	assert.ok(isFeatureJob({ kind: "a run", outcome: { kind: "feature", feature: "critique", subject: { paper: "p" } } }));
	assert.ok(!isFeatureJob({ kind: "a run", outcome: null }));
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
