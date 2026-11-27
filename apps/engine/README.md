# Constellate engine

The Rust side of Constellate: everything the app does that is not drawing it. `src/lib.rs` explains the layout by
subject. This page is about how the engine is started and reached: its HTTP API (`src/api`).

The engine is not a public server. The desktop app starts it on the user's own computer and is the only client it
answers. Every request must prove that, whatever address the engine listens on. It can also run as one person's own
server, used from elsewhere through a browser: then its owner signs in ([Running a server](#running-a-server)).

## Starting it

```sh
cargo run --release -p constellate
```

The engine reads its configuration from environment variables, and from a `.env` file in the working directory if
there is one (`NAME=value` lines, `#` comments, optional quotes and `export`). A variable set in the environment
wins over the same one in `.env`. `.env` is ignored by git; never commit it, since it holds the token.

| Variable                | Default     | What it is                                                                                                                                                                                                                     |
| ----------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `CONSTELLATE_BIND`      | `127.0.0.1` | The IP address to listen on. A loopback address is **local mode**, anything else **network mode**.                                                                                                                             |
| `CONSTELLATE_PORT`      | `0`         | The port. `0` takes any free port; the one taken is reported on the startup line.                                                                                                                                              |
| `CONSTELLATE_TOKEN`     | made up     | The secret every request must carry: at least 32 characters of `A-Z a-z 0-9 - . _ ~ + / =`. Local mode makes one up when it is not set. In network mode it is for programs only, and only with `CONSTELLATE_ALLOW_TOKEN=true`. |
| `CONSTELLATE_USE_HTTPS` | `true`      | `false` allows plain HTTP in network mode, as an explicit and loudly warned opt-out.                                                                                                                                           |
| `CONSTELLATE_TLS_CERT`  |             | A PEM file with the certificate (and its chain).                                                                                                                                                                               |
| `CONSTELLATE_TLS_KEY`   |             | A PEM file with that certificate's private key.                                                                                                                                                                                |
| `CONSTELLATE_ORIGIN`    |             | The interface's exact origin (`http://127.0.0.1:3000`): the only web page whose requests are answered. Required in network mode.                                                                                               |
| `CONSTELLATE_HOST`      |             | The names the engine is reached by, separated by commas (`engine.example.org`). Needed when `CONSTELLATE_BIND` is `0.0.0.0` or `::`.                                                                                           |
| `CONSTELLATE_DATA`      | see below   | The data folder: `settings.json`, the library, the runs.                                                                                                                                                                       |

Network mode only, for the owner's sign-in ([Running a server](#running-a-server)):

| Variable                         | Default                       | What it is                                                                                  |
| -------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------- |
| `CONSTELLATE_OIDC_ISSUER`        | `https://accounts.google.com` | The OpenID Connect provider. Another provider works by changing this.                       |
| `CONSTELLATE_OIDC_CLIENT_ID`     |                               | The OAuth client registered with the provider for this server. Turns provider sign-in on.   |
| `CONSTELLATE_OIDC_CLIENT_SECRET` |                               | That client's secret.                                                                       |
| `CONSTELLATE_OWNER_EMAIL`        |                               | The owner's address: the one account allowed in. Required with a provider.                  |
| `CONSTELLATE_ALLOW_PASSWORD`     | `false`                       | `true` lets the owner sign in with a password, set once with a setup code printed at start. |
| `CONSTELLATE_ALLOW_TOKEN`        | `false`                       | `true` lets programs use `CONSTELLATE_TOKEN` instead of signing in.                         |

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
- The owner signs in, and every route but signing in needs their session ([Running a server](#running-a-server)).
  `CONSTELLATE_ORIGIN` must be given (https while HTTPS is on), and at least one way to sign in.
- `CONSTELLATE_TOKEN` is accepted only with `CONSTELLATE_ALLOW_TOKEN=true`, for programs (scripts, `curl`); it must
  then be at least 32 characters long (`openssl rand -hex 32` makes a good one). Set without the switch, it is a
  refusal to start, so that nobody believes a token protects a server that does not take it.
- A client that sends wrong tokens is made to wait: after 10 within ten minutes, that address is answered `429` for
  the rest of the ten minutes, even with the right token. An IPv6 /64 network as a whole is allowed 200, so that a
  guesser who takes a new address for every guess is stopped too, while one guessing neighbour does not lock out
  the others. Clients behind one shared address (a NAT, a proxy) share its count.
- One remote address may hold at most 32 connections, and all of them together 512.
- `CONSTELLATE_USE_HTTPS=false` turns HTTPS off. The engine then prints a multi-line warning on standard error at
  start, and `GET /api/status` lists the same warnings so that the interface can show them. Use it only on a network
  you trust completely, or behind something else that does TLS (then keep `CONSTELLATE_ORIGIN` https: the sign-in
  cookie stays `Secure`). With an `http://` origin the cookie loses `Secure` and its `__Host-` prefix, which is one
  more warning.

## Running a server

One person can run the engine on a machine of their own and use it from anywhere through a browser. The server has
one owner, and nobody else gets in.

1. **Addresses.** Choose the address people open, for example `https://papers.example.org`, and set it as
   `CONSTELLATE_ORIGIN`. If the interface is served by something in front of the engine, that front must forward
   both `/api/` and `/auth/` to the engine, with `Host` set to a name in `CONSTELLATE_HOST`. Set
   `CONSTELLATE_BIND` (for example `0.0.0.0`), `CONSTELLATE_HOST`, `CONSTELLATE_TLS_CERT` and `CONSTELLATE_TLS_KEY`.
2. **Sign-in with Google.** In the Google Cloud console, under APIs & Services, Credentials, create an OAuth client
   ID of type "Web application". Add one authorised redirect URI: `https://<host>/auth/callback`, exactly the origin
   above followed by `/auth/callback`. Set `CONSTELLATE_OIDC_CLIENT_ID` and `CONSTELLATE_OIDC_CLIENT_SECRET` to what
   Google shows, and `CONSTELLATE_OWNER_EMAIL` to your Google address as Google shows it. The address is compared
   exactly, except that the part after `@` may be in any case; only an address Google has verified is accepted.
   Another OpenID Connect provider works the same way with `CONSTELLATE_OIDC_ISSUER` set to its issuer.
3. **A password instead, or as well.** Set `CONSTELLATE_ALLOW_PASSWORD=true`. At the first start without a password,
   the engine prints a setup code on standard error. Within 15 minutes, open `<origin>/auth/`, enter the code and
   choose a password: at least 12 characters, not your email address, not one of the most common passwords or a word
   with a few digits, and not one character or pattern repeated. The code works once; a restart prints a new one
   while no password is set. The password is kept only as an Argon2id hash (64 MiB, 3 passes, 1 lane) in
   `<data>/owner-password`, readable by the engine's user alone on Unix (on Windows it has the data folder's
   permissions: keep that folder your own). To choose a new password, stop the engine, delete that file, and start
   it again.
4. **Start it**, and keep all of these in the environment or in `.env`, never in a file that is committed.

With neither Google nor a password configured, the engine refuses to start across a network and says what to set.

### What signing in gives

A session: a random 256-bit id in the cookie `__Host-constellate` (`HttpOnly; Secure; SameSite=Strict; Path=/`),
kept in the engine's memory only, so a restart signs everyone out. It ends after 2 hours without a request, 12 hours
after sign-in whatever happens, at sign-out, at "sign out on every device", or when the same browser signs in again
(every sign-in issues a new id). At most 32 sessions are kept.

Every request that changes something must also carry `Origin` (the interface's exact origin) and the session's own
anti-forgery token in `X-Constellate-Csrf`, which the interface reads from `GET /auth/session`. Without both it is
answered `403`.

Sign-in attempts are limited: 30 from one address in ten minutes; wrong passwords and setup codes 10 from one
address, 100 from an IPv6 /64 and 1000 from everyone together, after which password sign-in waits for the rest of
the ten minutes (Google sign-in does not). A wrong password, a wrong setup code and a way of signing in that is not
offered all get the same answer and take as long. A signed-in owner is never held back by someone else's wrong
tokens from the same address.

**Behind a proxy** (the interface's own server forwarding `/api/` and `/auth/`, or a TLS terminator), every request
reaches the engine from the proxy's address, so all these limits count everyone together: a stranger's attempts can
then make sign-in wait for the owner too. The engine does not yet read `X-Forwarded-For`; until it does, prefer
reaching the engine directly (it serves the sign-in page itself), or let the proxy limit requests per client.

### The settings the server owns

In network mode `GET /api/status` says `"mode": "network"` and `"server_owned": ["embedding_model"]`: the embedding
model is the library's, and changing it from a window would embed the whole library again. `PUT /api/settings`
refuses to change it (`403`); it is changed in the server's own `settings.json`.

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
3. on this computer, carry the token: `Authorization: Bearer <token>`. It is compared in constant time. Across a
   network, carry the owner's session cookie instead (and, if it changes something, `Origin` and
   `X-Constellate-Csrf`), unless it is one of the sign-in routes; or the token, if `CONSTELLATE_ALLOW_TOKEN=true`;
4. have a body of at most 4 MB, sent within 30 seconds, with its headers (HTTP/1) within 10.

Below requests, a connection through which nothing has moved for 30 seconds is closed, whatever HTTP it speaks or
has yet to say it speaks; HTTP/2 connections are pinged and closed when the peer stops answering.

```sh
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:52817/api/status
```

A refused request is answered `401` (no token or a wrong one: the same answer for both; across a network, not signed
in), `403` (another address or another site, or a change without `Origin` or the session's token), `405` (a method
the route does not take), `408` (too slow), `413` (too large) or `429` (too many wrong tokens or sign-in attempts),
with a JSON body `{ "error": "..." }`. Across a network, a browser that opens a page without being signed in is sent
to `/auth/` instead. (A request too malformed to be HTTP at all is answered by
the HTTP library itself, before the engine sees it.) Every answer carries `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`,
`Referrer-Policy: no-referrer`, a `Content-Security-Policy` that allows nothing (the sign-in page's allows its own
script and style sheet, and nothing from elsewhere), `Strict-Transport-Security` for
HTTPS across a network, and no CORS header: the interface reaches the engine through its own origin (the
web build forwards `/api`), so no other origin may ever read an answer.

### Routes

| Route                     | What it answers                                                                                                                                                                                                                              |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/status?since=N` | The background job: `id`, `kind`, `state`, `result`, `error`, its `log` from line N on, `total` lines, `warnings` about how the engine listens, `mode` (`local` or `network`) and `server_owned`, the settings the interface may not change. |
| `GET /api/settings`       | The settings (`Settings` in `packages/contracts`).                                                                                                                                                                                           |
| `PUT /api/settings`       | Changes some: a JSON object of the ones to change. A bad value changes nothing and is answered `400` with what is wrong; a server-owned one `403`.                                                                                           |

Across a network, also (the first six answer anyone; the last two need the session):

| Route                           | What it does                                                                                                                               |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `GET /auth/`                    | The sign-in page (and its `sign-in.js` and `sign-in.css`).                                                                                 |
| `GET /auth/session`             | `signed_in`, the `methods` offered, `provider`'s name, `setup` (a password is still to be set); and to the owner only, `email` and `csrf`. |
| `GET /auth/start`               | Off to the provider's sign-in.                                                                                                             |
| `GET /auth/callback`            | Back from the provider: signed in, or sent to the sign-in page.                                                                            |
| `POST /auth/password`           | `{ "password": ... }`: signs in.                                                                                                           |
| `POST /auth/setup`              | `{ "code": ..., "password": ... }`: sets the first password with the setup code, and signs in.                                             |
| `POST /auth/signout`            | Ends this browser's session.                                                                                                               |
| `POST /auth/signout-everywhere` | Ends every session, on every device.                                                                                                       |

Errors are `{ "error": "..." }` with `400` for a bad request, `404` for nothing there, `409` when a job is already
running (naming it) and `413` for a body that is too large.

Adding a route is a line in `table()` in `src/api/routes.rs`; it is served behind the same checks as the others,
and a test walks that list to see that every route is refused to whoever has not signed in.
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
