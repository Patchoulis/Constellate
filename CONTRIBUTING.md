## Contributing to Constellate

Thanks for your interest in improving Constellate!

**No contribution is too small and all contributions are valued.** This document is a guide to the process, not a
set of hurdles: fill in what you can and ask about the rest.

### Ways to contribute

1. **By opening an issue.** If you believe you have found a bug, or a paper, conference or format that is read
   wrongly, an issue is the way to report it.
2. **By adding context** to existing issues: screenshots, the paper or page that shows the problem, a snippet.
3. **By resolving issues**: showing that a report is not a problem after all or, more often, opening a pull
   request that fixes it in a concrete and reviewable way.

### Submitting a bug report

The issue tracker offers a short form. The most useful things to include are:

- the version of Constellate and your platform
- what you selected (conferences, years or date range) and the relevant settings
- the paper, PDF or page the problem shows on, when it is about how something was read
- concrete steps to reproduce it, as small as you can make them

Never paste an API key, and check logs for one before attaching them.

### Submitting a feature request

Describe what you would like and what you are trying to find out with it, as concretely as you can. If another
tool does it, say which.

### How a change reaches `main`

Nobody pushes to `main`, the maintainers included: the branch's rules (`.github/rulesets/main.json`, imported
under Settings > Rules) refuse it. Every change goes this way:

1. **Branch** from `main` (or fork the repository and branch there): `feat/<what>`, `fix/<what>`, `docs/<what>`,
   `chore/<what>`.
2. **Commit** as the hooks ask (Conventional Commits; nothing shaped like a secret), and run `npm run pr` before
   pushing: it is everything CI runs.
3. **Open a pull request** into `main`. Its title is a Conventional Commit, because it becomes the commit on `main`
   when the pull request is squashed. Fill in the template; a draft is welcome early.
4. **The checks must pass**: `Checks (npm run pr)`, `Engine on Linux`, `Conventional title`, `No secrets`,
   `Advisories` and `Dependency review` are required; `CodeQL` reports what it finds on the pull request. The branch
   must be up to date with `main`, and every review conversation resolved.
5. **Merge** by squashing (one change, one commit on `main`). A long-lived branch that gathers many reviewed
   changes is merged with a merge commit instead, so that its history is kept.

Security reports do not go through issues or pull requests: see `SECURITY.md`.

### Resolving an issue

Pull requests are how concrete changes are made to the code, documentation and dependencies.

Before a large change, it is usually a good idea to open an issue describing it first, to get feedback before the
work is done. For a larger feature, a draft pull request shows others what is in progress.

Once after cloning, get everything the project is built with (Rust, its formatter and linter, the npm packages,
and the git hooks that check commit messages and refuse anything shaped like a secret):

```sh
npm run setup
```

Please make sure that this passes before you ask for review; it is exactly what CI runs:

```sh
npm run pr
```

