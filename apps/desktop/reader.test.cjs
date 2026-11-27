/** What the embedded reader may open and what its pages may reach.    node --test */
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readable, local, refused } = require("./reader.cjs");

test("the reader opens https pages of the sites papers live on, their sub-domains included", () => {
	for (const address of [
		"https://openreview.net/forum?id=abc",
		"https://api.openreview.net/notes",
		"https://arxiv.org/pdf/2401.00001#page=3",
		"https://export.arxiv.org/abs/2401.00001",
		"https://OPENREVIEW.NET/forum",
		"https://openreview.net./forum",
	]) {
		assert.ok(readable(address), address);
	}
});

test("a look-alike site, another scheme or this computer is not opened in the reader", () => {
	for (const address of [
		"https://evil-openreview.net/forum",
		"https://openreview.net.evil.example/forum",
		"https://notarxiv.org/abs/1",
		"http://openreview.net/forum",
		"file:///C:/Windows/win.ini",
		"https://127.0.0.1:8765/api/settings",
		"https://localhost:3210/",
		"javascript:alert(1)",
		"not a url",
		"",
	]) {
		assert.equal(readable(address), false, address);
	}
});

test("every way of writing this computer's address is local", () => {
	for (const address of [
		"http://127.0.0.1:8765/api/runs",
		"http://127.1:8765/",
		"http://2130706433:8765/",
		"http://0x7f000001/",
		"http://127.0.0.2/",
		"http://0.0.0.0:3210/",
		"http://localhost:3210/",
		"http://localhost.:3210/",
		"http://app.localhost/",
		"http://LOCALHOST/",
		"http://[::1]:8765/",
		"http://[::]:8765/",
		"http://[::ffff:127.0.0.1]:8765/",
		"ws://127.0.0.1:9229/",
	]) {
		assert.ok(local(address), address);
	}
});

test("the local network is local, the internet is not", () => {
	for (const address of [
		"http://192.168.1.1/",
		"http://10.0.0.5/",
		"http://172.16.0.1/",
		"http://169.254.169.254/latest",
		"http://[fe80::1]/",
		"http://[fd00::1]/",
	]) {
		assert.ok(local(address), address);
	}
	for (const address of [
		"https://openreview.net/",
		"https://172.32.0.1/",
		"https://8.8.8.8/",
		"https://[2606:4700::1111]/",
		"https://localhost.example.com/",
	]) {
		assert.equal(local(address), false, address);
	}
});

test("a page in the reader may reach https and wss on the internet and nothing else", () => {
	assert.equal(refused("https://openreview.net/notes?id=1"), false);
	assert.equal(refused("https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-chtml.js"), false);
	assert.equal(refused("wss://openreview.net/socket"), false);
	for (const address of [
		"http://127.0.0.1:8765/api/runs",
		"https://127.0.0.1:8765/api/runs",
		"https://localhost:3210/",
		"wss://[::1]:8765/",
		"http://example.com/",
		"file:///C:/Users/",
		"ftp://example.com/",
		"https://192.168.0.1/",
		"garbage",
	]) {
		assert.ok(refused(address), address);
	}
});
