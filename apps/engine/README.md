# Constellate engine

The Rust side of Constellate: everything the app does that is not drawing it. `src/lib.rs` explains the layout by
subject. This page is about how the engine is started and reached: its HTTP API (`src/api`).

The engine is not a public server. The desktop app starts it on the user's own computer and is the only client it
answers. Every request must prove that, whatever address the engine listens on.

## Starting it

```sh
cargo run --release -p constellate
```

The engine reads its configuration from environment variables, and from a `.env` file in the working directory if
there is one (`NAME=value` lines, `#` comments, optional quotes and `export`). A variable set in the environment
wins over the same one in `.env`. `.env` is ignored by git; never commit it, since it holds the token.

| Variable                | Default     | What it is                                                                                                                                                                        |
| ----------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CONSTELLATE_BIND`      | `127.0.0.1` | The IP address to listen on. A loopback address is **local mode**, anything else **network mode**.                                                                                |
| `CONSTELLATE_PORT`      | `0`         | The port. `0` takes any free port; the one taken is reported on the startup line.                                                                                                 |
| `CONSTELLATE_TOKEN`     | made up     | The secret every request must carry: at least 32 characters of `A-Z a-z 0-9 - . _ ~ + / =`. Local mode makes one up when it is not set; network mode refuses to start without it. |
| `CONSTELLATE_USE_HTTPS` | `true`      | `false` allows plain HTTP in network mode, as an explicit and loudly warned opt-out.                                                                                              |
| `CONSTELLATE_TLS_CERT`  |             | A PEM file with the certificate (and its chain).                                                                                                                                  |
| `CONSTELLATE_TLS_KEY`   |             | A PEM file with that certificate's private key.                                                                                                                                   |
| `CONSTELLATE_ORIGIN`    |             | The interface's exact origin (`http://127.0.0.1:3000`): the only web page whose requests are answered.                                                                            |
| `CONSTELLATE_HOST`      |             | The names the engine is reached by, separated by commas (`engine.example.org`). Needed when `CONSTELLATE_BIND` is `0.0.0.0` or `::`.                                              |
| `CONSTELLATE_DATA`      | see below   | The data folder: `settings.json`, the library, the runs.                                                                                                                          |

An empty value counts as unset. A value that is wrong stops the engine at once with a message on standard error
that says what to do; messages never repeat a value (the token could be one).

### Local mode: a loopback address

Traffic to `127.0.0.1` or `::1` never leaves the computer, so there is nothing on a wire for anyone to read and TLS
is not needed (if a certificate and key are given, HTTPS is used anyway). The token is still required: any other
program, and any web page open in a browser, can reach a loopback port too. When `CONSTELLATE_TOKEN` is not set,
the engine makes one from the operating system's random source (256 bits) and reports it on the startup line.

### Network mode: any other address

The engine can then be reached from other machines, so:

- HTTPS is required: `CONSTELLATE_TLS_CERT` and `CONSTELLATE_TLS_KEY` must name a certificate and its key. A missing
  certificate is a refusal to start, never a quiet fall back to plain HTTP.
- `CONSTELLATE_TOKEN` must be given, at least 32 characters long (`openssl rand -hex 32` makes a good one).
- A client that sends wrong tokens is made to wait: after 10 within ten minutes, that address is answered `429` for
  the rest of the ten minutes, even with the right token. An IPv6 /64 network as a whole is allowed 200, so that a
  guesser who takes a new address for every guess is stopped too, while one guessing neighbour does not lock out
  the others. Clients behind one shared address (a NAT, a proxy) share its count.
- One remote address may hold at most 32 connections, and all of them together 512.
- `CONSTELLATE_USE_HTTPS=false` turns HTTPS off. The engine then prints a multi-line warning on standard error at
  start, and `GET /api/status` lists the same warnings so that the interface can show them. Use it only on a network
  you trust completely, or behind something else that does TLS.

