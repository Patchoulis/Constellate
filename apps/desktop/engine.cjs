/**
 * The app's two servers, started as children of the shell: the engine (apps/engine, the Rust program `constellate`)
 * and the interface's server (Next's standalone server.js). Nothing in this file needs Electron, so the shell's
 * tests start the very same servers the app does (servers.test.cjs).
 *
 * The engine
 *   - is started in the data folder, so the `.env` it reads from its working folder is `<data>/.env`: the one
 *     place the installed app's keys go (README.md, "Keys");
 *   - gets an environment of what it needs and nothing else (engineEnvironment): the data folder, the interface's
 *     origin, how it listens (this computer only, a port of its choosing, a token it makes up: fixed here, so that a
 *     `.env` cannot change them), the language model's keys and addresses, and what the system needs to run a
 *     program, find the user's folders, reach the network through the user's proxy and a graphics card;
 *   - says where it listens in one line on standard output, the token with it (apps/engine/README.md, "The startup
 *     line"). That line is read here and never written anywhere; every later line of its output goes to
 *     `<data>/logs/engine.log` (EngineLog: two files of at most 2 MB, the token blanked should it ever appear);
 *   - stops when its standard input closes: the pipe is held for as long as the shell lives, so a shell that ends
 *     however it ends leaves no engine behind.
 *
 * The interface's server gets the engine's address and its token in CONSTELLATE_ENGINE and
 * CONSTELLATE_ENGINE_TOKEN, and adds the token to every request it forwards to the engine itself (apps/web/proxy.ts):
 * no page ever sees it. It gets no provider key: it shows pages and has no use for one.
 */
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");

/** How long the engine has to say where it listens. It binds a port and nothing more before it does; the rest is a
 *  margin for a slow disk or a virus scanner looking at a new program for the first time. */
const STARTUP_DEADLINE_MS = 60_000;

/**
 * The language model's keys and addresses the engine reads (apps/engine/src/model/language/provider.rs
 * `variables`; listed in .env.example, which a test holds this list to). Taken from the user's environment;
 * the same names in `<data>/.env` are read by the engine itself.
 */
const PROVIDER_VARIABLES = [
	"ANTHROPIC_API_KEY",
	"ANTHROPIC_BASE_URL",
	"OPENAI_API_KEY",
	"OPENAI_BASE_URL",
	"GEMINI_API_KEY",
	"GEMINI_BASE_URL",
	"CONSTELLATE_MODEL_BASE_URL",
	"CONSTELLATE_MODEL_API_KEY",
	"CONSTELLATE_MODEL_JSON",
	"OPENROUTER_API_KEY",
	"XAI_API_KEY",
	"MISTRAL_API_KEY",
	"DEEPSEEK_API_KEY",
	"GROQ_API_KEY",
	"TOGETHER_API_KEY",
	"FIREWORKS_API_KEY",
	"CEREBRAS_API_KEY",
	"MOONSHOT_API_KEY",
	"MINIMAX_API_KEY",
	"ZAI_API_KEY",
	"NVIDIA_API_KEY",
	"HF_TOKEN",
	"AI_GATEWAY_API_KEY",
	"BASETEN_API_KEY",
	"META_API_KEY",
];

/**
 * What any program needs from the environment to run: the system's own folders, the user's (home, temporary
 * files), the proxy the user set for reaching the network (the engine's HTTP client honours it), and on Linux
 * where the graphics card's Vulkan driver is described. Names are matched without regard to case on Windows.
 */
const SYSTEM_VARIABLES = {
	win32: ["SystemRoot", "windir", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "PATH"],
	linux: ["HOME", "TMPDIR", "PATH", "LANG", "XDG_DATA_HOME", "XDG_DATA_DIRS", "XDG_CONFIG_DIRS", "VK_ICD_FILENAMES", "VK_DRIVER_FILES"],
	darwin: ["HOME", "TMPDIR", "PATH", "LANG"],
};
const PROXY_VARIABLES = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "all_proxy", "no_proxy"];
/** Where the embedding model is downloaded from (a mirror) and where a copy may already be (apps/engine/src/pipeline/model_files.rs). */
const MODEL_VARIABLES = ["HF_ENDPOINT", "HF_HOME", "HF_HUB_CACHE"];

