//! How the engine listens, read from the environment (and from a `.env` file in the working directory).
//!
//! There are two modes, decided in one place ([`Mode::of`]) from the address the engine binds to:
//!
//! * **Local**: a loopback address (`127.0.0.1`, `::1`). Traffic never leaves the computer, so there is nothing
//!   on the wire for anyone to read and TLS is not needed (it is used if a certificate is given). A token is
//!   still required, because other programs and web pages on the same computer can reach a loopback port too;
//!   if none is given, one is made from the operating system's random source and handed to the parent process.
//! * **Network**: any other address. The engine is then reachable from other machines, so it needs HTTPS with a
//!   certificate and key, and its one owner signs in ([`super::login`]): with Google (OpenID Connect), with a
//!   password, or either. The interface's origin must be given, since it is where the owner signs in and the
//!   only page whose requests are answered. A token for programs (`CONSTELLATE_TOKEN`, 32+ characters) is
//!   accepted only when the owner asks for it (`CONSTELLATE_ALLOW_TOKEN=true`). Plain HTTP is possible only as an
//!   explicit opt-out (`CONSTELLATE_USE_HTTPS=false`), which is warned about loudly at start and told to the
//!   interface ([`Listen::warnings`]). A missing certificate is a refusal to start, never a quiet fall back to
//!   plain HTTP.
//!
//! The outcome is a [`Listen`]: it can only be made by [`Listen::from_environment`], which refuses every insecure
//! combination, and the plain-HTTP transports and the cookie without `Secure` carry a proof ([`Loopback`],
//! [`OptOut`]) that only this module can make. No value read here is ever written to a log or an error message:
//! a `.env` file holds the token and the sign-in secrets.
//!
//! The same place gives the Claude API's key and address (`ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`,
//! [`Environment::language_endpoint`]), so that a `.env` beside the engine is enough. They are secrets like the
//! token: never shown in a `Debug` of the [`Environment`], and never handed to another program ([`child_process`]).

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use super::login::{self, SignIn};
use crate::model::language::{self, Endpoint, Unavailable};

/// The variables this engine reads; nothing else is taken from the environment or from `.env`.
pub const VARIABLES: [&str; 18] = [
    BIND,
    PORT,
    TOKEN,
    ALLOW_TOKEN,
    USE_HTTPS,
    TLS_CERT,
    TLS_KEY,
    ORIGIN,
    HOST,
    DATA,
    crate::threads::VARIABLE,
    login::OIDC_ISSUER,
    login::OIDC_CLIENT_ID,
    login::OIDC_CLIENT_SECRET,
    login::OWNER_EMAIL,
    login::ALLOW_PASSWORD,
    language::KEY_VARIABLE,
    language::BASE_VARIABLE,
];
/// The variables whose values are secrets: never passed on to a program the engine starts. The base address is
/// one too, since a proxy's address may carry its credentials.
pub const SECRETS: [&str; 4] = [TOKEN, login::OIDC_CLIENT_SECRET, language::KEY_VARIABLE, language::BASE_VARIABLE];
const BIND: &str = "CONSTELLATE_BIND";
const PORT: &str = "CONSTELLATE_PORT";
const TOKEN: &str = "CONSTELLATE_TOKEN";
/// Network mode only: whether programs may use [`TOKEN`] instead of signing in.
const ALLOW_TOKEN: &str = "CONSTELLATE_ALLOW_TOKEN";
const USE_HTTPS: &str = "CONSTELLATE_USE_HTTPS";
const TLS_CERT: &str = "CONSTELLATE_TLS_CERT";
const TLS_KEY: &str = "CONSTELLATE_TLS_KEY";
const ORIGIN: &str = "CONSTELLATE_ORIGIN";
const HOST: &str = "CONSTELLATE_HOST";
/// The data folder (settings, library, runs); read by the program, listed here so that `.env` may set it too.
pub const DATA: &str = "CONSTELLATE_DATA";

/// The shortest token accepted: 32 characters of a random hex string are 128 bits, beyond guessing.
pub const SHORTEST_TOKEN: usize = 32;

/// Why the engine refuses to start, worded for the person who set it up, with what to do about it. Never
/// contains the value of a variable.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("line {line} of .env is not NAME=value")]
    EnvFileLine { line: usize },
    #[error(".env could not be read: {why}")]
    EnvFile { why: String },
    #[error("{variable} is not valid text")]
    NotText { variable: &'static str },
    #[error("{BIND} must be an IP address, such as 127.0.0.1 (this computer only) or 0.0.0.0 (every network)")]
    NotAnAddress,
    #[error("{PORT} must be a port number from 0 to 65535 (0: any free port)")]
    NotAPort,
    #[error("{variable} must be true or false")]
    NotASwitch { variable: &'static str },
    #[error(
        "{ALLOW_TOKEN}=true lets programs use {TOKEN} instead of signing in, but {TOKEN} is not set. Use a random \
         string of at least {SHORTEST_TOKEN} characters, for example the output of `openssl rand -hex 32`"
    )]
    TokenMissing,
    #[error(
        "{TOKEN} is set, but across a network the owner signs in and programs may use the token only if \
         {ALLOW_TOKEN}=true is set too. Set it, or remove {TOKEN}"
    )]
    TokenNotAllowed,
    #[error(
        "{ORIGIN} must be set when the engine listens on a network: it is the address people open in the browser \
         (the interface, or the engine itself), where the owner signs in, for example https://papers.example.org"
    )]
    OriginNeeded,
    #[error(
        "{ORIGIN} must be an https address while HTTPS is on: the sign-in cookie must never travel unencrypted. \
         Behind something else that does TLS, set {USE_HTTPS}=false and keep {ORIGIN} https"
    )]
    OriginNotHttps,
    #[error(transparent)]
    SignIn(#[from] login::ConfigError),
    #[error("{TOKEN} must be at least {SHORTEST_TOKEN} characters long")]
    TokenTooShort,
    #[error("{TOKEN} may contain only letters, digits and - . _ ~ + / =")]
    TokenCharacters,
    #[error("the operating system's random source could not be read to make a token: {why}")]
    NoRandomness { why: String },
    #[error(
        "the engine listens on a network ({BIND} is not a loopback address) with HTTPS on, but {missing} is not \
         set. Set {TLS_CERT} and {TLS_KEY} to a PEM certificate (with its chain) and its private key. To use the \
         engine on this computer only, set {BIND}=127.0.0.1 instead. Plain HTTP across a network sends the token \
         in the clear; if you really mean it, set {USE_HTTPS}=false"
    )]
    CertificateMissing { missing: &'static str },
    #[error("{given} is set but {missing} is not: a certificate needs its private key, and the other way round")]
    HalfCertificate { given: &'static str, missing: &'static str },
    #[error("{variable} ({path}) could not be used: {why}")]
    Certificate { variable: &'static str, path: PathBuf, why: String },
    #[error("{ORIGIN} must be the interface's origin, scheme://host[:port] with http or https, and nothing after it")]
    NotAnOrigin,
    #[error(
        "{HOST} must list the names the engine is reached by (separated by commas, without ports), and must be \
         set when {BIND} is 0.0.0.0 or ::, so that requests addressed to other names can be refused"
    )]
    NamesNeeded,
    #[error(transparent)]
    Threads(#[from] crate::threads::ThreadsError),
}

