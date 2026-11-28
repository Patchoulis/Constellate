/** A request to the engine (lib/net/request.ts): its deadline, being called off, its errors, and what is sent again. */
import assert from "node:assert/strict";
import { mock, test } from "node:test";
import {
	answered,
	idempotent,
	problemMessage,
	problemOf,
	POLLING,
	QUICK,
	READS_PAPERS,
	RequestError,
	request,
	RETRY_PAUSE_MS,
	STARTS_JOB,
	withSignal,
	currentSignal,
	type RequestProblem,
} from "@/lib/net/request";
import { startLoad, type AsyncEvent } from "@/lib/use-async";

/** A fetch that answers when the test says, heeds the abort, and counts what it was sent. */
function fakeFetch(answer: (attempt: number) => Promise<Response> | "hang") {
	const sent: { path: string; method: string }[] = [];
	const fetcher = ((path: string, init: RequestInit) => {
		sent.push({ path, method: init.method ?? "GET" });
		const told = answer(sent.length);
		return new Promise<Response>((resolve, reject) => {
			init.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
			if (told !== "hang") told.then(resolve, reject);
		});
	}) as typeof fetch;
	return { fetcher, sent };
}
const json = (status: number, body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status }));
const unreachable = () => Promise.reject(new TypeError("Failed to fetch"));

async function problemOfRequest(asking: Promise<unknown>): Promise<RequestProblem> {
	try {
		await asking;
	} catch (error) {
		assert.ok(error instanceof RequestError, `a RequestError, not ${String(error)}`);
		return error.problem;
	}
	throw new Error("the request did not fail");
}

test("a request that gets no answer within its deadline fails with the time it waited, in plain words", async () => {
	mock.timers.enable({ apis: ["setTimeout"] });
	try {
		const { fetcher } = fakeFetch(() => "hang");
		const asking = request("/api/runs", undefined, { patience: QUICK, fetcher });
		mock.timers.tick(QUICK.ms - 1);
		let settled = false;
		void asking.then(
			() => (settled = true),
			() => (settled = true),
		);
		await Promise.resolve();
		assert.equal(settled, false, "still waited for just before the deadline");
		mock.timers.tick(1);
		const error = await asking.catch((error: unknown) => error);
		assert.ok(error instanceof RequestError);
		assert.deepEqual(error.problem, { kind: "timeout", seconds: 20 });
		assert.equal(error.message, "Constellate did not answer within 20 seconds.");
		assert.equal(error.status, null, "no answer came, so no status");
	} finally {
		mock.timers.reset();
	}
});

test("a route that reads papers is given longer than a read, and the poller's request is not retried", () => {
	assert.ok(READS_PAPERS.ms > QUICK.ms);
	assert.ok(READS_PAPERS.ms > 30_000, "past the engine's own 30 seconds, so that its answer comes first");
	assert.ok(READS_PAPERS.ms <= 60_000, "nothing waits for minutes: that is a job");
	assert.equal(POLLING.retry, "never");
	assert.ok(STARTS_JOB.ms > 30_000, "a paid job's start: the engine's answer, or its own 408, comes before ours");
	assert.equal(STARTS_JOB.retry, "never");
});

test("a body that never comes is not waited for past the deadline either", async () => {
	mock.timers.enable({ apis: ["setTimeout"] });
	try {
		const stalled = new Response(new ReadableStream({ start() {} }), { status: 200 });
		const fetcher = (() => Promise.resolve(stalled)) as unknown as typeof fetch;
		const asking = request("/api/runs", undefined, { patience: QUICK, fetcher });
		await Promise.resolve();
		mock.timers.tick(QUICK.ms);
		assert.deepEqual(await problemOfRequest(asking), { kind: "timeout", seconds: 20 });
	} finally {
		mock.timers.reset();
	}
});

test("a request called off by its asker fails as aborted, and is not sent when called off before", async () => {
	const { fetcher, sent } = fakeFetch(() => "hang");
	const asker = new AbortController();
	const asking = request("/api/runs", undefined, { patience: QUICK, fetcher, signal: asker.signal });
	asker.abort();
	assert.deepEqual(await problemOfRequest(asking), { kind: "aborted" });
	assert.equal(sent.length, 1);
	assert.deepEqual(await problemOfRequest(request("/api/runs", undefined, { patience: QUICK, fetcher, signal: asker.signal })), {
		kind: "aborted",
	});
	assert.equal(sent.length, 1, "already called off: nothing sent");
});