/** `names` taken from `source` where they are set (on Windows, whatever the case they are written in there). */
function pick(source, names, platform) {
	const picked = {};
	const wanted = new Map(names.map((name) => [platform === "win32" ? name.toUpperCase() : name, name]));
	for (const [name, value] of Object.entries(source)) {
		const key = platform === "win32" ? name.toUpperCase() : name;
		if (wanted.has(key) && typeof value === "string" && value !== "") picked[wanted.get(key)] = value;
	}
	return picked;
}

/** What a server needs from the system to run at all. */
function systemEnvironment(source, platform) {
	return pick(source, [...(SYSTEM_VARIABLES[platform] ?? SYSTEM_VARIABLES.linux), ...PROXY_VARIABLES], platform);
}

/**
 * The engine's whole environment. How it listens is fixed here, over anything a `.env` says (the environment wins
 * over `.env`, and an empty value counts as unset): this computer only, a free port, a token it makes up and
 * reports on its startup line, no certificate, no other names.
 */
function engineEnvironment(source, { data, origin, platform = process.platform }) {
	return {
		...systemEnvironment(source, platform),
		...pick(source, [...MODEL_VARIABLES, ...PROVIDER_VARIABLES], platform),
		CONSTELLATE_DATA: data,
		CONSTELLATE_ORIGIN: origin,
		CONSTELLATE_BIND: "127.0.0.1",
		CONSTELLATE_PORT: "0",
		CONSTELLATE_TOKEN: "",
		CONSTELLATE_HOST: "",
		CONSTELLATE_TLS_CERT: "",
		CONSTELLATE_TLS_KEY: "",
		CONSTELLATE_ALLOW_TOKEN: "",
	};
}

/** The interface's server's whole environment: no key of any provider, the engine's address and token. */
function interfaceEnvironment(source, { port, engine, token, platform = process.platform }) {
	return {
		...systemEnvironment(source, platform),
		ELECTRON_RUN_AS_NODE: "1",
		NODE_ENV: "production",
		NEXT_TELEMETRY_DISABLED: "1",
		PORT: String(port),
		HOSTNAME: "127.0.0.1",
		CONSTELLATE_ENGINE: engine,
		CONSTELLATE_ENGINE_TOKEN: token,
	};
}

/**
 * The engine's startup line, checked: it must say it listens over plain HTTP on this computer, on a port, with a
 * token it made up. Anything else is not the engine this shell started (or not started as this shell asked).
 */
function parseStartupLine(line) {
	let told;
	try {
		told = JSON.parse(line);
	} catch {
		throw new Error("The engine's first line is not the startup line it should be.");
	}
	const { scheme, address, port, token } = told ?? {};
	if (scheme !== "http" || address !== "127.0.0.1")
		throw new Error("The engine does not listen on this computer over plain HTTP, as the app asked.");
	if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("The engine's startup line names no port.");
	if (typeof token !== "string" || !/^[A-Za-z0-9._~+/=-]{32,}$/.test(token)) {
		throw new Error("The engine's startup line carries no token (is CONSTELLATE_TOKEN set somewhere the app does not control?).");
	}
	return { url: `http://127.0.0.1:${port}`, token };
}

/**
 * The engine's log: what it says after its startup line, kept in `folder` as `engine.log`. A file that reaches
 * `limit` bytes becomes `engine.1.log` (the one before is dropped), so at most twice `limit` is ever kept.
 * `secret` (the token) is blanked out of every line, though the engine never repeats it.
 */
