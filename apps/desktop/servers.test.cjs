/**
 * The app's two servers, started as the shell starts them (engine.cjs).    node --test
 *
 *   - The interface's server forwards /api and /auth to the engine with the token, which it adds itself; the
 *     browser never receives it: not in a page, a header, a script or an error. Against a stand-in engine that
 *     answers only with the token.
 *   - The real engine and the interface's server listen on this computer only (Windows: the system's own table
 *     of listening sockets, read with PowerShell; elsewhere that part is skipped, and says so).
 *
 * They need the interface built (`node scripts/build.mjs --interface`, which `npm run pr` does) and, for the real
 * engine, its program (target/release, or target/debug as `cargo test` leaves it). Missing, a test is skipped,
 * except in CI, where a missing build fails it instead: a check that did not run has not passed.
 */
const assert = require("node:assert/strict");
const { spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");
const { EngineLog, PROVIDER_VARIABLES, engineEnvironment, interfaceEnvironment, parseStartupLine, startEngine } = require("./engine.cjs");
const { engineFilesIn, missingFrom, stage } = require("./engine-files.cjs");
const { withParent } = require("./lifecycle.cjs");

const REPO = path.join(__dirname, "..", "..");
const SERVER = path.join(REPO, "apps", "web", ".next", "standalone", "apps", "web", "server.js");
const PROGRAM = process.platform === "win32" ? "constellate.exe" : "constellate";
const ENGINE = ["release", "debug"].map((profile) => path.join(REPO, "target", profile, PROGRAM)).find((file) => fs.existsSync(file));
// a made-up token of the engine's shape, put together here (64 hex characters)
const TOKEN = "5ec7e7".repeat(10) + "abcd";

/** Skip unless `ready`; in CI, fail instead. */
function needs(ready, what) {
	if (ready) return false;
	if (process.env.CI) throw new Error(`${what} is missing, and CI must not skip this test`);
	return `${what} is missing`;
}

function freePort() {
	return new Promise((resolve) => {
		const server = net.createServer().listen(0, "127.0.0.1", () => {
			const { port } = server.address();
			server.close(() => resolve(port));
		});
	});
}

/** Start the interface's server as the shell does, against the engine at `engine`; resolves once it answers. */
async function startInterface(engine, token) {
	const port = await freePort();
	const child = spawn(process.execPath, withParent(SERVER), {
		cwd: path.dirname(SERVER),
		env: interfaceEnvironment(process.env, { port, engine, token }),
		stdio: ["pipe", "pipe", "pipe"],
	});
	let output = "";
	child.stdout.on("data", (chunk) => (output += chunk));
	child.stderr.on("data", (chunk) => (output += chunk));
	const url = `http://127.0.0.1:${port}`;
	for (let tries = 0; tries < 300; tries++) {
		const up = await fetch(`${url}/icon.svg`).then(
			() => true,
			() => false,
		);
		if (up) return { child, url, port, output: () => output };
		if (child.exitCode !== null) break;
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
	child.kill();
	throw new Error(`the interface's server did not start:\n${output}`);
}

async function stop(child) {
	if (child.exitCode !== null) return;
	const gone = new Promise((resolve) => child.once("exit", resolve));
	child.stdin.end();
	const late = setTimeout(() => child.kill(), 5000);
	await gone;
	clearTimeout(late);
}

/** A stand-in for the engine: what it heard, answered only with the token, as the engine answers. */
function standIn() {
	const heard = [];
	const server = http.createServer((request, response) => {
		heard.push({ url: request.url, method: request.method, headers: request.headers });
		if (request.headers.authorization !== `Bearer ${TOKEN}`) {
			response.writeHead(401, { "content-type": "application/json" });
			return response.end('{"error":"not allowed"}');
		}
		if (request.url.endsWith("/pdf")) {
			const pdf = Buffer.from("%PDF-1.4 a stand-in for a paper's PDF %%EOF");
			const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? "");
			if (range) {
				const [start, end] = [Number(range[1]), Number(range[2])];
				response.writeHead(206, { "content-type": "application/pdf", "content-range": `bytes ${start}-${end}/${pdf.length}` });
				return response.end(pdf.subarray(start, end + 1));
			}
			response.writeHead(200, { "content-type": "application/pdf", "accept-ranges": "bytes" });
			return response.end(pdf);
		}
		const chunks = [];
		request.on("data", (chunk) => chunks.push(chunk));
		request.on("end", () => {
			response.writeHead(200, { "content-type": "application/json" });
			response.end(JSON.stringify({ state: "idle", echoed: Buffer.concat(chunks).toString() }));
		});
	});
	return new Promise((resolve) =>
		server.listen(0, "127.0.0.1", () => resolve({ server, heard, url: `http://127.0.0.1:${server.address().port}` })),
	);
}

/** Everything a browser would receive for `response`: status line, headers and body, as text. */
async function received(response) {
	const headers = [...response.headers].map(([name, value]) => `${name}: ${value}`).join("\n");
	return `${response.status}\n${headers}\n\n${await response.text()}`;
}

test("the interface's server forwards /api with the token it adds itself, and the browser never receives the token", async (t) => {
	const skip = needs(fs.existsSync(SERVER), "the interface's build (node scripts/build.mjs --interface)");
	if (skip) return t.skip(skip);
	const engine = await standIn();
	const web = await startInterface(engine.url, TOKEN);
	try {
		const origin = web.url;
		// a request of the window's own page
		const status = await fetch(`${web.url}/api/status?since=0`, { headers: { origin, "sec-fetch-site": "same-origin" } });
		assert.equal(status.status, 200);
		const seen = engine.heard.at(-1);
		assert.equal(seen.url, "/api/status?since=0");
		assert.equal(seen.headers.authorization, `Bearer ${TOKEN}`);
		assert.equal(seen.headers.host, new URL(engine.url).host, "Host is the engine's own address");
		assert.equal(seen.headers.origin, origin, "the page's Origin goes along, for the engine to check");
		assert.equal(seen.headers["x-forwarded-host"], new URL(web.url).host, "the name the browser used goes along too");
		const answers = [await received(status)];

		// a body is forwarded, and a token the page sends is not: the engine's is the only one
		const posted = await fetch(`${web.url}/api/runs`, {
			method: "POST",
			headers: { origin, "content-type": "application/json", authorization: "Bearer from-a-page" },
			body: JSON.stringify({ selection: "a stand-in" }),
		});
		assert.equal(posted.status, 200);
		assert.equal(engine.heard.at(-1).headers.authorization, `Bearer ${TOKEN}`);
		assert.match(JSON.parse(await posted.clone().text()).echoed, /a stand-in/);
		answers.push(await received(posted));

		// a PDF for the reader's frame, by byte ranges
		const part = await fetch(`${web.url}/api/papers/2501.00001/pdf`, { headers: { range: "bytes=0-7" } });
		assert.equal(part.status, 206);
		assert.equal(await part.clone().text(), "%PDF-1.4");
		assert.equal(engine.heard.at(-1).headers.range, "bytes=0-7");
		answers.push(await received(part));

		// /auth too (the sign-in of a server), as the engine's own /auth/
		answers.push(await received(await fetch(`${web.url}/auth`)));
		assert.equal(engine.heard.at(-1).url, "/auth/");

		// another site's page in a browser on this computer reaches nothing through the interface's server
		const before = engine.heard.length;
		const foreign = await fetch(`${web.url}/api/status`, { headers: { "sec-fetch-site": "cross-site", "sec-fetch-mode": "no-cors" } });
		assert.equal(foreign.status, 403);
		assert.equal(engine.heard.length, before, "not forwarded");
		answers.push(await received(foreign));
		// nor does a request addressed to another name (DNS rebinding)
		const rebound = await new Promise((resolve) => {
			http.get(`${web.url}/api/status`, { headers: { host: `attacker.example:${web.port}` } }, (response) => {
				response.resume();
				resolve(response.statusCode);
			});
		});
		assert.equal(rebound, 403);
		assert.equal(engine.heard.length, before);

		// the stand-in refuses a request without the token, as the engine does: the token is what admits it
		assert.equal((await fetch(`${engine.url}/api/status`)).status, 401);

		// the page, and every script it loads
		const page = await fetch(`${web.url}/`);
		const html = await page.clone().text();
		answers.push(await received(page));
		const scripts = [...html.matchAll(/(?:src|href)="(\/_next\/static\/[^"]+)"/g)].map((match) => match[1]);
		assert.ok(
			scripts.some((file) => file.endsWith(".js")),
			"the page loads scripts",
		);
		for (const file of new Set(scripts)) answers.push(await received(await fetch(`${web.url}${file}`)));
		// every script the build made, loaded by this page or not
		const chunks = path.join(path.dirname(SERVER), ".next", "static");
		for (const file of fs.readdirSync(chunks, { recursive: true }).filter((name) => String(name).endsWith(".js"))) {
			answers.push(fs.readFileSync(path.join(chunks, String(file)), "utf8"));
		}

		// an error: the engine gone, the interface's server answers what it answers
		await new Promise((resolve) => engine.server.close(resolve));
		engine.server.closeAllConnections?.();
		const failed = await fetch(`${web.url}/api/status`, { headers: { origin } });
		assert.ok(failed.status >= 500, `a missing engine is a failure (${failed.status})`);
		answers.push(await received(failed));

		for (const answer of answers) assert.equal(answer.includes(TOKEN), false, `the token reached the browser:\n${answer.slice(0, 400)}`);
		assert.equal(web.output().includes(TOKEN), false, "the interface's server printed the token");
	} finally {
		await stop(web.child);
		engine.server.close();
	}
});

test("the shell's environments: the engine gets keys and its fixed way of listening, the interface neither", () => {
	const user = {
		PATH: "/usr/bin",
		HOME: "/home/someone",
		ANTHROPIC_API_KEY: "a-key",
		OPENAI_BASE_URL: "https://gateway.example/v1",
		HF_ENDPOINT: "https://mirror.example",
		AWS_SECRET_ACCESS_KEY: "not for the engine",
		CONSTELLATE_BIND: "0.0.0.0",
		CONSTELLATE_TOKEN: "chosen-elsewhere",
		NODE_OPTIONS: "--require something",
		ELECTRON_RUN_AS_NODE: "1",
	};
	const engine = engineEnvironment(user, { data: "/data", origin: "http://127.0.0.1:4000", platform: "linux" });
	assert.equal(engine.ANTHROPIC_API_KEY, "a-key");
	assert.equal(engine.OPENAI_BASE_URL, "https://gateway.example/v1");
	assert.equal(engine.HF_ENDPOINT, "https://mirror.example");
	assert.equal(engine.CONSTELLATE_DATA, "/data");
	assert.equal(engine.CONSTELLATE_ORIGIN, "http://127.0.0.1:4000");
	assert.equal(engine.CONSTELLATE_BIND, "127.0.0.1", "this computer only, whatever was set");
	assert.equal(engine.CONSTELLATE_TOKEN, "", "the engine makes its own token and reports it");
	for (const name of ["AWS_SECRET_ACCESS_KEY", "NODE_OPTIONS", "ELECTRON_RUN_AS_NODE"]) assert.equal(name in engine, false, name);

	const web = interfaceEnvironment(user, { port: 4000, engine: "http://127.0.0.1:5000", token: TOKEN, platform: "linux" });
	assert.equal(web.CONSTELLATE_ENGINE_TOKEN, TOKEN);
	assert.equal(web.PORT, "4000");
	for (const name of [...PROVIDER_VARIABLES, "AWS_SECRET_ACCESS_KEY", "NODE_OPTIONS", "HF_ENDPOINT", "CONSTELLATE_TOKEN"]) {
		assert.equal(name in web, false, name);
	}
	assert.ok(!Object.keys(web).some((name) => name.startsWith("NEXT_PUBLIC_")), "nothing the build could hand a page");

	// on Windows, a variable is found whatever its case
	const windows = engineEnvironment({ Path: "C:\\Windows", SYSTEMROOT: "C:\\Windows" }, { data: "C:\\d", origin: "x", platform: "win32" });
	assert.equal(windows.PATH, "C:\\Windows");
	assert.equal(windows.SystemRoot, "C:\\Windows");
});

test("the provider variables passed to the engine are the ones .env.example lists for the language model", () => {
	const example = fs.readFileSync(path.join(REPO, ".env.example"), "utf8");
	const section = example.split("# ----------------------------------------------------------------")[1];
	const listed = [...section.matchAll(/^([A-Z0-9_]+)=/gm)].map((match) => match[1]);
	assert.deepEqual([...listed].sort(), [...PROVIDER_VARIABLES].sort());
});

test("the startup line is taken only as the engine writes it in local mode", () => {
	const line = JSON.stringify({ listening: "http://127.0.0.1:52817", scheme: "http", address: "127.0.0.1", port: 52817, token: TOKEN });
	assert.deepEqual(parseStartupLine(line), { url: "http://127.0.0.1:52817", token: TOKEN });
	for (const bad of [
		"not json",
		JSON.stringify({ scheme: "https", address: "127.0.0.1", port: 1, token: TOKEN }),
		JSON.stringify({ scheme: "http", address: "0.0.0.0", port: 1, token: TOKEN }),
		JSON.stringify({ scheme: "http", address: "127.0.0.1", port: 52817 }),
		JSON.stringify({ scheme: "http", address: "127.0.0.1", port: "1", token: TOKEN }),
	]) {
		assert.throws(() => parseStartupLine(bad), undefined, bad);
	}
});

test("the engine's log is bounded and never holds the token", () => {
	const folder = fs.mkdtempSync(path.join(os.tmpdir(), "rp-log-"));
	const log = new EngineLog(folder, { limit: 1000 });
	log.hide(TOKEN);
	for (let line = 0; line < 300; line++) log.write(`line ${line} ${line === 50 ? TOKEN : ""}\n`);
	const files = fs.readdirSync(folder).sort();
	assert.deepEqual(files, ["engine.1.log", "engine.log"]);
	for (const file of files) {
		const text = fs.readFileSync(path.join(folder, file), "utf8");
		assert.ok(Buffer.byteLength(text) <= 1000);
		assert.equal(text.includes(TOKEN), false);
	}
	assert.match(fs.readFileSync(path.join(folder, "engine.log"), "utf8"), /line 299/);
	fs.rmSync(folder, { recursive: true, force: true });
});

test("a startup line that never comes is a clear failure, and the engine is stopped", async () => {
	const folder = fs.mkdtempSync(path.join(os.tmpdir(), "rp-silent-"));
	// a program that says nothing: Node with no script, which waits for one on its standard input for ever
	let child;
	const started = startEngine({
		binary: process.execPath,
		data: folder,
		origin: "http://127.0.0.1:9",
		log: new EngineLog(folder),
		deadline: 1500,
		admit: (spawned) => ((child = spawned), true),
	});
	await assert.rejects(started, /said nothing for 1.5 seconds/);
	await new Promise((resolve) => (child.exitCode !== null || child.signalCode !== null ? resolve() : child.once("exit", resolve)));
	fs.rmSync(folder, { recursive: true, force: true });
});

test("the engine is staged with what ONNX Runtime loads beside it", async (t) => {
	const build = path.join(REPO, "target", "release");
	const skip = needs(
		fs.existsSync(path.join(build, PROGRAM)) && missingFrom(build).length === 0,
		"the engine's release build (npm run build)",
	);
	if (skip) return t.skip(skip);
	const to = fs.mkdtempSync(path.join(os.tmpdir(), "rp-stage-"));
	const names = stage(build, to);
	assert.deepEqual(missingFrom(to), []);
	assert.deepEqual(names, engineFilesIn(build));
	if (process.platform === "win32") assert.ok(names.includes("DirectML.dll"));
	if (process.platform === "linux") assert.ok(names.includes("libwebgpu_dawn.so"));
	fs.rmSync(to, { recursive: true, force: true });
});

/** The addresses the processes `pids` listen on (Windows only; null elsewhere). */
function listening(pids) {
	if (process.platform !== "win32") return null;
	const script =
		`$owners = @(${pids.join(",")}); ` +
		"Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $owners -contains $_.OwningProcess } | " +
		'ForEach-Object { $_.LocalAddress + " " + $_.LocalPort }';
	const listed = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8" });
	return listed.stdout.split(/\r?\n/).filter(Boolean);
}

test("the real engine and the interface's server answer through each other and listen on this computer only", async (t) => {
	const skip =
		needs(fs.existsSync(SERVER), "the interface's build (node scripts/build.mjs --interface)") ||
		needs(ENGINE, "the engine's program (cargo build, or cargo test)");
	if (skip) return t.skip(skip);
	const data = fs.mkdtempSync(path.join(os.tmpdir(), "rp-servers-"));
	const port = await freePort();
	const origin = `http://127.0.0.1:${port}`;
	const log = new EngineLog(path.join(data, "logs"));
	const engine = await startEngine({ binary: ENGINE, data, origin, log });
	let web;
	try {
		assert.equal((await fetch(`${engine.url}/api/status`)).status, 401, "the engine refuses a request without its token");
		const child = spawn(process.execPath, withParent(SERVER), {
			cwd: path.dirname(SERVER),
			env: interfaceEnvironment(process.env, { port, engine: engine.url, token: engine.token }),
			stdio: ["pipe", "pipe", "pipe"],
		});
		web = { child };
		for (let tries = 0; tries < 300; tries++) {
			if (
				await fetch(`${origin}/icon.svg`).then(
					() => true,
					() => false,
				)
			)
				break;
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
		const status = await fetch(`${origin}/api/status`, { headers: { origin, "sec-fetch-site": "same-origin" } });
		assert.equal(status.status, 200, await status.clone().text());
		const told = await status.json();
		assert.equal(told.mode, "local");
		assert.equal(JSON.stringify(told).includes(engine.token), false);

		const sockets = listening([engine.child.pid, child.pid]);
		if (sockets === null) t.diagnostic("the listening sockets are read on Windows only: not checked on this system");
		else {
			assert.ok(sockets.length >= 2, `both listen: ${sockets.join(", ")}`);
			for (const socket of sockets) assert.match(socket, /^(127\.0\.0\.1|::1) /, `listens beyond this computer: ${socket}`);
		}
		// the log has what the engine said after its startup line, and not the token
		const logged = fs.readFileSync(path.join(data, "logs", "engine.log"), "utf8");
		assert.match(logged, /computing on \d+ threads/);
		assert.equal(logged.includes(engine.token), false);
	} finally {
		if (web) await stop(web.child);
		engine.child.stdin.end();
		const code = await Promise.race([engine.exited, new Promise((resolve) => setTimeout(() => resolve("late"), 10_000))]);
		if (code === "late") engine.child.kill();
		assert.notEqual(code, "late", "the engine stops when its standard input closes");
		fs.rmSync(data, { recursive: true, force: true });
	}
});
