//! How the engine listens, read from the environment (and from a `.env` file in the working directory).
//!
//! There are two modes, decided in one place ([`Mode::of`]) from the address the engine binds to:
//!
//! * **Local**: a loopback address (`127.0.0.1`, `::1`). Traffic never leaves the computer, so there is nothing
//!   on the wire for anyone to read and TLS is not needed (it is used if a certificate is given). A token is
//!   still required, because other programs and web pages on the same computer can reach a loopback port too;
//!   if none is given, one is made from the operating system's random source and handed to the parent process.
//! * **Network**: any other address. The engine is then reachable from other machines, so it needs HTTPS with a
//!   certificate and key, and a token of at least 32 characters given by whoever starts it. Plain HTTP is
//!   possible only as an explicit opt-out (`CONSTELLATE_USE_HTTPS=false`), which is warned about loudly at start
//!   and told to the interface ([`Listen::warnings`]). A missing certificate is a refusal to start, never a quiet
//!   fall back to plain HTTP.
//!
//! The outcome is a [`Listen`]: it can only be made by [`Listen::from_environment`], which refuses every insecure
//! combination, and the plain-HTTP transports carry a proof ([`Loopback`], [`OptOut`]) that only this module can
//! make. No value read here is ever written to a log or an error message: a `.env` file holds the token.

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

/// The variables this engine reads; nothing else is taken from the environment or from `.env`.
pub const VARIABLES: [&str; 9] = [BIND, PORT, TOKEN, USE_HTTPS, TLS_CERT, TLS_KEY, ORIGIN, HOST, DATA];
const BIND: &str = "CONSTELLATE_BIND";
const PORT: &str = "CONSTELLATE_PORT";
const TOKEN: &str = "CONSTELLATE_TOKEN";
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
    #[error("{USE_HTTPS} must be true or false")]
    NotASwitch,
    #[error(
        "{TOKEN} must be set when the engine listens on a network ({BIND} is not a loopback address): every \
         request must carry it. Use a random string of at least {SHORTEST_TOKEN} characters, for example the \
         output of `openssl rand -hex 32`"
    )]
    TokenMissing,
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
    token: Token,
    token_source: TokenSource,
    transport: Transport,
    origin: Option<Origin>,
    names: Vec<String>,
}

