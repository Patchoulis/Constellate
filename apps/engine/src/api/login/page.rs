//! The sign-in page, served by the engine itself at `/auth/`: plain HTML, one small script and one style sheet
//! of its own, nothing from anywhere else. Its Content-Security-Policy allows exactly that: scripts, styles and
//! requests from this origin only, no inline script, no frames, no forms sent elsewhere.
//!
//! The script asks `GET /auth/session` what to show (the provider's button, the password form, the setup form,
//! or that the owner is signed in) and sends the forms as JSON. Without the script, a form is still sent in the
//! body of a POST (refused, since it is not JSON), never in an address where a password would be logged.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

/// The page's script and style sheet.
pub const SCRIPT: &str = "/auth/sign-in.js";
pub const STYLE: &str = "/auth/sign-in.css";

/// Everything from this origin only; inline script and style are not allowed, since none is used.
pub const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
                          img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

const HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Constellate: sign in</title>
<link rel="stylesheet" href="/auth/sign-in.css">
<script src="/auth/sign-in.js" defer></script>
</head>
<body>
<main>
<h1>Constellate</h1>
<p id="said" role="status" aria-live="polite">Loading…</p>
<section id="signed-in" hidden>
<p><a href="/">Open Constellate</a></p>
<button type="button" id="sign-out">Sign out</button>
<button type="button" id="sign-out-everywhere">Sign out on every device</button>
</section>
<section id="provider" hidden>
<a id="provider-link" class="button" href="/auth/start">Sign in</a>
</section>
<form id="password" method="post" action="/auth/password" hidden>
<label>Password <input type="password" name="password" autocomplete="current-password" required></label>
<button type="submit">Sign in</button>
</form>
<form id="setup" method="post" action="/auth/setup" hidden>
<p>No password is set yet. Enter the setup code the server printed when it started (it works for 15 minutes,
once), and choose a password of at least 12 characters.</p>
<label>Setup code <input name="code" autocomplete="one-time-code" required></label>
<label>New password <input type="password" name="password" autocomplete="new-password" minlength="12" required></label>
<button type="submit">Set the password and sign in</button>
</form>
</main>
</body>
</html>
"#;

const JS: &str = r#""use strict";
const said = document.getElementById("said");
const tell = (text) => { said.textContent = text; };
const show = (id) => { document.getElementById(id).hidden = false; };
const WORDS = {
	refused: "Sign-in was refused.",
	wait: "Too many attempts: try again in a few minutes.",
	unavailable: "The sign-in provider could not be reached: try again later.",
};
let csrf = null;

async function send(path, body) {
	const headers = { "Content-Type": "application/json" };
	if (csrf) headers["X-Constellate-Csrf"] = csrf;
	const response = await fetch(path, { method: "POST", headers, body: JSON.stringify(body), credentials: "same-origin" });
	const told = await response.json().catch(() => ({}));
	if (!response.ok) throw new Error(told.error || "The server answered " + response.status);
	return told;
}

function form(id, read) {
	const element = document.getElementById(id);
	element.addEventListener("submit", async (event) => {
		event.preventDefault();
		tell("Signing in…");
		try {
			await send(element.getAttribute("action"), read(element));
			location.assign("/");
		} catch (error) {
			tell(error.message);
		}
	});
}

async function start() {
	const word = new URLSearchParams(location.search).get("said");
	const session = await (await fetch("/auth/session", { credentials: "same-origin" })).json();
	if (session.signed_in) {
		csrf = session.csrf;
		tell(session.email ? "Signed in as " + session.email + "." : "Signed in.");
		show("signed-in");
		for (const [id, path] of [["sign-out", "/auth/signout"], ["sign-out-everywhere", "/auth/signout-everywhere"]]) {
			document.getElementById(id).addEventListener("click", async () => {
				try {
					await send(path, {});
					location.reload();
				} catch (error) {
					tell(error.message);
				}
			});
		}
		return;
	}
	// only the page's own words: what the address says is never shown as it is
	tell(Object.hasOwn(WORDS, word) ? WORDS[word] : "Sign in to use this server.");
	if (session.methods.includes("provider")) {
		document.getElementById("provider-link").textContent = "Sign in with " + (session.provider || "the provider");
		show("provider");
	}
	if (session.methods.includes("password")) {
		if (session.setup) {
			show("setup");
			form("setup", (element) => ({ code: element.elements.code.value, password: element.elements.password.value }));
		} else {
			show("password");
			form("password", (element) => ({ password: element.elements.password.value }));
		}
	}
}

start().catch(() => tell("The server could not be reached."));
"#;

const CSS: &str = "body { font: 16px/1.5 system-ui, sans-serif; margin: 0; min-height: 100vh; display: grid; \
place-items: center; background: #0f1115; color: #e8e8ea; }
main { width: min(24rem, 92vw); display: flex; flex-direction: column; gap: 1rem; }
h1 { font-size: 1.4rem; margin: 0; }
form, section { display: flex; flex-direction: column; gap: 0.75rem; }
[hidden] { display: none; }
label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.9rem; }
input { font: inherit; padding: 0.5rem; border-radius: 0.375rem; border: 1px solid #3a3d45; background: #181b21; \
color: inherit; }
button, .button { font: inherit; padding: 0.55rem 0.9rem; border-radius: 0.375rem; border: 1px solid #3a3d45; \
background: #2a5bd7; color: #fff; text-align: center; text-decoration: none; cursor: pointer; }
a { color: #8fb3ff; }
";

/// A page under [`POLICY`].
fn served(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(POLICY));
    response
}

pub async fn page() -> Response {
    served("text/html; charset=utf-8", HTML)
}

pub async fn script() -> Response {
    served("text/javascript; charset=utf-8", JS)
}

pub async fn style() -> Response {
    served("text/css; charset=utf-8", CSS)
}

/// What the browser is shown when it comes back from the provider signed in: a page that goes on to the
/// interface by itself. Not a redirect: the provider's redirect started on its own site, and a browser keeps a
/// `SameSite=Strict` cookie back from every request of a chain another site started, so the interface would
/// load as if nobody were signed in. A navigation this page starts is this site's own.
pub fn signed_in() -> Response {
    const SIGNED_IN: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="0;url=/">
<meta name="referrer" content="no-referrer">
<title>Constellate: signed in</title>
</head>
<body>
<p>Signed in. <a href="/">Open Constellate</a>.</p>
</body>
</html>
"#;
    served("text/html; charset=utf-8", SIGNED_IN)
}
