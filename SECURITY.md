# Security policy

## Supported versions

Constellate is released from `main`. Security fixes go into the newest release only; please update before
reporting, and say which version you are on (the release you installed, or the commit you built).

| Version             | Supported |
| ------------------- | --------- |
| the newest release  | yes       |
| any earlier release | no        |

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately through GitHub: on the repository's **Security** tab, choose **Report a vulnerability**
([direct link](https://github.com/Patchoulis/Constellate/security/advisories/new)). Only the maintainers see the
report. If you cannot use that form, open an issue that says only "I have a security report" and a private channel
will be arranged; put no details in the issue.

A useful report says:

- what an attacker can do, and from which position: a web page open in the user's browser, a page shown in the
  app's embedded reader, a paper source or a crafted PDF, another program or account on the same computer, a
  stranger on the network when the engine is run as a server;
- the version, the operating system, and whether the app was installed or run from source;
- the smallest steps that show it. A proof of concept helps; never include a real API key, and check logs for one.

What to expect: an acknowledgement within 7 days, an assessment within 14, and a fix or a stated plan within 90
days of the report, sooner for anything being exploited. You will be credited in the advisory and the changelog
unless you ask not to be. Please give us that time before publishing.

## What counts

Constellate runs on the user's own computer, holds their API keys, spends their money on model calls, fetches
papers from the internet, parses PDFs nobody vetted, and shows other sites' pages inside its window. In scope, for
example:

- a page in the embedded reader, or in the user's ordinary browser, reaching the app's interface, its engine, the
  user's files or the local network;
- a way to make the app call a language model, or pass the budget the user accepted, without the user's click;
- an API key, the engine's token, a session or the owner's password reaching a log, an error, a page, a URL, a
  child process, or the repository;
- a crafted PDF, web page, conference listing or model answer that crashes the app, exhausts the machine, writes
  outside the data folder, or runs script in the app's window;
- in server mode (`CONSTELLATE_BIND` not loopback): reaching any route without signing in, forging a request as
  the signed-in owner, locking the owner out, or weakening TLS.

Out of scope: another program already running as the same user reading the user's own files (it can read
everything that user can); what a site the user chose to open in the reader does within its own pages; findings
that need a modified build; and the accuracy of what a language model writes (the app shows a model's statement
only beside a quote found in the paper, and says that it is the model's reading).

## How the project guards against these

- Keys and endpoints come only from environment variables or `.env`, which git ignores; commit hooks and CI refuse
  anything shaped like a secret (`.githooks/secrets.txt`), and GitHub's secret scanning with push protection is on.
- Dependencies are checked against the advisory databases on every pull request and every week (`Advisories`
  workflow: `npm audit`, `cargo deny`), Dependabot opens the updates, and CodeQL scans the code.
- Every change reaches `main` through a pull request whose checks must pass; nobody pushes to `main` directly.
- The embedded reader's confinement is tested by running a hostile page inside a real Electron
  (`apps/desktop/reader.electron.cjs`).

`CONTRIBUTING.md` says how to run these checks yourself.
