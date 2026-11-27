/**
 * The git hooks (.githooks) and the shapes of secrets they and CI look for (.githooks/secrets.txt).
 * Needs sh and git, as committing does.    node --test scripts
 *
 * Every sample key below is put together from pieces when the test runs, so that this file itself holds
 * nothing shaped like a key.
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const hooks = join(root, ".githooks");
const patterns = readFileSync(join(hooks, "secrets.txt"), "utf8").split("\n");

const repeat = (text, times) => text.repeat(times);
/** Keys of the shapes the providers in .env.example hand out, and the other secrets a repository must not hold. */
const KEYS = {
	anthropic: ["sk-", "ant-", "api03-", repeat("aB3_", 20)].join(""),
	openai: ["sk-", "proj-", repeat("Xy9-", 30)].join(""),
	"openai (older)": ["sk-", repeat("Ab12", 12)].join(""),
	deepseek: ["sk-", repeat("0f", 16)].join(""),
	openrouter: ["sk-", "or-v1-", repeat("ab", 32)].join(""),
	gemini: ["AI", "za", repeat("Sy", 17), "Q"].join(""),
	xai: ["xa", "i-", repeat("Kq7", 27)].join(""),
	groq: ["gs", "k_", repeat("Wd2", 18)].join(""),
	nvidia: ["nva", "pi-", repeat("z9_", 22)].join(""),
	cerebras: ["cs", "k-", repeat("m4", 24)].join(""),
	fireworks: ["fw", "_", repeat("T6", 12)].join(""),
	together: ["tgp", "_v1_", repeat("Lp", 20)].join(""),
	"hugging face": ["hf", "_", repeat("Qw", 18)].join(""),
	"github token": ["gh", "p_", repeat("A1", 18)].join(""),
	"github fine-grained": ["github", "_pat_", repeat("B2_", 14)].join(""),
	aws: ["AK", "IA", repeat("Z7", 8)].join(""),
	"private key": ["-----BEGIN ", "RSA PRIVATE", " KEY-----"].join(""),
	"openssh key": ["-----BEGIN ", "OPENSSH PRIVATE", " KEY-----"].join(""),
	"pgp key": ["-----BEGIN ", "PGP PRIVATE", " KEY BLOCK-----"].join(""),
};

/** Text of the kind this repository is full of, which must not be taken for a secret. */
const ORDINARY = [
	"const disk-cleanup-of-every-temporary-folder-made = 1",
	"task-queue-with-a-name-that-is-rather-long-indeed",
	'"integrity": "sha512-q0bYMEtP3q8pCNYnEHMDmYqEmqrQ5aDTCgXL2bZpFNgAAABBBCCCDDD=="',
	"ANTHROPIC_API_KEY=",
	"risk-assessment-for-the-whole-run-and-its-results",
	"https://github.com/astral-sh/uv/issues/19622",
	"-----BEGIN CERTIFICATE-----",
];

const shaped = (text) => patterns.slice(0, -1).some((pattern) => new RegExp(pattern).test(text));

test("the shapes of secrets are one regular expression per line, with no empty line (which would match everything)", () => {
	assert.equal(patterns.at(-1), "", "the file ends with a newline");
	for (const pattern of patterns.slice(0, -1)) {
		assert.notEqual(pattern.trim(), "", "an empty line");
		assert.doesNotThrow(() => new RegExp(pattern), pattern);
	}
});

test("every provider's key, a token and a private key are each taken for a secret", () => {
	for (const [what, key] of Object.entries(KEYS)) assert.ok(shaped(`value = "${key}"`), what);
});

test("ordinary text is not taken for a secret", () => {
	for (const line of ORDINARY) assert.equal(shaped(line), false, line);
});

test("no file of the repository holds anything shaped like a secret", () => {
	const found = spawnSync("git", ["grep", "-nIE", "-f", join(hooks, "secrets.txt")], { cwd: root, encoding: "utf8" });
	assert.equal(found.status, 1, `git grep found (or failed): ${found.stdout.slice(0, 300)}${found.stderr}`); // 1: nothing found
});

function sh(hook, args, cwd) {
	const done = spawnSync("sh", [join(hooks, hook), ...args], { cwd, encoding: "utf8" });
	assert.ok(done.status !== null, `sh could not run ${hook}: ${done.error?.message}`);
	return done.status;
}

test("a commit message with a key or a tool's credit is refused, an ordinary one goes through", () => {
	const folder = mkdtempSync(join(tmpdir(), "rp-hooks-"));
	const message = join(folder, "MESSAGE");
	const verdict = (text) => {
		writeFileSync(message, text);
		return sh("commit-msg", [message], folder);
	};
	assert.equal(verdict("fix(desktop): stop the server with the shell\n\nIt stayed behind.\n"), 0);
	assert.equal(verdict(`fix(desktop): stop the server\n\nkey ${KEYS.openai}\n`), 1);
	assert.equal(verdict(`fix(desktop): stop the server\n\n${KEYS.gemini}\n`), 1);
	assert.equal(verdict("fix(desktop): stop the server\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n"), 1);
	assert.equal(verdict("Fixed the server."), 1);
	rmSync(folder, { recursive: true, force: true });
});

test("a commit that stages a key or a secret file is refused, an ordinary one goes through", () => {
	const folder = mkdtempSync(join(tmpdir(), "rp-hooks-"));
	const git = (...args) => assert.equal(spawnSync("git", args, { cwd: folder }).status, 0, `git ${args.join(" ")}`);
	git("init", "-q");
	const staged = (name, content) => {
		writeFileSync(join(folder, name), content);
		git("add", name);
		const status = sh("pre-commit", [], folder);
		git("rm", "-q", "--cached", name);
		return status;
	};
	assert.equal(staged("notes.txt", "nothing secret here\n"), 0);
	assert.equal(staged("config.ts", `export const key = "${KEYS.anthropic}";\n`), 1);
	assert.equal(staged("config.ts", `const groq = "${KEYS.groq}";\n`), 1);
	assert.equal(staged("server.pem", "certificate\n"), 1);
	assert.equal(staged(".env.production", "X=1\n"), 1);
	assert.equal(staged("production.env", "X=1\n"), 1);
	assert.equal(staged(".env.example", "ANTHROPIC_API_KEY=\n"), 0);
	assert.equal(staged(".env.example", "ANTHROPIC_API_KEY=abc\n"), 1);
	rmSync(folder, { recursive: true, force: true });
});