test("an error answer carries its status and the engine's own words", async () => {
	const { fetcher } = fakeFetch(() => json(409, { error: "a run is already going" }));
	const answer = await request("/api/runs", { method: "POST" }, { patience: QUICK, fetcher });
	const error = (() => {
		try {
			answered(answer);
		} catch (error) {
			return error;
		}
	})();
	assert.ok(error instanceof RequestError);
	assert.equal(error.status, 409);
	assert.equal(error.message, "a run is already going");
	// the body as it came, for a refusal that says more than its words (a 409 that names the run it would replace)
	const body = { error: "a run is already going" };
	assert.deepEqual(error.problem, { kind: "refused", status: 409, error: "a run is already going", body });
});

test("an error answer with no words of the engine's says its status; the engine's own time limit says so", async () => {
	const { fetcher } = fakeFetch(() => Promise.resolve(new Response("<html>bad gateway</html>", { status: 502 })));
	const answer = await request("/api/runs", undefined, { patience: QUICK, fetcher });
	assert.throws(() => answered(answer), { message: "Constellate answered with error 502." });
	const timedOut = await request("/api/runs", undefined, {
		patience: QUICK,
		fetcher: fakeFetch(() => Promise.resolve(new Response("", { status: 408 }))).fetcher,
	});
	assert.throws(() => answered(timedOut), /took too long/);
});

test("an answer of success that is not JSON, is empty, or is cut off is an error that says so", async () => {
	const htmlAnswer = await request("/api/runs", undefined, {
		patience: QUICK,
		fetcher: fakeFetch(() => Promise.resolve(new Response("<html></html>", { status: 200 }))).fetcher,
	});
	assert.throws(
		() => answered(htmlAnswer),
		(error: RequestError) => error.problem.kind === "unreadable" && /not JSON/.test(error.message),
	);
	const empty = await request("/api/runs", undefined, {
		patience: QUICK,
		fetcher: fakeFetch(() => Promise.resolve(new Response("", { status: 200 }))).fetcher,
	});
	assert.throws(() => answered(empty), { message: "Constellate's answer was empty (status 200)." });
	const half = new Response(
		new ReadableStream({
			start(controller) {
				controller.enqueue(new TextEncoder().encode('{"runs": ['));
				controller.error(new TypeError("connection reset"));
			},
		}),
		{ status: 200 },
	);
	const cut = request("/api/runs", undefined, { patience: QUICK, fetcher: (() => Promise.resolve(half)) as unknown as typeof fetch });
	assert.deepEqual(await problemOfRequest(cut), { kind: "unreadable", status: 200, why: "cut_off" });
});

test("a GET nothing answered is asked once more, and only once", async () => {
	mock.timers.enable({ apis: ["setTimeout"] });
	try {
		const { fetcher, sent } = fakeFetch((attempt) => (attempt === 1 ? unreachable() : json(200, [])));
		const asking = request("/api/runs", undefined, { patience: QUICK, fetcher });
		for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
		mock.timers.tick(RETRY_PAUSE_MS);
		assert.deepEqual(answered(await asking), []);
		assert.equal(sent.length, 2);

		const down = fakeFetch(() => unreachable());
		const failing = request("/api/runs", undefined, { patience: QUICK, fetcher: down.fetcher });
		for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
		mock.timers.tick(RETRY_PAUSE_MS);
		const problem = await problemOfRequest(failing);
		assert.deepEqual(problem, { kind: "unreachable" });
		assert.equal(down.sent.length, 2, "asked twice, not more");
	} finally {
		mock.timers.reset();
	}
});

test("a request that could cost money or change something is never sent twice", async () => {
	for (const method of ["POST", "PUT", "PATCH", "DELETE"]) {
		const { fetcher, sent } = fakeFetch(() => unreachable());
		const problem = await problemOfRequest(request("/api/papers/p/critique", { method }, { patience: READS_PAPERS, fetcher }));
		assert.deepEqual(problem, { kind: "unreachable" });
		assert.equal(sent.length, 1, `${method} sent once`);
	}
});