/// How many threads the engine computes on: every core, or fewer when `CONSTELLATE_THREADS` says so
/// ([`crate::threads::decide`]).
pub fn threads(environment: &Environment) -> Result<std::num::NonZeroUsize, ConfigError> {
    Ok(crate::threads::decide(crate::threads::cap(environment.get(crate::threads::VARIABLE))?))
}

/// The two ways the engine can be reached. This is the one place that decides which one an address is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Only from this computer.
    Local,
    /// From other machines.
    Network,
}

impl Mode {
    pub fn of(address: IpAddr) -> Mode {
        // an IPv6-mapped 127.0.0.1 is loopback too
        if address.to_canonical().is_loopback() { Mode::Local } else { Mode::Network }
    }
}

/// The secret every request carries. Shown by no `Debug` or `Display`, so that it cannot reach a log by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// 32 bytes from the operating system's random source, as 64 hexadecimal characters.
    fn generate() -> Result<Token, ConfigError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| ConfigError::NoRandomness { why: error.to_string() })?;
        Ok(Token(bytes.iter().map(|byte| format!("{byte:02x}")).collect()))
    }

    fn given(text: &str) -> Result<Token, ConfigError> {
        // what may stand after "Bearer " in a header (RFC 6750's token68), so that it can always be sent as is
        let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~+/=".contains(c);
        if !text.chars().all(allowed) {
            return Err(ConfigError::TokenCharacters);
        }
        if text.len() < SHORTEST_TOKEN {
            return Err(ConfigError::TokenTooShort);
        }
        Ok(Token(text.to_owned()))
    }

    /// The token itself: for the startup line and the security layer, nowhere else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(hidden)")
    }
}

/// Where the token came from: a generated one must be handed to whoever started the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenSource {
    Given,
    Generated,
}

/// What lets a request in. Which one is decided by the mode, and only here.
#[derive(Clone, Debug)]
pub enum Access {
    /// On this computer: the token handed to the desktop shell. Nobody signs in.
    Token { token: Token, source: TokenSource },
    /// Across a network: the owner signs in; programs may also carry the token if the owner allowed it.
    SignIn { sign_in: SignIn, programs: Programs },
}

/// Across a network, whether a program may skip signing in by carrying the token.
#[derive(Clone, Debug)]
pub enum Programs {
    /// The default: every request needs the owner's session.
    Refused,
    /// `CONSTELLATE_ALLOW_TOKEN=true`: this token is accepted too.
    Token(Token),
}

/// How the sign-in cookie is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cookie {
    /// `__Host-` prefixed and `Secure`: the browser sends it over HTTPS only, to this exact host.
    Secure,
    /// Over plain HTTP, which the person asked for ([`OptOut`]) and is warned about: no prefix, no `Secure`.
    Plain(OptOut),
}

/// The interface's origin, `scheme://host[:port]` in lower case: what a browser sends in `Origin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin(String);

impl Origin {
    pub fn parse(text: &str) -> Option<Origin> {
        let text = text.trim().trim_end_matches('/').to_ascii_lowercase();
        let (scheme, authority) = text.split_once("://")?;
        let fine = |c: char| c.is_ascii_alphanumeric() || "-.:[]".contains(c);
        let valid = matches!(scheme, "http" | "https") && !authority.is_empty() && authority.chars().all(fine);
        valid.then_some(Origin(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_https(&self) -> bool {
        self.0.starts_with("https://")
    }
}

/// A certificate and key, read and ready to serve.
#[derive(Clone)]
pub struct Tls {
    config: Arc<ServerConfig>,
}

impl fmt::Debug for Tls {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Tls")
    }
}

impl Tls {
    /// Read a PEM certificate chain and its private key. Errors name the file, never its content.
    pub fn load(certificate: &Path, key: &Path) -> Result<Tls, ConfigError> {
        let refused =
            |variable, path: &Path, why: String| ConfigError::Certificate { variable, path: path.to_path_buf(), why };
        let pem_error = |error: rustls::pki_types::pem::Error| match error {
            rustls::pki_types::pem::Error::Io(error) => error.to_string(),
            // the other errors may quote a line of the file, which for a key is part of the secret
            _ => "it is not in PEM format".to_owned(),
        };
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(certificate)
            .and_then(|found| found.collect())
            .map_err(|error| refused(TLS_CERT, certificate, pem_error(error)))?;
        if chain.is_empty() {
            return Err(refused(TLS_CERT, certificate, "it holds no certificate".to_owned()));
        }
        let private = PrivateKeyDer::from_pem_file(key).map_err(|error| refused(TLS_KEY, key, pem_error(error)))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| refused(TLS_CERT, certificate, error.to_string()))?
            .with_no_client_auth()
            .with_single_cert(chain, private)
            .map_err(|_| refused(TLS_KEY, key, "it is not the private key of that certificate".to_owned()))?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Tls { config: Arc::new(config) })
    }

    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }
}

