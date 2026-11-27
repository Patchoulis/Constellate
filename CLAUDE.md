# Constellate: working rules

Constellate is a desktop app that gathers research papers (conference lists, arXiv), embeds and clusters them,
and helps a researcher find what matters: communities, overviews with citations, acceptance tiers, authors'
declarations, reviewers' ratings. README.md describes what it does, CONTRIBUTING.md how changes are made and
the style they follow. This file is the short version, plus what an automated assistant must not get wrong.

## Layout

| Path                 | What                                                                                             |
| -------------------- | ------------------------------------------------------------------------------------------------ |
| `apps/engine`        | The Rust engine `constellate`: the pipeline, the library, the HTTP API on 127.0.0.1 (its README) |
| `apps/web`           | The interface: Next.js, React, Tailwind, a d3 canvas; its server forwards /api to the engine     |
| `apps/desktop`       | Electron shell: starts the engine and the interface's server, opens the window; the packages     |
| `packages/contracts` | TypeScript types shared by the interface and described by the API                                |
| `scripts/build.mjs`  | `npm run build` / `npm run app` / `npm run dist`                                                 |
| `scripts/pr.mjs`     | `npm run pr`: everything a pull request must pass                                                |
| `data/`              | The library and runs when the app runs from the repository. Never committed.                     |

## Commands

```
npm run app            build and open the desktop app
npm run pr             format check, lint, types, every test (what CI runs)
npm run pr -- --fix    the same, formatting and fixing what it can
npm run dist           build this system's packages into dist/ (Windows installer; Linux AppImage and .deb)
npm run dev            the engine and the interface with hot reload, on http://localhost:3210
```

## Secrets: never commit them

- Anything sensitive, and every endpoint the app talks to or listens on, is configured through environment variables
  or a `.env` file, never written in code or a tracked file. `.env.example` lists every name with no values; add a
  new variable there when you add one.
- API keys live in the environment or in the `.env` the engine reads from its working folder, which for the desktop
  app is its data folder (`<data>/.env`, README.md "Keys"); never in this repository. Only the engine is given them:
  the interface's server and the pages never are. Never write a key, token or password into a file in this
  repository, a test, a fixture, a log line, a commit message or the changelog.
- The engine makes up its token at each start, and the shell reads it from the engine's startup line, which is
  never logged or written anywhere. The shell gives it to the interface's server alone (`CONSTELLATE_ENGINE_TOKEN`),
  which adds it to what it forwards to the engine. It must never reach a page, a response header, a `NEXT_PUBLIC_`
  variable or a log (`apps/desktop/servers.test.cjs` checks).
- Never commit `data/`, `.env*`, `*.pem`, `*.key`, or `.claude/settings.local.json`. They are in `.gitignore`;
  do not remove them from it and do not use `git add -f` on them.
- Stage files by name or by folder you have looked at (`git add apps packages ...`), then read
  `git diff --cached --stat` before committing. If something unexpected is staged, stop.
- The hooks in `.githooks/` refuse a commit that contains something shaped like a key. Do not bypass them
  (`--no-verify`). If a hook is wrong, fix the hook.
- Nothing personal either: no real names, email addresses or paths of the machine in files or messages.
- If a secret is ever committed: say so at once, do not just delete it in a new commit. The key must be revoked
  and the history rewritten before anything is pushed.

## Every change: test, review, commit, release

1. **Branch** from `main`: `feat/...`, `fix/...`, `chore/...`. `main` only takes pull requests whose checks pass.
2. **Tests** come with the change. `npm run pr` must pass before committing.
3. **Review**: before a change is called done, an independent reviewer on the strongest available model reads
   the code just written, and its real findings are fixed.
4. **Changelog**: add the change under `## [Unreleased]` in `CHANGELOG.md` (Added / Changed / Fixed / Security),
   in words a user of the app would understand.
5. **Commit** with a Conventional Commit message (below).
6. **Release**: move the Unreleased entries under a new version heading with today's date, set that version in
   the root `package.json`, commit (`chore(release): vX.Y.Z`), merge, then tag `vX.Y.Z` on `main` and push the
   tag. The Release workflow builds the Windows installer and the Linux AppImage and .deb, and publishes one GitHub
   release with them and their checksums, its notes from the changelog section.
   Version by SemVer: `fix` raises the patch, `feat` the minor, a breaking change the major (while 0.x: the minor).

## Commit messages

```
<type>(<scope>): <summary in the imperative, lower case, no full stop>

<why, and what a reader of the diff cannot see>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
Scopes: `engine`, `web`, `desktop`, `contracts`, `build`, `release` (optional).

**A message describes the change and nothing else.** No mention of who or what wrote it: no assistant or tool
names, no `Co-Authored-By` lines for assistants, no "generated with". The same goes for pull request titles and
descriptions, the changelog and code comments. The `commit-msg` hook rejects such lines.

## Style (the full list is in CONTRIBUTING.md)

- Formatting is the formatter's: `cargo fmt` for Rust; Prettier for TypeScript, JSON, Markdown, YAML.
- `cargo clippy -- -D warnings` clean, `tsc` clean. A caught error is named `error`.
- Explicit states, each set written down in one place (`apps/engine/src/jobs.rs`, `useJob`, `useAsync`,
  `apps/desktop/lifecycle.cjs`), small modules, no repeated code.
- Work that was done is not done again: everything fetched or computed is kept by id in the library.
- The library is written through `apps/engine/src/library` (the database; `store` for files, replaced whole),
  never directly.
- Anything shown as a fact about a paper that is really an estimate or a program's reading (AI detected,
  declaration flags, ratings) says so where it is shown.

## Things that look wrong but are deliberate

- OpenReview serves a bot challenge to programs. Do not work around it. Reviews are read only from the page the
  user opens in the reader.
- The shell starts the engine in the data folder: that is how the `.env` it reads from its working folder is the
  data folder's. And it sets how the engine listens (`CONSTELLATE_BIND`, `CONSTELLATE_PORT`, an empty
  `CONSTELLATE_TOKEN`, ...) explicitly, so that a `.env` cannot turn the desktop app's engine into a server
  (`apps/desktop/engine.cjs`).
- The interface's server forwards /api and /auth in `apps/web/proxy.ts`, not with rewrites in `next.config.ts`: a
  rewrite cannot add the engine's token to each request.
