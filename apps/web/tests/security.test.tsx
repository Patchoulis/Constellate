/** What a hostile page, paper or answer must not get out of the interface. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MathText } from "@/components/math-text";
import {
	addressedToUs,
	allowedHosts,
	contentSecurityPolicy,
	DEV_ENGINE,
	engineAddress,
	engineRequestHeaders,
	engineUrl,
	fromAnotherSite,
	hostName,
	isBackendPath,
	WINDOW_COOKIE,
	windowAdmitted,
} from "@/lib/page-policy";

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

test("the engine's answers keep their own headers; pages get the interface's policy", () => {
	for (const path of ["/api", "/api/settings", "/auth", "/auth/", "/auth/sign-in.js"]) assert.ok(isBackendPath(path), path);
	for (const path of ["/", "/apiary", "/authors"]) assert.ok(!isBackendPath(path), path);
});

test("a request forwarded to the engine carries the engine's token and no token of the page's", () => {
	const page = new Headers({ origin: "http://127.0.0.1:3210", authorization: "Bearer from-a-page", range: "bytes=0-7" });
	const sent = engineRequestHeaders(page, "the-engines-token");
	assert.equal(sent.get("authorization"), "Bearer the-engines-token");
	assert.equal(sent.get("origin"), "http://127.0.0.1:3210", "the engine checks the page's Origin");
	assert.equal(sent.get("range"), "bytes=0-7");
	assert.equal(page.get("authorization"), "Bearer from-a-page", "the page's own headers are left as they were");
	// on a server, where the owner signs in, nothing is added and a page's token goes nowhere
	assert.equal(engineRequestHeaders(page, undefined).get("authorization"), null);
	assert.equal(engineRequestHeaders(page, "").get("authorization"), null);
});

test("the engine is reached at the address the shell gives, the path and query kept", () => {
	const engine = engineAddress("http://127.0.0.1:52817");
	assert.equal(
		engineUrl(engine, new URL("http://127.0.0.1:3210/api/papers/hep-th%2F9901001/pdf?x=1")).href,
		"http://127.0.0.1:52817/api/papers/hep-th%2F9901001/pdf?x=1",
	);
	assert.equal(engineUrl(engine, new URL("http://localhost:3210/auth")).href, "http://127.0.0.1:52817/auth/");
	assert.equal(engineAddress(undefined).href, `${DEV_ENGINE}/`);
	assert.equal(engineAddress("  ").href, `${DEV_ENGINE}/`);
	assert.throws(() => engineAddress("file:///etc/passwd"));
});

test("only the desktop app's window, by the key in its cookie, reaches the engine through the interface's server", () => {
	const key = "k".repeat(64);
	assert.ok(windowAdmitted(`${WINDOW_COOKIE}=${key}`, key));
	assert.ok(windowAdmitted(`theme=dark; ${WINDOW_COOKIE}=${key}`, key));
	for (const cookies of [null, "", `${WINDOW_COOKIE}=${"j".repeat(64)}`, `${WINDOW_COOKIE}=${key}x`, `other=${key}`]) {
		assert.equal(windowAdmitted(cookies, key), false, String(cookies));
	}
	assert.ok(windowAdmitted(null, undefined), "without a key (npm run dev, a server) every request is admitted");
	// the key goes no further than the interface's server
	const sent = engineRequestHeaders(new Headers({ cookie: `${WINDOW_COOKIE}=${key}; theme=dark` }), "t");
	assert.equal(sent.get("cookie"), "theme=dark");
	assert.equal(engineRequestHeaders(new Headers({ cookie: `${WINDOW_COOKIE}=${key}` }), "t").get("cookie"), null);
});

test("another site's page cannot reach the engine through the interface's server", () => {
	const ask = (headers: Record<string, string>, path = "/api/status") => fromAnotherSite(new Headers(headers), path);
	assert.equal(ask({}), false, "no page made it (a program)");
	assert.equal(ask({ "sec-fetch-site": "same-origin", "sec-fetch-mode": "cors" }), false, "the interface's own page");
	assert.equal(ask({ "sec-fetch-site": "none", "sec-fetch-mode": "navigate", "sec-fetch-dest": "document" }), false, "a typed address");
	const followed = { "sec-fetch-site": "cross-site", "sec-fetch-mode": "navigate", "sec-fetch-dest": "document" };
	assert.equal(ask(followed, "/auth/callback"), false, "a sign-in coming back");
	assert.equal(ask(followed, "/api/papers/x/pdf"), true, "a link on another site to the API");
	assert.equal(ask({ "sec-fetch-site": "cross-site", "sec-fetch-mode": "no-cors", "sec-fetch-dest": "image" }), true, "an image");
	assert.equal(
		ask({ "sec-fetch-site": "same-site", "sec-fetch-mode": "no-cors", "sec-fetch-dest": "script" }),
		true,
		"another port of this computer",
	);
	assert.equal(ask({ "sec-fetch-site": "cross-site", "sec-fetch-mode": "navigate", "sec-fetch-dest": "iframe" }), true, "a frame");
	assert.equal(ask({ "sec-fetch-site": "cross-site", "sec-fetch-mode": "cors" }), true, "a fetch");
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