/// The variables as read, from the process's environment over `.env`.
#[derive(Clone, Debug, Default)]
pub struct Environment(HashMap<&'static str, String>);

impl Environment {
    /// The engine's variables: from `.env` in `folder` if there is one, each overridden by the process's
    /// environment.
    pub fn read(folder: &Path) -> Result<Environment, ConfigError> {
        let mut read = match fs::read_to_string(folder.join(".env")) {
            Ok(text) => parse_env_file(&text)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Environment::default(),
            Err(error) => return Err(ConfigError::EnvFile { why: error.to_string() }),
        };
        for variable in VARIABLES {
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
            if let Some(variable) = VARIABLES.iter().find(|known| **known == name) {
                read.0.insert(variable, value.to_owned());
            }
        }
        read
    }

    /// A variable's value; an empty one counts as unset.
    pub fn get(&self, variable: &str) -> Option<&str> {
        self.0.get(variable).map(|value| value.trim()).filter(|value| !value.is_empty())
    }
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
fn switch(value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::NotASwitch),
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
        let use_https = environment.get(USE_HTTPS).map(switch).transpose()?.unwrap_or(true);
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
        let (token, token_source) = match (mode, given_token) {
            (_, Some(token)) => (token, TokenSource::Given),
            (Mode::Local, None) => (Token::generate()?, TokenSource::Generated),
            (Mode::Network, None) => return Err(ConfigError::TokenMissing),
        };
        let transport = match (mode, use_https, certificate) {
            (_, true, Some((certificate, key))) => Transport::Https(Tls::load(&certificate, &key)?),
            (Mode::Local, _, _) => Transport::Loopback(Loopback(())),
            (Mode::Network, true, None) => return Err(ConfigError::CertificateMissing { missing: TLS_CERT }),
            (Mode::Network, false, _) => Transport::PlainByChoice(OptOut(())),
        };
        if mode == Mode::Network && ip.is_unspecified() && names.is_empty() {
            return Err(ConfigError::NamesNeeded);
        }
        Ok(Listen { address: SocketAddr::new(ip, port), token, token_source, transport, origin, names })
    }

    /// The address to bind; its port is 0 when any free port will do.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn mode(&self) -> Mode {
        Mode::of(self.address.ip())
    }

    pub fn token(&self) -> &Token {
        &self.token
    }

    pub fn token_source(&self) -> TokenSource {
        self.token_source
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
        match self.transport {
            Transport::PlainByChoice(_) => vec![
                format!(
                    "HTTPS is off ({USE_HTTPS}=false) although the engine listens on a network ({})",
                    self.address.ip()
                ),
                "The access token and everything the engine answers cross the network unencrypted: anyone who can see \
                 the traffic can read them and then act as the app."
                    .to_owned(),
                format!("Set {TLS_CERT} and {TLS_KEY} and remove {USE_HTTPS}=false, or bind to 127.0.0.1."),
            ],
            Transport::Https(_) | Transport::Loopback(_) => Vec::new(),
        }
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

    #[test]
    fn nothing_set_listens_on_this_computer_on_any_port_with_a_generated_token_and_plain_http() {
        let listen = listen(&[]).unwrap();
        assert_eq!(listen.address(), "127.0.0.1:0".parse().unwrap());
        assert_eq!(
            (listen.mode(), listen.token_source(), listen.scheme()),
            (Mode::Local, TokenSource::Generated, "http")
        );
        assert_eq!(listen.token().expose().len(), 64);
        assert!(listen.warnings().is_empty());
        assert_ne!(listen.token(), self::listen(&[]).unwrap().token()); // a new one each time
    }

    #[test]
    fn a_token_is_never_shown_by_debug() {
        let listen = listen(&[(TOKEN, STRONG)]).unwrap();
        assert!(!format!("{listen:?}").contains(STRONG));
    }

    #[test]
    fn a_given_token_must_be_long_and_fit_in_a_header() {
        assert_eq!(listen(&[(TOKEN, "short")]).unwrap_err(), ConfigError::TokenTooShort);
        let spaced = format!("{STRONG} x");
        assert_eq!(listen(&[(TOKEN, &spaced)]).unwrap_err(), ConfigError::TokenCharacters);
        let broken = format!("{STRONG}\r\nX-Injected: 1");
        assert_eq!(listen(&[(TOKEN, &broken)]).unwrap_err(), ConfigError::TokenCharacters);
        let given = listen(&[(TOKEN, STRONG)]).unwrap();
        assert_eq!((given.token().expose(), given.token_source()), (STRONG, TokenSource::Given));
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
    fn network_mode_needs_a_token_of_its_own() {
        let refused = listen(&[(BIND, "192.168.1.20"), (USE_HTTPS, "false")]).unwrap_err();
        assert_eq!(refused, ConfigError::TokenMissing);
        assert_eq!(
            listen(&[(BIND, "192.168.1.20"), (TOKEN, "short"), (USE_HTTPS, "false")]).unwrap_err(),
            ConfigError::TokenTooShort
        );
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
        let listen = listen(&[
            (BIND, "192.168.1.20"),
            (PORT, "8443"),
            (TOKEN, STRONG),
            (TLS_CERT, certificate.to_str().unwrap()),
            (TLS_KEY, key.to_str().unwrap()),
        ])
        .unwrap();
        assert_eq!((listen.mode(), listen.scheme(), listen.address().port()), (Mode::Network, "https", 8443));
        assert!(listen.warnings().is_empty());
        assert!(listen.names().is_empty()); // only the address itself
    }

    #[test]
    fn plain_http_across_a_network_is_allowed_only_when_asked_for_and_is_warned_about() {
        let listen = listen(&[(BIND, "192.168.1.20"), (TOKEN, STRONG), (USE_HTTPS, "False")]).unwrap();
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
        let named = listen(&[base[0], base[1], base[2], (HOST, "Engine.Example.org, 192.168.1.20")]).unwrap();
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
            ([(USE_HTTPS, "maybe")], ConfigError::NotASwitch),
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
}