/// Proof that the address is loopback: plain HTTP is allowed because nothing leaves the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loopback(());

/// Proof that the person asked for plain HTTP across a network (`CONSTELLATE_USE_HTTPS=false`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptOut(());

/// How bytes travel between the caller and the engine.
#[derive(Clone, Debug)]
pub enum Transport {
    Https(Tls),
    /// Plain HTTP on this computer's loopback.
    Loopback(Loopback),
    /// Plain HTTP across a network, because the person said so.
    PlainByChoice(OptOut),
}

/// How the engine listens, checked. See the module's documentation for the rules.
#[derive(Clone, Debug)]
pub struct Listen {
    address: SocketAddr,
    access: Access,
    transport: Transport,
    origin: Option<Origin>,
    names: Vec<String>,
}

/// The variables as read, from the process's environment over `.env`.
#[derive(Clone, Default)]
pub struct Environment(HashMap<&'static str, String>);

/// The names that are set, never their values: they include the language model's API keys.
impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<&str> = self.0.keys().copied().collect();
        names.sort_unstable();
        formatter.debug_tuple("Environment").field(&names).finish()
    }
}

impl Environment {
    /// The engine's variables: from `.env` in `folder` if there is one, each overridden by the process's
    /// environment.
    pub fn read(folder: &Path) -> Result<Environment, ConfigError> {
        let mut read = match fs::read_to_string(folder.join(".env")) {
            Ok(text) => parse_env_file(&text)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Environment::default(),
            Err(error) => return Err(ConfigError::EnvFile { why: error.to_string() }),
        };
        // the language model's keys and addresses too (`model::language::variables`), so that `.env` may set them
        for variable in VARIABLES.into_iter().chain(crate::model::language::variables()) {
            match std::env::var(variable) {
                Ok(value) => {
                    read.0.insert(variable, value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(std::env::VarError::NotUnicode(_)) => return Err(ConfigError::NotText { variable }),
            }
        }
        Ok(read)
    }

    /// Made from pairs, for tests and for callers that read their configuration elsewhere. Unknown names are
    /// dropped.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Environment {
        let mut read = Environment::default();
        for (name, value) in pairs {
            if let Some(variable) =
                VARIABLES.into_iter().chain(crate::model::language::variables()).find(|known| *known == name)
            {
                read.0.insert(variable, value.to_owned());
            }
        }
        read
    }

    /// A variable's value; an empty one counts as unset.
    pub fn get(&self, variable: &str) -> Option<&str> {
        self.0.get(variable).map(|value| value.trim()).filter(|value| !value.is_empty())
    }

    /// The Claude API's key and address, from here like every other variable; why not when they cannot be used.
    pub fn language_endpoint(&self) -> Result<Endpoint, Unavailable> {
        Endpoint::from_lookup(|name| self.get(name).map(str::to_owned))
    }
}

/// The one way the engine starts another program: with the secrets ([`SECRETS`]) and every language model
/// provider's key and address ([`crate::model::language::variables`]) taken out of what it inherits, so that a key
/// set in the engine's environment never reaches a program that has no use for it. A test reads the engine's
/// sources to see that no program is started any other way.
pub fn child_process(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    for secret in SECRETS.into_iter().chain(crate::model::language::variables()) {
        command.env_remove(secret);
    }
    command
}

/// `NAME=value` lines; blank lines and `#` comments are skipped, `export ` before a name is allowed, a value may
/// be in single or double quotes. A line that is none of these is an error that gives its number only.
fn parse_env_file(text: &str) -> Result<Environment, ConfigError> {
    let mut pairs = Vec::new();
    // Notepad starts a file saved as UTF-8 with a byte order mark
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (name, value) = line.split_once('=').ok_or(ConfigError::EnvFileLine { line: index + 1 })?;
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ConfigError::EnvFileLine { line: index + 1 });
        }
        let value = value.trim();
        let quoted =
            ['"', '\''].iter().find(|quote| value.len() >= 2 && value.starts_with(**quote) && value.ends_with(**quote));
        let value = match quoted {
            Some(_) => &value[1..value.len() - 1],
            None => value.split(" #").next().unwrap_or(value).trim(), // a comment after an unquoted value
        };
        pairs.push((name, value));
    }
    Ok(Environment::from_pairs(pairs))
}

/// `true`, `false` and their usual spellings.
pub(crate) fn switch(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The switch `variable`, `default` when it is not set.
fn switched(environment: &Environment, variable: &'static str, default: bool) -> Result<bool, ConfigError> {
    match environment.get(variable) {
        Some(text) => switch(text).ok_or(ConfigError::NotASwitch { variable }),
        None => Ok(default),
    }
}

/// A host name or address literal as it may stand in a `Host` header, in lower case.
/// A host name or an IPv6 literal in brackets, without a port.
fn host_name(text: &str) -> Option<String> {
    let name = text.trim().to_ascii_lowercase();
    let plain = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c));
    let bracketed = name.strip_prefix('[').and_then(|rest| rest.strip_suffix(']'));
    let ipv6 = bracketed.is_some_and(|inside| inside.parse::<std::net::Ipv6Addr>().is_ok());
    (plain || ipv6).then_some(name)
}

