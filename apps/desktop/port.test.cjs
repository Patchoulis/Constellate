const assert = require("node:assert/strict");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");

const { interfacePort, probe, remembered } = require("./port.cjs");

function scratch() {
	return path.join(fs.mkdtempSync(path.join(os.tmpdir(), "constellate-port-")), "kept", "interface-port");
}

function hold(port) {
	return new Promise((resolve) => {
		const server = net.createServer();
		server.listen(port, "127.0.0.1", () => resolve(server));
	});
}

test("only a port a program may listen on is remembered", () => {
	assert.equal(remembered("51664\n"), 51664);
	for (const text of ["", "0", "80", "70000", "51664.5", "port", "-3"]) assert.equal(remembered(text), 0);
});

test("the first launch takes a free port and writes it down", async () => {
	const file = scratch();
	const port = await interfacePort(file);
	assert.equal(remembered(fs.readFileSync(file, "utf8")), port);
});

test("the next launch takes the same port", async () => {
	const file = scratch();
	const first = await interfacePort(file);
	assert.equal(await interfacePort(file), first);
	assert.equal(await interfacePort(file), first);
});

test("a remembered port something else holds is replaced by a free one", async () => {
	const file = scratch();
	const first = await interfacePort(file);
	const holder = await hold(first);
	try {
		const second = await interfacePort(file);
		assert.notEqual(second, first);
		assert.equal(remembered(fs.readFileSync(file, "utf8")), second);
	} finally {
		holder.close();
	}
});

test("a file that names no port is ignored and corrected", async () => {
	const file = scratch();
	fs.mkdirSync(path.dirname(file), { recursive: true });
	fs.writeFileSync(file, "not a port");
	const port = await interfacePort(file);
	assert.equal(remembered(fs.readFileSync(file, "utf8")), port);
});

test("probing a free port answers that port, any port answers a real one, and a held port answers null", async () => {
	const any = await probe(0);
	assert.ok(Number.isInteger(any) && any > 0 && any <= 65535, `port ${any}`);
	assert.equal(await probe(any), any, `port ${any} is free again once probed`);
	const holder = net.createServer();
	await new Promise((resolve) => holder.listen(0, "127.0.0.1", resolve));
	try {
		assert.equal(await probe(holder.address().port), null);
	} finally {
		await new Promise((resolve) => holder.close(resolve));
	}
});