class EngineLog {
	constructor(folder, { limit = 2 * 1024 * 1024 } = {}) {
		this.file = path.join(folder, "engine.log");
		this.older = path.join(folder, "engine.1.log");
		this.limit = limit;
		this.secret = null;
		this.size = 0;
		this.tail = "";
		try {
			fs.mkdirSync(folder, { recursive: true });
			this.size = fs.statSync(this.file).size;
		} catch {
			this.size = 0; // no log yet
		}
	}

	/** Text never written: the token, once it is known. */
	hide(secret) {
		this.secret = secret;
	}

	write(text) {
		const shown = this.secret ? text.split(this.secret).join("[token]") : text;
		this.tail = (this.tail + shown).slice(-4000); // what a failure message quotes
		const bytes = Buffer.byteLength(shown);
		try {
			if (this.size + bytes > this.limit && this.size > 0) {
				fs.rmSync(this.older, { force: true });
				fs.renameSync(this.file, this.older);
				this.size = 0;
			}
			fs.appendFileSync(this.file, shown);
			this.size += bytes;
		} catch {
			// a log that cannot be written does not stop the app
		}
	}
}

/**
 * Start the engine and resolve, once it has said where it listens, with `{ child, url, token, exited }`; reject if
 * it ends first, says something else, or says nothing for `deadline` ms (it is then stopped). `exited` settles when
 * it ends, with its exit code. `admit(child)` is asked first whether the child may live (the shell may be closing).
 */
function startEngine({ binary, data, origin, env = process.env, log, deadline = STARTUP_DEADLINE_MS, admit = () => true }) {
	return new Promise((resolve, reject) => {
		const child = spawn(binary, [], {
			cwd: data,
			env: engineEnvironment(env, { data, origin }),
			stdio: ["pipe", "pipe", "pipe"],
			windowsHide: true,
		});
		if (!admit(child)) {
			child.kill();
			return reject(new Error("The engine was not started: the app is closing."));
		}
		const exited = new Promise((done) => child.once("exit", (code) => done(code)));
		let first = "";
		let told = false;
		const settle = (error, value) => {
			if (told) return;
			told = true;
			clearTimeout(timer);
			if (error) {
				child.kill();
				reject(error);
			} else resolve(value);
		};
		const timer = setTimeout(
			() => settle(new Error(`The engine said nothing for ${deadline / 1000} seconds after it was started.\n\n${log.tail}`)),
			deadline,
		);
		child.stdout.setEncoding("utf8");
		child.stderr.setEncoding("utf8");
		child.stderr.on("data", (text) => log.write(text));
		child.stdout.on("data", (text) => {
			if (told) return log.write(text);
			first += text;
			const end = first.indexOf("\n");
			if (end < 0) {
				if (first.length > 4096) settle(new Error("The engine's first line is not the startup line it should be."));
				return;
			}
			const rest = first.slice(end + 1);
			try {
				const where = parseStartupLine(first.slice(0, end).trim());
				log.hide(where.token);
				first = "";
				settle(null, { child, ...where, exited });
			} catch (error) {
				settle(error);
			}
			if (rest) log.write(rest);
		});
		child.once("error", (error) => settle(new Error(`The engine could not be started (${binary}): ${error.message}`)));
		child.once("exit", (code) => settle(new Error(`The engine stopped before it was ready (code ${code}).\n\n${log.tail}`)));
	});
}

/** Does something answer at `url`? */
function answers(url) {
	return new Promise((resolve) => {
		const request = http.get(url, (response) => {
			response.resume();
			resolve(true);
		});
		request.once("error", () => resolve(false));
		request.setTimeout(2000, () => request.destroy());
	});
}

module.exports = {
	EngineLog,
	PROVIDER_VARIABLES,
	STARTUP_DEADLINE_MS,
	answers,
	engineEnvironment,
	interfaceEnvironment,
	parseStartupLine,
	startEngine,
};