## The startup line

Once listening, the engine writes exactly one line to standard output, as JSON:

```json
{ "listening": "http://127.0.0.1:52817", "scheme": "http", "address": "127.0.0.1", "port": 52817, "token": "<64 hex characters>" }
```

`token` is there only when the engine made the token up; a token that was given is never repeated. Nothing else is
written to standard output; everything meant for a person goes to standard error.

The line is meant for the process that started the engine, read over a pipe. **It must not be written anywhere else**
(a log file, the console of a terminal someone else can see): with the token, anyone can act as the app.

## Stopping it

The engine stops when its standard input closes, on Ctrl-C, and on SIGTERM (Unix). The desktop shell keeps the engine's standard input
open for as long as it lives, so a shell that closed or crashed leaves no engine behind. Whoever starts the engine
some other way must give it a standard input that stays open (a pipe); started with `< /dev/null`, it stops at once.

## Calling it

Every request, on every path, must:

1. be addressed to the engine: `Host` is the address it listens on, or `localhost` in local mode, or one of the
   names in `CONSTELLATE_HOST`, with the port (defeats DNS rebinding);
2. if it carries an `Origin` header (a browser page sent it), come from exactly `CONSTELLATE_ORIGIN`. Without one
   configured, local mode accepts pages served from this computer and network mode accepts no page at all;
3. carry the token: `Authorization: Bearer <token>`. It is compared in constant time;
4. have a body of at most 4 MB, sent within 30 seconds, with its headers (HTTP/1) within 10.

Below requests, a connection through which nothing has moved for 30 seconds is closed, whatever HTTP it speaks or
has yet to say it speaks; HTTP/2 connections are pinged and closed when the peer stops answering.

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:52817/api/status
```

A refused request is answered `401` (no token or a wrong one: the same answer for both), `403` (another address
or another site), `405` (a method the route does not take), `408` (too slow), `413` (too large) or `429` (too many
wrong tokens), with a JSON body `{ "error": "..." }`. (A request too malformed to be HTTP at all is answered by
the HTTP library itself, before the engine sees it.) Every answer carries `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`,
`Referrer-Policy: no-referrer`, a `Content-Security-Policy` that allows nothing, `Strict-Transport-Security` for
HTTPS across a network, and no CORS header: the interface reaches the engine through its own origin (the
web build forwards `/api`), so no other origin may ever read an answer.

### Routes

| Route                     | What it answers                                                                                                                                     |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/status?since=N` | The background job: `id`, `kind`, `state`, `result`, `error`, its `log` from line N on, `total` lines, and `warnings` about how the engine listens. |
| `GET /api/settings`       | The settings (`Settings` in `packages/contracts`).                                                                                                  |
| `PUT /api/settings`       | Changes some: a JSON object of the ones to change. A bad value changes nothing and is answered `400` with what is wrong.                            |

Errors are `{ "error": "..." }` with `400` for a bad request, `404` for nothing there, `409` when a job is already
running (naming it) and `413` for a body that is too large.

Adding a route is a line in `routes()` in `src/api/routes.rs`; it is served behind the same checks as the others.
A route that takes a name in its path (a run) checks it with `routes::one_name` first.

## Where the data is kept

Unless `CONSTELLATE_DATA` says otherwise, the engine keeps its data in the system's folder for a program's own
local data, never in the folder it was started from and never in a folder that syncs:

| System  | Folder                                                        |
| ------- | ------------------------------------------------------------- |
| Windows | `%LOCALAPPDATA%\Constellate`                                  |
| macOS   | `~/Library/Application Support/Constellate`                   |
| Linux   | `$XDG_DATA_HOME/constellate`, or `~/.local/share/constellate` |

The library is a database that is open while the app runs. A sync client (OneDrive, Dropbox, iCloud) that uploads
its files mid-write can leave them inconsistent, so do not point `CONSTELLATE_DATA` into a synced folder.