impl Listen {
    /// Check the variables and decide how to listen. See the module's documentation for the rules.
    pub fn from_environment(environment: &Environment) -> Result<Listen, ConfigError> {
        let ip: IpAddr = match environment.get(BIND) {
            Some(text) => text.parse().map_err(|_| ConfigError::NotAnAddress)?,
            None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        let port: u16 = match environment.get(PORT) {
            Some(text) => text.parse().map_err(|_| ConfigError::NotAPort)?,
            None => 0,
        };
        let use_https = switched(environment, USE_HTTPS, true)?;
        let origin =
            environment.get(ORIGIN).map(|text| Origin::parse(text).ok_or(ConfigError::NotAnOrigin)).transpose()?;
        let names = match environment.get(HOST) {
            Some(text) => text
                .split(',')
                .map(|name| host_name(name).ok_or(ConfigError::NamesNeeded))
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let given_token = environment.get(TOKEN).map(Token::given).transpose()?;
        let certificate = match (environment.get(TLS_CERT), environment.get(TLS_KEY)) {
            (Some(certificate), Some(key)) => Some((PathBuf::from(certificate), PathBuf::from(key))),
            (Some(_), None) => return Err(ConfigError::HalfCertificate { given: TLS_CERT, missing: TLS_KEY }),
            (None, Some(_)) => return Err(ConfigError::HalfCertificate { given: TLS_KEY, missing: TLS_CERT }),
            (None, None) => None,
        };

        let mode = Mode::of(ip);
        let transport = match (mode, use_https, certificate) {
            (_, true, Some((certificate, key))) => Transport::Https(Tls::load(&certificate, &key)?),
            (Mode::Local, _, _) => Transport::Loopback(Loopback(())),
            (Mode::Network, true, None) => return Err(ConfigError::CertificateMissing { missing: TLS_CERT }),
            (Mode::Network, false, _) => Transport::PlainByChoice(OptOut(())),
        };
        if mode == Mode::Network && ip.is_unspecified() && names.is_empty() {
            return Err(ConfigError::NamesNeeded);
        }
        let access = match mode {
            Mode::Local => match given_token {
                Some(token) => Access::Token { token, source: TokenSource::Given },
                None => Access::Token { token: Token::generate()?, source: TokenSource::Generated },
            },
            Mode::Network => {
                let allow_token = switched(environment, ALLOW_TOKEN, false)?;
                let programs = match (allow_token, given_token) {
                    (true, Some(token)) => Programs::Token(token),
                    (true, None) => return Err(ConfigError::TokenMissing),
                    (false, Some(_)) => return Err(ConfigError::TokenNotAllowed),
                    (false, None) => Programs::Refused,
                };
                let origin = origin.as_ref().ok_or(ConfigError::OriginNeeded)?;
                // The browser decides whether it keeps a Secure cookie from the scheme it sees: an https origin
                // gets one even when the engine itself is plain behind something that does TLS.
                let cookie = match (&transport, origin.is_https()) {
                    (_, true) => Cookie::Secure,
                    (Transport::PlainByChoice(proof), false) => Cookie::Plain(*proof),
                    (Transport::Https(_) | Transport::Loopback(_), false) => return Err(ConfigError::OriginNotHttps),
                };
                Access::SignIn { sign_in: SignIn::from_environment(environment, origin, cookie)?, programs }
            }
        };
        Ok(Listen { address: SocketAddr::new(ip, port), access, transport, origin, names })
    }

    /// The address to bind; its port is 0 when any free port will do.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn mode(&self) -> Mode {
        Mode::of(self.address.ip())
    }

    pub fn access(&self) -> &Access {
        &self.access
    }

    /// The token, when the engine made it up itself and must hand it to whoever started it.
    pub fn generated_token(&self) -> Option<&Token> {
        match &self.access {
            Access::Token { token, source: TokenSource::Generated } => Some(token),
            Access::Token { source: TokenSource::Given, .. } | Access::SignIn { .. } => None,
        }
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn scheme(&self) -> &'static str {
        match self.transport {
            Transport::Https(_) => "https",
            Transport::Loopback(_) | Transport::PlainByChoice(_) => "http",
        }
    }

    /// The interface's origin, when one is configured: the only one whose browser requests are answered.
    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }

    /// The host names a request may be addressed to, besides the bound address itself.
    pub fn names(&self) -> Vec<String> {
        let mut names = self.names.clone();
        if self.mode() == Mode::Local {
            names.push("localhost".to_owned());
        }
        names
    }

