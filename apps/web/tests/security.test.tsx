/** What a hostile page, paper or answer must not get out of the interface. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MathText } from "@/components/math-text";
import { addressedToUs, allowedHosts, contentSecurityPolicy, hostName, isBackendPath } from "@/lib/page-policy";

const shown = (text: string) => renderToStaticMarkup(<MathText>{text}</MathText>);

test("the interface answers only under this computer's names and those configured (DNS rebinding)", () => {
	const local = allowedHosts(undefined);
	for (const host of ["127.0.0.1:3210", "localhost:3210", "LOCALHOST", "[::1]:3210"]) assert.ok(addressedToUs(host, local), host);
	for (const host of ["rebound.evil.example:3210", "127.0.0.1.evil.example:3210", "localhost.evil.example", "", null])
		assert.ok(!addressedToUs(host, local), String(host));
	const server = allowedHosts(" papers.example.org , ");
	assert.ok(addressedToUs("papers.example.org", server));
	assert.ok(!addressedToUs("other.example.org", server));
	assert.equal(hostName("[::1]:80"), "[::1]");
});

test("the backend's answers keep their own headers; pages get the interface's policy", () => {
	for (const path of ["/api", "/api/settings", "/auth", "/auth/", "/auth/sign-in.js"]) assert.ok(isBackendPath(path), path);
	for (const path of ["/", "/apiary", "/authors"]) assert.ok(!isBackendPath(path), path);
});

test("a page's policy runs only its own nonce's scripts, asks only its own server and allows no plugin or <base>", () => {
	const policy = contentSecurityPolicy("abc", false);
	const directive = (name: string) => policy.split("; ").find((part) => part.startsWith(`${name} `));
	assert.equal(directive("script-src"), "script-src 'self' 'nonce-abc' 'strict-dynamic'");
	assert.equal(directive("connect-src"), "connect-src 'self'");
	assert.equal(directive("object-src"), "object-src 'none'");
	assert.equal(directive("base-uri"), "base-uri 'none'");
	assert.equal(directive("form-action"), "form-action 'self'");
	assert.equal(directive("frame-ancestors"), "frame-ancestors 'self'");
	assert.ok(!policy.includes("unsafe-eval"));
	assert.ok(contentSecurityPolicy("abc", true).includes("'unsafe-eval'"));
});

test("math cannot draw a box over the window, link anywhere or embed anything", () => {
	// a huge rule would cover the app with whatever the paper wants shown
	const huge = shown("$\\rule{100000em}{100000em}$");
	const sizes = [...huge.matchAll(/(\d+(?:\.\d+)?)em/g)].map((found) => Number(found[1]));
	assert.ok(Math.max(...sizes) <= 20, `a rule ${Math.max(...sizes)}em large`);
	for (const hostile of [
		"$\\href{javascript:alert(1)}{x}$",
		"$\\url{javascript:alert(1)}$",
		"$\\includegraphics{https://evil.example/a.png}$",
		"$\\htmlClass{x}{y}$",
		"$\\htmlData{onload=alert(1)}{y}$",
	]) {
		const html = shown(hostile);
		assert.ok(!/<a |<img |javascript:|data-onload/.test(html), `${hostile}: ${html}`);
	}
	assert.ok(!shown("$<img src=x onerror=alert(1)>$").includes("<img"));
	// a macro that expands without end is stopped
	const bomb = shown("$\\def\\a{\\a\\a}\\a$");
	assert.ok(bomb.length < 10_000);
});

test("a link in a title or abstract is a web address or no link at all", () => {
	for (const hostile of ["\\url{javascript:alert(1)}", "\\href{javascript:alert(1)}{click}", "\\href{data:text/html,x}{x}"])
		assert.ok(!shown(hostile).includes("<a "), hostile);
	assert.ok(!shown('https://example.org/"onmouseover="alert(1)').includes('onmouseover="'));
});
