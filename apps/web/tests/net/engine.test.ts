/** What the window does differently when its engine is a server the owner signs in to. */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { Settings } from "@rp/contracts";
import { CSRF_HEADER, changesSomething, csrfOf, serverOwned, signInRedirect, splitByOwner, withCsrf } from "@/lib/net/engine";

test("only a server owns settings, and only those it names", () => {
	assert.deepEqual([...serverOwned({ mode: "network", server_owned: ["embedding_model"] })], ["embedding_model"]);
	assert.equal(serverOwned({ mode: "local", server_owned: ["embedding_model"] }).size, 0);
	assert.equal(serverOwned({}).size, 0); // a status that says neither
	assert.equal(serverOwned(undefined).size, 0); // not known yet
	assert.equal(serverOwned({ mode: "network" }).size, 0);
});

test("the fields a server owns are taken out of their group and kept to be named", () => {
	const fields: { key: keyof Settings; label: string }[] = [
		{ key: "embedding_model", label: "Embedding model" },
		{ key: "center_embeddings", label: "Centre embeddings" },
	];
	const { shown, decided } = splitByOwner(fields, new Set<keyof Settings>(["embedding_model"]));
	assert.deepEqual(
		shown.map((field) => field.key),
		["center_embeddings"],
	);
	assert.deepEqual(
		decided.map((field) => field.label),
		["Embedding model"],
	);
	assert.equal(splitByOwner(fields, new Set()).shown.length, 2);
});

test("an answer that says nobody is signed in sends the window to the sign-in page, once", () => {
	const session = 'Session realm="constellate", sign_in="/auth/"';
	assert.equal(signInRedirect(401, session, "/"), "/auth/");
	assert.equal(signInRedirect(401, session, "/runs/x"), "/auth/");
	assert.equal(signInRedirect(401, session, "/auth/"), null); // already there: no loop
	assert.equal(signInRedirect(401, session, "/auth"), null);
	// a wrong token on the user's own computer: signing in would not help
	assert.equal(signInRedirect(401, "Bearer", "/"), null);
	assert.equal(signInRedirect(401, null, "/"), null);
	for (const status of [200, 403, 404, 429, 500]) assert.equal(signInRedirect(status, session, "/"), null);
});

test("a change carries the session's token and a read does not", () => {
	assert.equal(changesSomething(undefined), false);
	assert.equal(changesSomething("get"), false);
	for (const method of ["POST", "PUT", "PATCH", "DELETE", "put"]) assert.equal(changesSomething(method), true);

	const sent = withCsrf({ method: "PUT", headers: { "Content-Type": "application/json" } }, "token-1");
	const headers = new Headers(sent?.headers);
	assert.equal(headers.get(CSRF_HEADER), "token-1");
	assert.equal(headers.get("Content-Type"), "application/json");
	const read = { method: "GET" };
	assert.equal(withCsrf(read, "token-1"), read);
	const local = { method: "POST" };
	assert.equal(withCsrf(local, null), local); // on the user's own computer there is no token
	assert.equal(withCsrf(undefined, "token-1"), undefined);
});

test("the token is read only from a signed-in session", () => {
	assert.equal(csrfOf({ signed_in: true, csrf: "t" }), "t");
	assert.equal(csrfOf({ signed_in: false, methods: [], provider: null, setup: false }), null);
	assert.equal(csrfOf({ signed_in: true }), null);
	assert.equal(csrfOf(null), null);
	assert.equal(csrfOf("<html>not found</html>"), null);
});