    /// What is unsafe about this way of listening, for stderr at start and for the interface to show. Empty
    /// unless the person opted out of HTTPS across a network.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = match self.transport {
            Transport::PlainByChoice(_) => vec![
                format!(
                    "HTTPS is off ({USE_HTTPS}=false) although the engine listens on a network ({})",
                    self.address.ip()
                ),
                "Passwords, the sign-in cookie, the access token and everything the engine answers cross the \
                 network unencrypted unless something in front of the engine does TLS: anyone who can see the \
                 traffic can read them and then act as the owner."
                    .to_owned(),
                format!("Set {TLS_CERT} and {TLS_KEY} and remove {USE_HTTPS}=false, or bind to 127.0.0.1."),
            ],
            Transport::Https(_) | Transport::Loopback(_) => Vec::new(),
        };
        let plain_cookie = match &self.access {
            Access::SignIn { sign_in, .. } => matches!(sign_in.cookie(), Cookie::Plain(_)),
            Access::Token { .. } => false,
        };
        if plain_cookie {
            warnings.push(format!(
                "The sign-in cookie is sent without Secure (and without the __Host- prefix), because {ORIGIN} is \
                 plain http: a browser will send it over unencrypted connections."
            ));
        }
        warnings
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const STRONG: &str = "0123456789abcdef0123456789abcdef";

    /// A self-signed certificate for `localhost` and its key, written to `folder`.
    pub(crate) fn certificate_in(folder: &Path) -> (PathBuf, PathBuf) {
        let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).unwrap();
        let (certificate, key) = (folder.join("engine.pem"), folder.join("engine-key.pem"));
        fs::write(&certificate, made.cert.pem()).unwrap();
        fs::write(&key, made.signing_key.serialize_pem()).unwrap();
        (certificate, key)
    }

    fn listen(pairs: &[(&str, &str)]) -> Result<Listen, ConfigError> {
        Listen::from_environment(&Environment::from_pairs(pairs.iter().copied()))
    }

    #[test]
    fn the_mode_is_local_exactly_for_loopback_addresses() {
        for local in ["127.0.0.1", "127.8.9.10", "::1", "::ffff:127.0.0.1"] {
            assert_eq!(Mode::of(local.parse().unwrap()), Mode::Local, "{local}");
        }
        for network in ["0.0.0.0", "::", "192.168.1.20", "10.0.0.1", "fe80::1"] {
            assert_eq!(Mode::of(network.parse().unwrap()), Mode::Network, "{network}");
        }
    }

    /// What is needed besides an address across a network: the interface's origin and a way to sign in.
    pub(crate) const SIGNING_IN: [(&str, &str); 2] = [(ORIGIN, "https://papers.test"), (login::ALLOW_PASSWORD, "true")];

    fn with_sign_in<'a>(pairs: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        SIGNING_IN.iter().copied().chain(pairs.iter().copied()).collect()
    }

    /// The token of local mode, and where it came from.
    fn token_of(listen: &Listen) -> (&str, TokenSource) {
        match listen.access() {
            Access::Token { token, source } => (token.expose(), *source),
            Access::SignIn { .. } => panic!("not local mode"),
        }
    }

    #[test]
    fn nothing_set_listens_on_this_computer_on_any_port_with_a_generated_token_and_plain_http() {
        let listen = listen(&[]).unwrap();
        assert_eq!(listen.address(), "127.0.0.1:0".parse().unwrap());
        assert_eq!(
            (listen.mode(), token_of(&listen).1, listen.scheme()),
            (Mode::Local, TokenSource::Generated, "http")
        );
        assert_eq!(token_of(&listen).0.len(), 64);
        assert_eq!(listen.generated_token().map(Token::expose), Some(token_of(&listen).0));
        assert!(listen.warnings().is_empty());
        let again = self::listen(&[]).unwrap();
        assert_ne!(token_of(&listen).0, token_of(&again).0); // a new one each time
    }

    #[test]
    fn local_mode_ignores_the_sign_in_settings() {
        let listen = listen(&[(login::OIDC_CLIENT_ID, "x"), (login::ALLOW_PASSWORD, "true"), (TOKEN, STRONG)]).unwrap();
        assert!(matches!(listen.access(), Access::Token { source: TokenSource::Given, .. }));
        assert!(listen.generated_token().is_none());
    }

    #[test]
    fn a_token_and_the_sign_in_secrets_are_never_shown_by_debug() {
        let local = listen(&[(TOKEN, STRONG)]).unwrap();
        assert!(!format!("{local:?}").contains(STRONG));
        let network = listen(&with_sign_in(&[
            (BIND, "192.168.1.20"),
            (USE_HTTPS, "false"),
            (TOKEN, STRONG),
            (ALLOW_TOKEN, "true"),
            (login::OIDC_CLIENT_ID, "client.test"),
            (login::OIDC_CLIENT_SECRET, "secret-of-the-client"),
            (login::OWNER_EMAIL, "owner@example.org"),
        ]))
        .unwrap();
        let shown = format!("{network:?}");
        for hidden in [STRONG, "secret-of-the-client", "owner@example.org"] {
            assert!(!shown.contains(hidden), "{hidden} in {shown}");
        }
    }

    #[test]
    fn a_provider_needs_its_client_id_secret_and_the_owners_address() {
        let base = with_sign_in(&[(BIND, "192.168.1.20"), (USE_HTTPS, "false")]);
        let with = |extra: &[(&'static str, &'static str)]| {
            let pairs: Vec<_> = base.iter().copied().chain(extra.iter().copied()).collect();
            listen(&pairs)
        };
        let half = with(&[(login::OIDC_CLIENT_ID, "client.test")]).unwrap_err();
        assert!(matches!(half, ConfigError::SignIn(login::ConfigError::HalfClient { .. })));
        let half = with(&[(login::OIDC_CLIENT_SECRET, "s")]).unwrap_err();
        assert!(matches!(half, ConfigError::SignIn(login::ConfigError::HalfClient { .. })));
        let client = [(login::OIDC_CLIENT_ID, "client.test"), (login::OIDC_CLIENT_SECRET, "s")];
        assert_eq!(with(&client).unwrap_err(), ConfigError::SignIn(login::ConfigError::OwnerNeeded));
        let bad_owner = [client[0], client[1], (login::OWNER_EMAIL, "not an address")];
        assert_eq!(with(&bad_owner).unwrap_err(), ConfigError::SignIn(login::ConfigError::NotAnEmail));
        let owner = (login::OWNER_EMAIL, "owner@example.org");
        let both = with(&[client[0], client[1], owner]).unwrap();
        let Access::SignIn { sign_in, .. } = both.access() else { panic!("network mode") };
        assert!(matches!(sign_in.methods(), login::Methods::Both(_)));
        for issuer in ["http://accounts.example.org", "https://x.test/?a=1", "not a url"] {
            let pairs = [client[0], client[1], owner, (login::OIDC_ISSUER, issuer)];
            assert_eq!(with(&pairs).unwrap_err(), ConfigError::SignIn(login::ConfigError::NotAnIssuer), "{issuer}");
        }
        // Google only: password sign-in is off unless asked for
        let only = [(BIND, "192.168.1.20"), (USE_HTTPS, "false"), SIGNING_IN[0], client[0], client[1], owner];
        let only = listen(&only).unwrap();
        let Access::SignIn { sign_in, .. } = only.access() else { panic!("network mode") };
        assert!(matches!(sign_in.methods(), login::Methods::Provider(_)));
    }

    #[test]
    fn a_given_token_must_be_long_and_fit_in_a_header() {
        assert_eq!(listen(&[(TOKEN, "short")]).unwrap_err(), ConfigError::TokenTooShort);
        let spaced = format!("{STRONG} x");
        assert_eq!(listen(&[(TOKEN, &spaced)]).unwrap_err(), ConfigError::TokenCharacters);
        let broken = format!("{STRONG}\r\nX-Injected: 1");
        assert_eq!(listen(&[(TOKEN, &broken)]).unwrap_err(), ConfigError::TokenCharacters);
        let given = listen(&[(TOKEN, STRONG)]).unwrap();
        assert_eq!(token_of(&given), (STRONG, TokenSource::Given));
    }

    #[test]
    fn local_mode_uses_https_when_given_a_certificate_and_refuses_half_of_one() {
        let folder = tempfile::tempdir().unwrap();
        let (certificate, key) = certificate_in(folder.path());
        let (certificate, key) = (certificate.to_str().unwrap(), key.to_str().unwrap());
        assert_eq!(listen(&[(TLS_CERT, certificate), (TLS_KEY, key)]).unwrap().scheme(), "https");
        assert_eq!(listen(&[(TLS_CERT, certificate), (TLS_KEY, key), (USE_HTTPS, "false")]).unwrap().scheme(), "http");
        assert_eq!(
            listen(&[(TLS_CERT, certificate)]).unwrap_err(),
            ConfigError::HalfCertificate { given: TLS_CERT, missing: TLS_KEY }
        );
        assert_eq!(
            listen(&[(TLS_KEY, key)]).unwrap_err(),
            ConfigError::HalfCertificate { given: TLS_KEY, missing: TLS_CERT }
        );
        // local with HTTPS on and no certificate: plain, because nothing leaves the computer, and nothing to warn about
        let plain = listen(&[(USE_HTTPS, "true")]).unwrap();
        assert!(matches!(plain.transport(), Transport::Loopback(_)) && plain.warnings().is_empty());
    }

    #[test]
    fn across_a_network_the_owner_signs_in_and_the_token_is_for_programs_only_when_allowed() {
        let network = [(BIND, "192.168.1.20"), (USE_HTTPS, "false")];
        let signed = listen(&with_sign_in(&network)).unwrap();
        assert!(matches!(signed.access(), Access::SignIn { programs: Programs::Refused, .. }));
        assert!(signed.generated_token().is_none());

        let token = [network[0], network[1], (TOKEN, STRONG)];
        assert_eq!(listen(&with_sign_in(&token)).unwrap_err(), ConfigError::TokenNotAllowed);
        let allowed = [token[0], token[1], token[2], (ALLOW_TOKEN, "true")];
        let allowed = listen(&with_sign_in(&allowed)).unwrap();
        assert!(matches!(allowed.access(), Access::SignIn { programs: Programs::Token(_), .. }));
        let without = [network[0], network[1], (ALLOW_TOKEN, "true")];
        assert_eq!(listen(&with_sign_in(&without)).unwrap_err(), ConfigError::TokenMissing);
        let short = [network[0], network[1], (ALLOW_TOKEN, "true"), (TOKEN, "short")];
        assert_eq!(listen(&with_sign_in(&short)).unwrap_err(), ConfigError::TokenTooShort);
        let unclear = [network[0], network[1], (ALLOW_TOKEN, "perhaps")];
        assert_eq!(listen(&with_sign_in(&unclear)).unwrap_err(), ConfigError::NotASwitch { variable: ALLOW_TOKEN });
    }

    #[test]
    fn across_a_network_the_interfaces_origin_and_a_way_to_sign_in_are_required() {
        let network = [(BIND, "192.168.1.20"), (USE_HTTPS, "false")];
        assert_eq!(listen(&[network[0], network[1], SIGNING_IN[1]]).unwrap_err(), ConfigError::OriginNeeded);
        let refused = listen(&[network[0], network[1], SIGNING_IN[0]]).unwrap_err();
        assert_eq!(refused, ConfigError::SignIn(login::ConfigError::NoMethod));
        let said = refused.to_string();
        assert!(said.contains(login::OIDC_CLIENT_ID) && said.contains(login::ALLOW_PASSWORD), "{said}");
    }

    #[test]
    fn with_https_on_the_origin_must_be_https_and_the_cookie_is_secure() {
        let folder = tempfile::tempdir().unwrap();
        let (certificate, key) = certificate_in(folder.path());
        let secure = [
            (BIND, "192.168.1.20"),
            (TLS_CERT, certificate.to_str().unwrap()),
            (TLS_KEY, key.to_str().unwrap()),
            (login::ALLOW_PASSWORD, "true"),
        ];
        let plain_origin = [secure[0], secure[1], secure[2], secure[3], (ORIGIN, "http://papers.test")];
        assert_eq!(listen(&plain_origin).unwrap_err(), ConfigError::OriginNotHttps);
        let listen = listen(&with_sign_in(&secure)).unwrap();
        let Access::SignIn { sign_in, .. } = listen.access() else { panic!("network mode") };
        assert_eq!(sign_in.cookie(), Cookie::Secure);
        assert!(listen.warnings().is_empty());
    }

    #[test]
    fn behind_something_that_does_tls_the_cookie_stays_secure_and_over_plain_http_it_is_warned_about() {
        let opted_out = [(BIND, "192.168.1.20"), (USE_HTTPS, "false"), (login::ALLOW_PASSWORD, "true")];
        let proxied = listen(&[opted_out[0], opted_out[1], opted_out[2], (ORIGIN, "https://papers.test")]).unwrap();
        let Access::SignIn { sign_in, .. } = proxied.access() else { panic!("network mode") };
        assert_eq!(sign_in.cookie(), Cookie::Secure);
        assert!(!proxied.warnings().join(" ").contains("cookie is sent without Secure"));

        let plain = listen(&[opted_out[0], opted_out[1], opted_out[2], (ORIGIN, "http://papers.test")]).unwrap();
        let Access::SignIn { sign_in, .. } = plain.access() else { panic!("network mode") };
        assert!(matches!(sign_in.cookie(), Cookie::Plain(_)));
        assert!(plain.warnings().join(" ").contains("cookie is sent without Secure"));
    }

    #[test]
    fn network_mode_without_a_certificate_refuses_to_start_and_says_what_to_do() {
        let refused = listen(&[(BIND, "192.168.1.20"), (TOKEN, STRONG)]).unwrap_err();
        assert_eq!(refused, ConfigError::CertificateMissing { missing: TLS_CERT });
        let said = refused.to_string();
        assert!(
            said.contains(TLS_CERT)
                && said.contains(TLS_KEY)
                && said.contains("127.0.0.1")
                && said.contains("USE_HTTPS=false")
        );
        assert!(listen(&[(BIND, "192.168.1.20"), (TOKEN, STRONG), (USE_HTTPS, "yes")]).is_err());
    }

    #[test]
    fn network_mode_with_a_certificate_serves_https_without_warnings() {
        let folder = tempfile::tempdir().unwrap();
        let (certificate, key) = certificate_in(folder.path());
        let listen = listen(&with_sign_in(&[
            (BIND, "192.168.1.20"),
            (PORT, "8443"),
            (TLS_CERT, certificate.to_str().unwrap()),
            (TLS_KEY, key.to_str().unwrap()),
        ]))
        .unwrap();
        assert_eq!((listen.mode(), listen.scheme(), listen.address().port()), (Mode::Network, "https", 8443));
        assert!(listen.warnings().is_empty());
        assert!(listen.names().is_empty()); // only the address itself
    }

    #[test]
    fn plain_http_across_a_network_is_allowed_only_when_asked_for_and_is_warned_about() {
        let pairs = [(BIND, "192.168.1.20"), (TOKEN, STRONG), (ALLOW_TOKEN, "on"), (USE_HTTPS, "False")];
        let listen = listen(&with_sign_in(&pairs)).unwrap();
        assert!(matches!(listen.transport(), Transport::PlainByChoice(_)));
        let warned = listen.warnings().join("\n");
        assert!(warned.contains("HTTPS is off") && warned.contains("unencrypted"));
        assert!(!warned.contains(STRONG));
    }

    #[test]
    fn a_damaged_certificate_or_a_key_that_does_not_match_is_a_refusal_that_does_not_quote_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let (certificate, key) = certificate_in(folder.path());
        // a broken section start: the PEM reader's error for it quotes the line, which must not be passed on
        fs::write(&key, "-----BEGIN c2VjcmV0LWtleS1tYXRlcmlhbA\n").unwrap();
        let pairs = [
            (BIND, "192.168.1.20"),
            (TOKEN, STRONG),
            (TLS_CERT, certificate.to_str().unwrap()),
            (TLS_KEY, key.to_str().unwrap()),
        ];
        let refused = listen(&pairs).unwrap_err();
        assert!(matches!(refused, ConfigError::Certificate { variable: TLS_KEY, .. }), "{refused}");
        assert!(!refused.to_string().contains("c2VjcmV0"));

        let other = tempfile::tempdir().unwrap();
        let (_, other_key) = certificate_in(other.path());
        let mismatched = [(TLS_CERT, certificate.to_str().unwrap()), (TLS_KEY, other_key.to_str().unwrap())];
        assert!(matches!(listen(&mismatched).unwrap_err(), ConfigError::Certificate { variable: TLS_KEY, .. }));
        let missing = [(TLS_CERT, "no-such-file.pem"), (TLS_KEY, "no-such-key.pem")];
        assert!(matches!(listen(&missing).unwrap_err(), ConfigError::Certificate { variable: TLS_CERT, .. }));
    }

    #[test]
    fn every_address_needs_names_when_it_is_every_network() {
        let base = [(BIND, "0.0.0.0"), (TOKEN, STRONG), (USE_HTTPS, "false")];
        assert_eq!(listen(&base).unwrap_err(), ConfigError::NamesNeeded);
        let named = [base[0], base[1], base[2], (ALLOW_TOKEN, "true"), (HOST, "Engine.Example.org, 192.168.1.20")];
        let named = listen(&with_sign_in(&named)).unwrap();
        assert_eq!(named.names(), ["engine.example.org", "192.168.1.20"]);
        for bad in ["bad name", "engine.example.org:8443", "::1", "[nonsense]"] {
            assert_eq!(listen(&[(HOST, bad)]).unwrap_err(), ConfigError::NamesNeeded, "{bad}");
        }
        assert_eq!(listen(&[(HOST, "[::1]")]).unwrap().names(), ["[::1]", "localhost"]);
    }

    #[test]
    fn bad_values_are_refused_without_being_repeated() {
        for (pairs, expected) in [
            ([(BIND, "localhost-ish")], ConfigError::NotAnAddress),
            ([(PORT, "70000")], ConfigError::NotAPort),
            ([(USE_HTTPS, "maybe")], ConfigError::NotASwitch { variable: USE_HTTPS }),
            ([(ORIGIN, "null")], ConfigError::NotAnOrigin),
        ] {
            let refused = listen(&pairs).unwrap_err();
            assert!(!refused.to_string().contains(pairs[0].1), "{refused}");
            assert_eq!(refused, expected);
        }
        for bad in ["http://127.0.0.1:3000/page", "ftp://127.0.0.1", "http://"] {
            assert_eq!(listen(&[(ORIGIN, bad)]).unwrap_err(), ConfigError::NotAnOrigin, "{bad}");
        }
        let origin = listen(&[(ORIGIN, "HTTP://127.0.0.1:3000/")]).unwrap();
        assert_eq!(origin.origin().unwrap().as_str(), "http://127.0.0.1:3000");
    }

    #[test]
    fn a_env_file_is_read_with_quotes_comments_and_export_and_a_bad_line_is_named_by_number_only() {
        let read = parse_env_file(&format!(
            "\u{feff}# the engine\n\nexport CONSTELLATE_PORT=8765 # fixed\nCONSTELLATE_TOKEN=\"{STRONG}\"\nCONSTELLATE_ORIGIN='http://127.0.0.1:3000'\nOTHER=1\n"
        ))
        .unwrap();
        assert_eq!(
            (read.get(PORT), read.get(TOKEN), read.get(ORIGIN)),
            (Some("8765"), Some(STRONG), Some("http://127.0.0.1:3000"))
        );
        assert_eq!(read.get("OTHER"), None); // only the engine's own variables are kept
        let refused = parse_env_file(&format!("CONSTELLATE_PORT=1\n{STRONG}\n")).unwrap_err();
        assert_eq!(refused, ConfigError::EnvFileLine { line: 2 });
        assert!(!refused.to_string().contains(STRONG));
    }

    #[test]
    fn the_environment_overrides_the_env_file_and_an_empty_value_counts_as_unset() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(".env"), "CONSTELLATE_BIND=127.0.0.2\n").unwrap();
        // the process's environment is shared by every test: only a variable no other test sets is used here
        let read = Environment::read(folder.path()).unwrap();
        assert_eq!(read.get(BIND), std::env::var(BIND).ok().as_deref().or(Some("127.0.0.2")));
        assert_eq!(Environment::from_pairs([(BIND, "  ")]).get(BIND), None);
        assert!(Environment::read(&folder.path().join("nowhere")).is_ok()); // no .env: fine
    }

    #[test]
    fn the_claude_apis_key_and_address_are_read_from_env_like_the_rest_and_never_shown() {
        let key = "test-key-not-real-0123456789";
        let read = parse_env_file(&format!(
            "ANTHROPIC_API_KEY={key}\nANTHROPIC_BASE_URL=https://gateway.example.org/claude\n"
        ))
        .unwrap();
        let endpoint = read.language_endpoint().unwrap();
        assert!(!endpoint.is_default(), "the address set in .env is the one used");
        let shown = format!("{read:?} {endpoint:?}");
        assert!(shown.contains("ANTHROPIC_API_KEY"), "{shown}");
        assert!(!shown.contains(key) && !shown.contains("gateway"), "{shown}");
        assert_eq!(parse_env_file("").unwrap().language_endpoint().unwrap_err(), Unavailable::NoKey);
        let plain =
            parse_env_file(&format!("ANTHROPIC_API_KEY={key}\nANTHROPIC_BASE_URL=http://example.org\n")).unwrap();
        assert_eq!(plain.language_endpoint().unwrap_err(), Unavailable::BadEndpoint);
    }

    #[test]
    fn a_program_the_engine_starts_inherits_none_of_its_secrets() {
        let command = child_process("a-program");
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for secret in SECRETS {
            assert!(removed.iter().any(|name| name == secret), "{secret} is passed on");
        }
        // every provider's key and address, not only the Claude API's
        for variable in ["OPENAI_API_KEY", "GEMINI_API_KEY", "CONSTELLATE_MODEL_API_KEY", "OPENROUTER_API_KEY"] {
            assert!(removed.iter().any(|name| name == variable), "{variable} is passed on");
        }
        for variable in crate::model::language::variables() {
            assert!(removed.iter().any(|name| name == variable), "{variable} is passed on");
        }
        // and no program is started any other way
        fn sources(folder: &Path, found: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    sources(&path, found);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    found.push(path);
                }
            }
        }
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() > 20);
        let starting = ["Command", "::new("].concat();
        for file in files {
            let text = fs::read_to_string(&file).unwrap();
            if file.ends_with(Path::new("api").join("config.rs")) {
                assert_eq!(text.matches(&starting).count(), 1);
            } else {
                assert!(!text.contains(&starting), "{} starts a program without child_process", file.display());
            }
        }
    }

    #[test]
    fn the_computing_threads_are_every_core_or_the_cap_and_a_wrong_cap_stops_the_engine() {
        let cores = std::thread::available_parallelism().unwrap();
        assert_eq!(threads(&Environment::default()), Ok(cores));
        assert_eq!(threads(&Environment::from_pairs([(crate::threads::VARIABLE, "1")])).unwrap().get(), 1);
        assert_eq!(threads(&Environment::from_pairs([(crate::threads::VARIABLE, "  ")])), Ok(cores));
        let refused = threads(&Environment::from_pairs([(crate::threads::VARIABLE, "all")])).unwrap_err();
        assert_eq!(refused, ConfigError::Threads(crate::threads::ThreadsError::NotACount));
        assert!(refused.to_string().contains("CONSTELLATE_THREADS"));
    }
}
