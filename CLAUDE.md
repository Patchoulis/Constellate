# Constellate: working rules

Constellate is a desktop app that gathers research papers (conference lists, arXiv), embeds and clusters them,
and helps a researcher find what matters: communities, overviews with citations, acceptance tiers, authors'
declarations, reviewers' ratings. README.md describes what it does; this file says how to work on it.

## Layout

| Path | What |
| --- | --- |
| `apps/backend` | Python package `rp`: the pipeline and a Flask API on 127.0.0.1:8765 |
| `apps/web` | The interface: Next.js, React, Tailwind, a d3 canvas |
| `apps/desktop` | Electron shell that starts both and shows one window |
| `packages/contracts` | TypeScript types shared by the interface and described by the API |
| `scripts/build.mjs` | `npm run build` / `npm run app` / `npm run dist` |
| `data/` | The library and runs. Never committed. |

## Commands

```
npm run app                      build and open the desktop app
npm test                         every test (turbo: backend pytest + web node tests)
npx turbo run typecheck          TypeScript
cd apps/backend && python -W ignore -m pytest -q
```

## Secrets: never commit them

- API keys live in environment variables only (`ANTHROPIC_API_KEY` at Windows *User* scope). Never write a key,
  token or password into a file in this repository, a test, a fixture, a log line, a commit message or the changelog.
- Never commit `data/`, `.env*`, `*.pem`, `*.key`, or `.claude/settings.local.json`. They are in `.gitignore`;
  do not remove them from it and do not use `git add -f` on them.
- Stage files by name or by folder you have looked at (`git add apps packages ...`), then read
  `git diff --cached --stat` before committing. If something unexpected is staged, stop.
- The hooks in `.githooks/` refuse a commit that contains something shaped like a key. Do not bypass them
  (`--no-verify`). If a hook is wrong, fix the hook.
- If a secret is ever committed: say so at once, do not just delete it in a new commit. The key must be revoked
  and the history rewritten before anything is pushed.

## Every change: test, review, commit, release

1. **Branch** from `main`: `feat/...`, `fix/...`, `chore/...`. `main` only takes pull requests whose checks pass.
2. **Tests** with the change: backend in `apps/backend/tests`, interface logic in `apps/web/tests`. Run both
   suites and the typecheck before committing.
3. **Review**: before a change is called done, an Opus 5.5 agent reviews the code just written, and its real
   findings are fixed.
4. **Changelog**: add the change under `## [Unreleased]` in `CHANGELOG.md` (Added / Changed / Fixed / Security),
   in words a user of the app would understand.
5. **Commit** with a Conventional Commit message (below).
6. **Release**: move the Unreleased entries under a new version heading with today's date, set that version in
   the root `package.json`, commit (`chore(release): vX.Y.Z`), merge, then tag `vX.Y.Z` on `main` and push the
   tag. The Release workflow publishes the GitHub release from the changelog section. Version by SemVer:
   `fix` raises the patch, `feat` the minor, a breaking change the major (while 0.x: the minor).

## Conventional Commits

```
<type>(<scope>): <summary in the imperative, lower case, no full stop>

<why, and what a reader of the diff cannot see>
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`, `revert`.
Scopes: `backend`, `web`, `desktop`, `contracts`, `build`, `release` (optional).
A breaking change has `!` after the type or scope and a `BREAKING CHANGE:` footer.
Pull request titles follow the same form (CI checks it) and pull requests are squash-merged.

## Code

- Explicit state machines (`rp/jobs.py`, `useJob`, `useAsync`), small modules, no repeated code.
- Work that was done is not done again: everything fetched or computed is kept by id in `data/library`.
- Python here is 3.15 on Windows: `transformers.from_pretrained` and sentence-transformers crash it, so models
  are loaded by hand (`rp/embed.py`). Do not "simplify" that back.
- Files in the library are written through `rp/store.py` (atomic replace, lock), never directly.
- OpenReview serves a bot challenge to programs. Do not work around it. Reviews are read only from the page
  the user opens in the reader.
- Anything shown as a fact about a paper that is really an estimate or a program's reading (AI detected,
  declaration flags, ratings) says so where it is shown.
