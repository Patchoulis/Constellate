# Constellate: working rules

Constellate is a desktop app that gathers research papers (conference lists, arXiv), embeds and clusters them,
and helps a researcher find what matters: communities, overviews with citations, acceptance tiers, authors'
declarations, reviewers' ratings. README.md describes what it does, CONTRIBUTING.md how changes are made and
the style they follow. This file is the short version, plus what an automated assistant must not get wrong.

## Layout

| Path                 | What                                                                |
| -------------------- | ------------------------------------------------------------------- |
| `apps/backend`       | Python package `rp`: the pipeline and a Flask API on 127.0.0.1:8765 |
| `apps/web`           | The interface: Next.js, React, Tailwind, a d3 canvas                |
| `apps/desktop`       | Electron shell, first-start setup of the app's Python, installer    |
| `packages/contracts` | TypeScript types shared by the interface and described by the API   |
| `scripts/build.mjs`  | `npm run build` / `npm run app` / `npm run dist`                    |
| `scripts/pr.mjs`     | `npm run pr`: everything a pull request must pass                   |
| `data/`              | The library and runs. Never committed.                              |

## Commands

```
npm run app            build and open the desktop app
npm run pr             format check, lint, types, every test (what CI runs)
npm run pr -- --fix    the same, formatting and fixing what it can
npm run dist           build the Windows installer into dist/
```

## Secrets: never commit them

- API keys live in environment variables only (`ANTHROPIC_API_KEY`). Never write a key, token or password into a
  file in this repository, a test, a fixture, a log line, a commit message or the changelog.
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
   tag. The Release workflow builds the installer and publishes the GitHub release from the changelog section.
   Version by SemVer: `fix` raises the patch, `feat` the minor, a breaking change the major (while 0.x: the minor).

## Commit messages

```
<type>(<scope>): <summary in the imperative, lower case, no full stop>

<why, and what a reader of the diff cannot see>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.
Scopes: `backend`, `web`, `desktop`, `contracts`, `build`, `release` (optional).

**A message describes the change and nothing else.** No mention of who or what wrote it: no assistant or tool
names, no `Co-Authored-By` lines for assistants, no "generated with". The same goes for pull request titles and
descriptions, the changelog and code comments. The `commit-msg` hook rejects such lines.

## Style (the full list is in CONTRIBUTING.md)

- Formatting is the formatter's: Prettier for TypeScript, JSON, Markdown, YAML; `ruff format` for Python.
- `ruff check` clean, `tsc` clean. A caught error is named `error` in both languages.
- Explicit state machines (`rp/jobs.py`, `useJob`, `useAsync`, `apps/desktop/setup.cjs`), small modules, no
  repeated code.
- Work that was done is not done again: everything fetched or computed is kept by id in `data/library`.
- Files in the library are written through `rp/store.py` (atomic replace, lock), never directly.
- Anything shown as a fact about a paper that is really an estimate or a program's reading (AI detected,
  declaration flags, ratings) says so where it is shown.

## Things that look wrong but are deliberate

- Models are loaded by hand in `rp/embed.py` (config, then weights) rather than with `from_pretrained`, which
  crashes on the newest Python on Windows. Do not "simplify" that back.
- OpenReview serves a bot challenge to programs. Do not work around it. Reviews are read only from the page the
  user opens in the reader.
- The installed app does not use the machine's Python: it sets up its own on first start (`setup.cjs`).
