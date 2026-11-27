/**
 * The interface's port, the same one on every launch.
 *
 * What the interface remembers in the window (the papers someone marked, how wide they dragged the panels) is kept
 * by the browser per address, and the port is part of the address: on a new port each launch, all of it is gone.
 * So the port taken once is written down and taken again. Only when something else holds it is another one taken
 * (and written down in its place): the app still starts, and what the window remembered starts afresh.
 */
const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");

/** The port a file names, or 0 ("any") when it names none that a program may listen on. */
function remembered(text) {
	const port = Number(String(text).trim());
	return Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : 0;
}

/** Listen on `port` (0: any free one) for an instant; the port that was free, or null when `port` is taken. */
function probe(port) {
	return new Promise((resolve) => {
		const server = net.createServer();
		server.once("error", () => resolve(null));
		server.listen(port, "127.0.0.1", () => {
			const taken = server.address().port;
			server.close(() => resolve(taken));
		});
	});
}

/** The port to serve the interface on: the one `file` remembers when it is free, else a free one, remembered. */
async function interfacePort(file) {
	let before = 0;
	try {
		before = remembered(fs.readFileSync(file, "utf8"));
	} catch {
		// nothing remembered yet
	}
	const port = (before && (await probe(before))) || (await probe(0));
	if (!port) throw new Error("No port on this computer is free for the interface.");
	if (port !== before) {
		try {
			fs.mkdirSync(path.dirname(file), { recursive: true });
			fs.writeFileSync(file, `${port}\n`);
		} catch {
			// not remembered: the next launch takes any port, as this one did
		}
	}
	return port;
}

module.exports = { interfacePort, probe, remembered };