test("the poller's look and a GET that got an answer are not asked again by the request itself", async () => {
	const polled = fakeFetch(() => unreachable());
	await problemOfRequest(request("/api/status?since=0", undefined, { patience: POLLING, fetcher: polled.fetcher }));
	assert.equal(polled.sent.length, 1, "the poller's backoff is its retry");
	const refused = fakeFetch(() => json(500, { error: "the engine failed unexpectedly" }));
	const answer = await request("/api/runs", undefined, { patience: QUICK, fetcher: refused.fetcher });
	assert.equal(answer.status, 500);
	assert.equal(refused.sent.length, 1, "an answer came: not asked again");
});

test("the signal set by withSignal is the one a request started inside it reads, and no other", () => {
	const controller = new AbortController();
	assert.equal(currentSignal(), null);
	assert.equal(
		withSignal(controller.signal, () => currentSignal()),
		controller.signal,
	);
	assert.equal(currentSignal(), null, "put back after");
	assert.equal(
		withSignal(controller.signal, () => withSignal(null, () => currentSignal())),
		null,
		"a shared request is called off by nobody",
	);
});

test("a load superseded by the next one has its request called off and its answer dropped", async () => {
	const events: AsyncEvent<string>[] = [];
	const signals: AbortSignal[] = [];
	const ambient: (AbortSignal | null)[] = [];
	let answerFirst: (paper: string) => void = () => {};
	const stopFirst = startLoad(
		(signal) => {
			signals.push(signal);
			ambient.push(currentSignal());
			return new Promise<string>((resolve) => (answerFirst = resolve));
		},
		(event) => events.push(event),
	);
	assert.equal(ambient[0], signals[0], "the api requests the load starts get its signal");
	stopFirst(); // other deps, `reload`, or the component going away
	assert.equal(signals[0].aborted, true, "the first request is called off");
	answerFirst("paper A, late");
	const stopSecond = startLoad(
		() => Promise.resolve("paper B"),
		(event) => events.push(event),
	);
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(events, [{ kind: "answer", data: "paper B" }], "only the answer of the request still wanted");
	stopSecond();
});

test("a failed load keeps the request's problem beside its words", async () => {
	const events: AsyncEvent<string>[] = [];
	startLoad(
		() => Promise.reject(new RequestError({ kind: "refused", status: 404, error: "not found" })),
		(event) => events.push(event),
	);
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(events, [{ kind: "refuse", error: "not found", problem: { kind: "refused", status: 404, error: "not found" } }]);
});

test("every problem is said in words a person reads, with its seconds or its status", () => {
	assert.equal(
		problemMessage({ kind: "unreachable" }),
		"Constellate could not be reached: it may have stopped. Close it and open it again.",
	);
	assert.equal(problemMessage({ kind: "timeout", seconds: 60 }), "Constellate did not answer within 60 seconds.");
	assert.equal(problemMessage({ kind: "aborted" }), "The request was called off.");
	assert.equal(
		problemMessage({ kind: "refused", status: 408, error: null }),
		"Constellate stopped working on this request: it took too long.",
	);
	// the engine's own words win over the status's
	assert.equal(
		problemMessage({ kind: "refused", status: 408, error: "the request took too long to arrive" }),
		"the request took too long to arrive",
	);
	assert.equal(problemMessage({ kind: "refused", status: 500, error: null }), "Constellate answered with error 500.");
	assert.equal(
		problemMessage({ kind: "unreadable", status: 200, why: "cut_off" }),
		"Constellate's answer was cut off before its end (status 200).",
	);
	assert.equal(
		problemMessage({ kind: "unreadable", status: 200, why: "not_json" }),
		"Constellate's answer could not be read: it is not JSON (status 200).",
	);
});

test("a request's problem is read back from what was thrown, and nothing else is taken for one", () => {
	const problem: RequestProblem = { kind: "refused", status: 409, error: "busy" };
	const error = new RequestError(problem);
	assert.deepEqual(problemOf(error), problem);
	assert.equal(error.status, 409);
	assert.equal(error.name, "RequestError");
	assert.equal(new RequestError({ kind: "timeout", seconds: 20 }).status, null, "no status when nothing came");
	assert.equal(problemOf(new Error("busy")), null);
	assert.equal(problemOf("busy"), null);
	assert.equal(problemOf(undefined), null);
});

test("only reading methods may be sent twice, whatever their case, and no method means GET", () => {
	for (const method of ["GET", "get", "HEAD", "head", undefined]) assert.equal(idempotent(method), true, String(method));
	for (const method of ["POST", "PUT", "DELETE", "PATCH", "post", "OPTIONS"]) assert.equal(idempotent(method), false, method);
});