It checks formatting (`cargo fmt`, `prettier`), lints (`clippy` with warnings denied, and the naming rule below),
type-checks, builds the interface (the shell's tests start its server) and runs every test. `npm run pr -- --fix`
formats and fixes what it can.

#### Style

- **Formatting is the formatter's job.** Rust is laid out by `cargo fmt` (`rustfmt.toml`); TypeScript, JSON,
  Markdown and YAML by Prettier (`.prettierrc.json`: tabs, 140 columns). Do not format by hand and do not argue
  with the tool.
- **Lint warnings are errors.** `cargo clippy -- -D warnings` must be clean and `tsc` must pass with no errors.
- **A caught error is named `error`**: `Err(error)`, `catch (error)`. Not `e`, `err`, `ex` or `problem`.
- **State is explicit.** Anything with stages (a job, a request, a window starting up) is a named set of states
  with its transitions written down in one place, not a handful of booleans.
- **Say what something is in the words of the subject**, in names, comments and messages: a paper, a community,
  a run; not "item", "data", "handler".
- **A comment says why**, or what a reader cannot see from the code. It does not restate the line below it.
- **Estimates are labelled as estimates** wherever they are shown (AI detected, declaration flags, ratings).

#### Adding tests

A change to code either adds something or fixes something broken; in both cases the pull request should include
tests so that it stays that way.

- **Unit tests** for functions with one specific job: tests in the same file in `apps/engine` (Rust),
  `apps/web/tests`, `apps/desktop/*.test.cjs` and `scripts/*.test.mjs` (node's test runner). `npm run pr` runs
  every one of these files it finds.
- **Tests through the API** for behaviour that crosses modules: the engine's tests call its routes as the
  interface does (`apps/engine/src/api`, and each feature's `routes.rs` in `apps/engine/src/features`), and the
  shell's (`apps/desktop/servers.test.cjs`) start the real engine and the interface's server and check what passes
  between them: the token reaches the engine, and never a page.

Tests never touch the network or the real `data/` folder. A parser that reads papers or pages gets a test for
every layout that was once read wrongly. The shell's server tests need the interface's build (`npm run pr` makes
it) and the engine's program (`cargo test` leaves one); without them they are skipped, except in CI, where they
fail instead.

Run one test with `cargo test -p constellate name`, `npx tsx --test apps/web/tests/graph.test.ts` or
`node --test apps/desktop/reader.test.cjs`.

To work on the interface with hot reload, `npm run dev` starts the engine and `next dev` on http://localhost:3210
(`scripts/dev.mjs`); `RP_DEV_URL=http://localhost:3210 npm start -w @rp/desktop` opens the desktop window on it.

The shapes of secrets the git hooks and CI refuse are in `.githooks/secrets.txt`, one regular expression per line.
A provider with a new key format gets a line there, and a sample in `scripts/hooks.test.mjs`.

#### Dependencies and advisories

`npm run pr` ends with the advisory checks (they need the network; `npm run pr -- --only=advisories` runs only
them). CI runs them as a workflow of their own, Advisories, on every pull request and every Monday:

- `npm audit --omit=dev --audit-level=moderate` for what ships, and `npm audit --audit-level=high` for everything,
  the build tools included (they only ever read this repository's own files).
- `cargo deny check advisories licenses sources` for the engine's crates, with `deny.toml`: RustSec advisories, the
  licences the installer may carry, and crates only from crates.io. `npm run setup` installs cargo-deny.

When one fails, move the package to the smallest version that fixes it. Some fixes need more than a bump:

- **An npm package that comes with another one** is moved with an entry under `overrides` in the root
  `package.json`, which takes no comments, so each is explained here:
  - `minimatch@>=7.0.0 <11.0.1` -> `11.0.1`: GHSA-g3hv-75wg-m249, in electron-builder's packaging (app-builder-lib and
    @electron/universal ask for 10.x and 9.x, which have no fixed release). Remove it once electron-builder asks for
    11.0.1 or later itself.
- **A crate** is moved with `cargo update -p <crate> --precise <version>`. The workspace's `rust-version` (1.88) is
  the oldest Rust the fixed releases need; `clippy.toml` keeps clippy's suggestions at 1.85 until the code is moved on.
- **An advisory that cannot be fixed yet** and does not reach Constellate goes in `deny.toml`'s `ignore` with the
  reason and what would remove it. Look at those again when the crate that brings them in has a new major version.

Dependabot (`.github/dependabot.yml`) opens the pull requests for all of this, grouped by ecosystem, once a week.

#### Commits

Keep changes logically grouped within commits. There is no limit to the number of commits in a pull request; if
some are "checkpoints" that do not represent one logical change, squash them together.

A commit message describes the change and nothing else: no tool credits and no "generated by" or co-author lines
for assistants.

##### Conventional Commits

Commit messages and pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<optional scope>): <summary in the imperative, lower case, no full stop>
```

**Examples:**

- `feat(web): show or dim papers by acceptance tier`
- `fix(engine): read checklist answers written [N/A]`
- `refactor: split the reviews reader into blocks`
- `docs(readme): describe the reader`

**Types:**

- `feat`: a new feature
- `fix`: a bug fix
- `docs`: documentation only
- `style`: changes that do not affect meaning (formatting)
- `refactor`: neither fixes a bug nor adds a feature
- `perf`: makes something faster or smaller
- `test`: adding or updating tests
- `build`, `ci`: the build script, dependencies, workflows
- `chore`: maintenance, including releases
- `revert`: undoes an earlier commit

Scopes: `engine`, `web`, `desktop`, `contracts`, `build`, `release`. A breaking change has `!` after the type or
scope and a `BREAKING CHANGE:` footer.

#### Opening the pull request

Opening a pull request presents a template. Fill in what you can. Add your change to `CHANGELOG.md` under
`## [Unreleased]`, in words a user of the app would understand. Pull requests are squash-merged, so the title
becomes the commit message.

#### Discuss and update

You will probably get feedback or requests for changes. This is a normal part of the process, not a verdict.

#### Reviewing pull requests

Anyone is welcome to review any pull request. Reviews must be helpful and aimed at improving the contribution, not
simply blocking it; if you think a change should not be merged, explain why, and be open to having your mind
changed.

Focus first on what matters most:

1. Does this change make sense for Constellate?
2. Does it make Constellate better, even if only incrementally?
3. Are there clear bugs or larger problems?
4. Are the commit messages readable and correct? If something breaks, is that clear?

Only **incremental** improvement is needed to land a pull request: it has to be better than what is there, not
perfect. When changes are necessary, _request_ them, do not _demand_ them. Mark small non-essential requests as
such: `Nit: rename foo to bar. Not blocking.`

### Releases

Every merged change is released. The maintainer moves the Unreleased entries of `CHANGELOG.md` under a new
version, sets that version in `package.json`, and pushes the tag `vX.Y.Z` on a commit of `main`; the Release
workflow refuses a tag anywhere else, builds the installer and publishes it with that version's notes and its
SHA-256 checksum.

_The structure of this guide is adapted from the [Ream contributing guide](https://github.com/ReamLabs/ream/blob/master/CONTRIBUTING.md),
itself adapted from [Reth's](https://github.com/paradigmxyz/reth/blob/main/CONTRIBUTING.md)._
