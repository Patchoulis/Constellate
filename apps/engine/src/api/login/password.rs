//! The owner's password, for an owner who does not sign in with Google.
//!
//! It is kept only as an Argon2id hash, in [`FILE`] in the data folder (readable by the engine's user alone on
//! Unix). When the engine starts with password sign-in on and no password set, it makes a one-time setup code
//! ([`SetupCode`]) and prints it on standard error, where only whoever runs the server sees it; whoever has the
//! code sets the password (`POST /auth/setup`). The stages are explicit ([`Stage`]):
//!
//! ```text
//! AwaitingSetup { code, until } --right code, before `until`, acceptable password--> Set(hash)
//! Set(hash)                                         (the code is gone: it worked once)
//! ```
//!
//! A code that ran out ([`SETUP_LASTS`]) stays refused until the engine is restarted, which prints a new one.
//!
//! Argon2id, with [`Hashing::STRONG`]: 64 MiB of memory, 3 passes, 1 lane. This is the second of RFC 9106's
//! recommended settings with one lane instead of four (the crate computes lanes one after another, so more lanes
//! would only cost time here, and the memory is what makes guessing on graphics cards expensive). It is well
//! above OWASP's minimum (19 MiB, 2 passes) and takes a fraction of a second on a small server. At most
//! [`HASHING_AT_ONCE`] hashes are computed at once, so that many sign-in attempts together cannot take more than
//! that many times 64 MiB.
//!
//! Every attempt costs one Argon2 computation, whether a password is set, the code is awaited, or password
//! sign-in is not offered at all ([`Hashing::decoy`]): how long a refusal takes does not tell them apart.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::phc::PasswordHash;
use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};

use subtle::ConstantTimeEq;

use super::sessions::digest;

/// The file the hash is kept in, in the data folder.
pub const FILE: &str = "owner-password";
/// How long the setup code works after the engine started.
pub const SETUP_LASTS: Duration = Duration::from_secs(15 * 60);
/// The shortest password accepted, in characters.
pub const SHORTEST: usize = 12;
/// The longest, in bytes: far beyond any real password, short enough to cost nothing to hash.
pub const LONGEST: usize = 1024;
/// Argon2 computations at once.
pub const HASHING_AT_ONCE: usize = 2;

/// Why the password file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "{} does not hold a password hash this engine can use. Delete it to set a new password with a setup \
         code at the next start",
        path.display()
    )]
    Damaged { path: PathBuf },
    #[error("the operating system's random source could not be read")]
    NoRandomness,
    #[error("the password could not be hashed")]
    Hashing,
}

/// Why a password is not accepted, worded for the owner choosing it. Never repeats the password.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Weak {
    #[error("the password must be at least {SHORTEST} characters long")]
    TooShort,
    #[error("the password must be at most {LONGEST} bytes long")]
    TooLong,
    #[error("the password must not be or contain the owner's email address")]
    TheEmail,
    #[error("the password is one of the most common ones, or a word and a few digits: choose another")]
    Common,
    #[error("the password is too repetitive: use at least five different characters, not one pattern repeated")]
    Repetitive,
}

/// Passwords that are long enough and yet among the first ones tried. Compared in lower case.
const COMMON: [&str; 24] = [
    "123456789012",
    "1234567890123",
    "12345678901234",
    "123456789abc",
    "1q2w3e4r5t6y",
    "1qaz2wsx3edc",
    "qwertyuiopas",
    "qwertyuiop123",
    "qwerty123456",
    "asdfghjkl123",
    "zxcvbnm12345",
    "abcdefghijkl",
    "abc123abc123",
    "iloveyou1234",
    "letmeinletmein",
    "passwordpassword",
    "administrator",
    "administrator1",
    "correcthorsebatterystaple",
    "trustno1trustno1",
    "welcomewelcome",
    "changemechangeme",
    "qazwsxedcrfv",
    "football1234",
];

/// Words that, followed only by digits and punctuation, make a password that is guessed early.
const WORDS: [&str; 14] = [
    "password",
    "passw0rd",
    "qwerty",
    "letmein",
    "welcome",
    "admin",
    "iloveyou",
    "constellate",
    "monkey",
    "dragon",
    "sunshine",
    "princess",
    "football",
    "changeme",
];

/// Is `password` good enough to be the owner's? What is checked: its length ([`SHORTEST`] characters to
/// [`LONGEST`] bytes); that it is not, and does not contain, the owner's email address or the part before the
/// `@` (when that part has four characters or more); that it is not one of a short list of common long
/// passwords nor a common word followed only by digits and punctuation; and that it has at least five different
/// characters and is not one short pattern repeated. Case is ignored for all but the length.
pub fn acceptable(password: &str, owner: Option<&str>) -> Result<(), Weak> {
    if password.len() > LONGEST {
        return Err(Weak::TooLong);
    }
    if password.chars().count() < SHORTEST {
        return Err(Weak::TooShort);
    }
    let lower = password.to_lowercase();
    if let Some(owner) = owner.map(str::to_lowercase) {
        let local = owner.rsplit_once('@').map_or(owner.as_str(), |(local, _)| local);
        if lower.contains(&owner) || (local.chars().count() >= 4 && lower.contains(local)) {
            return Err(Weak::TheEmail);
        }
    }
    let after_word = WORDS.iter().find_map(|word| lower.strip_prefix(word));
    if COMMON.contains(&lower.as_str()) || after_word.is_some_and(|rest| rest.chars().all(|c| !c.is_alphabetic())) {
        return Err(Weak::Common);
    }
    let characters: Vec<char> = lower.chars().collect();
    let mut different = characters.clone();
    different.sort_unstable();
    different.dedup();
    let repeated = (1..=characters.len() / 2)
        .any(|unit| characters.len() % unit == 0 && characters.chunks(unit).all(|chunk| chunk == &characters[..unit]));
    if different.len() < 5 || repeated {
        return Err(Weak::Repetitive);
    }
    Ok(())
}

/// The Argon2id settings a new hash is made with. A stored hash names its own, which are used to check it.
#[derive(Clone)]
pub struct Hashing {
    params: Params,
    /// A hash of a random password, checked against when there is nothing real to check: a refusal then takes
    /// as long as a wrong password.
    decoy: String,
}

impl Hashing {
    /// 64 MiB, 3 passes, 1 lane (see the module's documentation).
    pub const STRONG: (u32, u32, u32) = (64 * 1024, 3, 1);

    pub fn strong() -> Result<Hashing, PasswordError> {
        let (memory, passes, lanes) = Hashing::STRONG;
        Hashing::with(memory, passes, lanes)
    }

    /// Settings of the tests' own, so that they run quickly; never used by the engine.
    #[cfg(test)]
    pub fn light() -> Hashing {
        Hashing::with(64, 1, 1).unwrap_or_else(|error| panic!("{error}"))
    }

    fn with(memory: u32, passes: u32, lanes: u32) -> Result<Hashing, PasswordError> {
        let params = Params::new(memory, passes, lanes, None).map_err(|_| PasswordError::Hashing)?;
        let decoy = hash_with(&params, &super::sessions::random_id().map_err(|_| PasswordError::NoRandomness)?)?;
        Ok(Hashing { params, decoy })
    }

    pub fn hash(&self, password: &str) -> Result<String, PasswordError> {
        hash_with(&self.params, password)
    }

    /// Spend one check's worth of time, for a refusal that must take as long as a wrong password.
    pub fn decoy(&self, password: &str) {
        let _ = verify(&self.decoy, password);
    }
}

fn hash_with(params: &Params, password: &str) -> Result<String, PasswordError> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| PasswordError::NoRandomness)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
    let hash = argon2.hash_password_with_salt(password.as_bytes(), &salt).map_err(|_| PasswordError::Hashing)?;
    Ok(hash.to_string())
}

/// Does `password` match the PHC string `hash`? The comparison inside the crate is in constant time.
fn verify(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else { return false };
    // the algorithm and settings are the stored hash's own
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// The code that lets whoever runs the server set the first password. Shown by no `Debug`.
pub struct SetupCode(String);

impl SetupCode {
    /// 100 bits from the operating system's random source, as 20 letters and digits in groups of five that are
    /// easy to type (no 0/O, 1/I/L).
    fn generate() -> Result<SetupCode, PasswordError> {
        const ALPHABET: &[u8; 32] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789-";
        let mut bytes = [0u8; 20];
        getrandom::fill(&mut bytes).map_err(|_| PasswordError::NoRandomness)?;
        let mut code = String::with_capacity(23);
        for (index, byte) in bytes.iter().enumerate() {
            if index > 0 && index % 5 == 0 {
                code.push('-');
            }
            // 31 symbols, the dash excluded; the slight bias of 256 % 31 costs less than a bit in total
            code.push(char::from(ALPHABET[usize::from(*byte) % 31]));
        }
        Ok(SetupCode(code))
    }

    /// The code itself: for the one line on standard error at start, nowhere else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SetupCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SetupCode(hidden)")
    }
}

/// A code as typed: upper case, without spaces or dashes, so that `abcde fghij` is `ABCDE-FGHIJ`.
fn typed(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Where setting the password stands.
enum Stage {
    /// No password yet: the setup code's digest (as typed), and when it stops working.
    AwaitingSetup { code: [u8; 32], until: Instant },
    /// The password's hash.
    Set(String),
}

/// What became of a setup attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum Setup {
    Done,
    /// The password is not good enough; nothing changed and the code still works.
    Weak(Weak),
    /// A wrong code, a code that ran out, or a password that is set already: one answer for all.
    Refused,
}

/// The owner's password as the engine keeps it.
pub struct Passwords {
    file: PathBuf,
    hashing: Hashing,
    owner: Option<String>,
    stage: Mutex<Stage>,
}

impl std::fmt::Debug for Passwords {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Passwords").field("file", &self.file).finish_non_exhaustive()
    }
}

impl Passwords {
    /// The password kept in `folder`, or, when there is none, a new setup code to set it with.
    pub fn open(
        folder: &Path,
        hashing: Hashing,
        owner: Option<String>,
        now: Instant,
    ) -> Result<(Passwords, Option<SetupCode>), PasswordError> {
        let file = folder.join(FILE);
        let (stage, code) = match fs::read_to_string(&file) {
            Ok(text) => {
                let hash = text.trim().to_owned();
                if PasswordHash::new(&hash).is_err() {
                    return Err(PasswordError::Damaged { path: file });
                }
                (Stage::Set(hash), None)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let code = SetupCode::generate()?;
                (Stage::AwaitingSetup { code: digest(&typed(code.expose())), until: now + SETUP_LASTS }, Some(code))
            }
            Err(source) => return Err(PasswordError::File { path: file, source }),
        };
        Ok((Passwords { file, hashing, owner, stage: Mutex::new(stage) }, code))
    }

    /// Is the password still to be set with the setup code (and is the code still good)?
    pub fn awaiting_setup(&self, now: Instant) -> bool {
        matches!(*self.stage(), Stage::AwaitingSetup { until, .. } if now < until)
    }

    fn stage(&self) -> std::sync::MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Is `password` the owner's? Takes one Argon2 computation whatever the answer, and whatever the stage.
    pub fn check(&self, password: &str) -> bool {
        let hash = match &*self.stage() {
            Stage::Set(hash) => Some(hash.clone()),
            Stage::AwaitingSetup { .. } => None,
        };
        match hash {
            // a password far too long to be the owner's is still hashed, as a decoy, so it costs the same
            Some(hash) if password.len() <= LONGEST => verify(&hash, password),
            _ => {
                self.hashing.decoy(password);
                false
            }
        }
    }

    /// Set the first password with the setup code.
    pub fn set_up(&self, code: &str, password: &str, now: Instant) -> Result<Setup, PasswordError> {
        // nothing secret is learnt from the password's strength, so it is judged first
        if let Err(weak) = acceptable(password, self.owner.as_deref()) {
            return Ok(Setup::Weak(weak));
        }
        let right = |stage: &Stage| match stage {
            Stage::AwaitingSetup { code: expected, until } => {
                bool::from(digest(&typed(code)).ct_eq(expected)) && now < *until
            }
            Stage::Set(_) => false,
        };
        if !right(&self.stage()) {
            return Ok(Setup::Refused);
        }
        let hash = self.hashing.hash(password)?;
        let mut stage = self.stage();
        // another request with the code may have set it while this one was hashing: the code works once
        if !right(&stage) {
            return Ok(Setup::Refused);
        }
        write_private(&self.file, &hash)?;
        *stage = Stage::Set(hash);
        Ok(Setup::Done)
    }

    #[cfg(test)]
    pub fn is_setup_code(&self, code: &str) -> bool {
        match &*self.stage() {
            Stage::AwaitingSetup { code: expected, .. } => digest(&typed(code)) == *expected,
            Stage::Set(_) => false,
        }
    }
}

/// Write `content` to `path` so that only this user can read it, replacing what is there in one move.
fn write_private(path: &Path, content: &str) -> Result<(), PasswordError> {
    let failed = |source| PasswordError::File { path: path.to_path_buf(), source };
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(failed)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = (|| {
        // a file left by an earlier attempt is removed, and a new one made: never one that someone else made
        // (whose permissions would be theirs)
        let _ = fs::remove_file(&temporary);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.map_err(failed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const OWNER: &str = "owner@example.org";
    const GOOD: &str = "violet kettle orbit 42";

    fn opened(folder: &Path, now: Instant) -> (Passwords, Option<SetupCode>) {
        Passwords::open(folder, Hashing::light(), Some(OWNER.to_owned()), now).unwrap()
    }

    #[test]
    fn a_password_must_be_long_not_the_email_not_common_and_not_repetitive() {
        assert_eq!(acceptable("short", None), Err(Weak::TooShort));
        assert_eq!(acceptable("ÄÖÜäöüßéèêëç", None), Ok(())); // twelve characters, more bytes
        assert_eq!(acceptable(&"x".repeat(LONGEST + 1), None), Err(Weak::TooLong));
        for email in ["owner@example.org", "OWNER@example.org!!", "my owner-key 2026"] {
            assert_eq!(acceptable(email, Some(OWNER)), Err(Weak::TheEmail), "{email}");
        }
        for common in ["123456789012", "Password2026!", "passwordpassword", "Constellate123", "QWERTYUIOPAS"] {
            assert_eq!(acceptable(common, None), Err(Weak::Common), "{common}");
        }
        for repetitive in ["aaaaaaaaaaaa", "abababababab", "abcdeabcdeabcde", "1212121212121"] {
            assert_eq!(acceptable(repetitive, None), Err(Weak::Repetitive), "{repetitive}");
        }
        assert_eq!(acceptable(GOOD, Some(OWNER)), Ok(()));
        assert_eq!(acceptable("password manager says hi", None), Ok(())); // a word, and then more than digits
    }

    #[test]
    fn the_setup_code_sets_the_password_once_and_is_kept_only_as_a_hash() {
        let folder = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let (passwords, code) = opened(folder.path(), now);
        let code = code.unwrap();
        assert!(!format!("{code:?}").contains(code.expose()));
        assert!(passwords.awaiting_setup(now));
        assert!(!passwords.check(GOOD)); // nothing to sign in with yet

        assert_eq!(passwords.set_up("WRONG-WRONG-WRONG-WRONG", GOOD, now).unwrap(), Setup::Refused);
        assert_eq!(passwords.set_up(code.expose(), "short", now).unwrap(), Setup::Weak(Weak::TooShort));
        // typed in lower case and with spaces for dashes: the same code
        let typed_by_hand = code.expose().to_lowercase().replace('-', " ");
        assert_eq!(passwords.set_up(&typed_by_hand, GOOD, now).unwrap(), Setup::Done);
        assert!(!passwords.awaiting_setup(now));
        // single use: the right code does nothing a second time
        assert_eq!(passwords.set_up(code.expose(), "another fine password 7", now).unwrap(), Setup::Refused);

        let kept = fs::read_to_string(folder.path().join(FILE)).unwrap();
        assert!(kept.starts_with("$argon2id$v=19$") && !kept.contains(GOOD));
        assert!(passwords.check(GOOD));
        assert!(!passwords.check("violet kettle orbit 43"));
        assert!(!passwords.check(&"x".repeat(LONGEST + 1)));

        // the next start finds the password, and makes no code
        let (again, code) = opened(folder.path(), now);
        assert!(code.is_none() && again.check(GOOD) && !again.awaiting_setup(now));
    }

    #[test]
    fn the_setup_code_stops_working_fifteen_minutes_after_start() {
        let folder = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let (passwords, code) = opened(folder.path(), now);
        let code = code.unwrap();
        let late = now + SETUP_LASTS;
        assert!(!passwords.awaiting_setup(late));
        assert_eq!(passwords.set_up(code.expose(), GOOD, late).unwrap(), Setup::Refused);
        assert!(!folder.path().join(FILE).exists());
        assert!(passwords.is_setup_code(code.expose()));
    }

    #[test]
    fn a_damaged_password_file_stops_the_engine_with_what_to_do() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join(FILE), "not a hash").unwrap();
        let refused = Passwords::open(folder.path(), Hashing::light(), None, Instant::now()).unwrap_err();
        assert!(matches!(refused, PasswordError::Damaged { .. }) && refused.to_string().contains("Delete it"));
    }

    #[cfg(unix)]
    #[test]
    fn the_password_file_is_readable_by_the_engines_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let now = Instant::now();
        let (passwords, code) = opened(folder.path(), now);
        passwords.set_up(code.unwrap().expose(), GOOD, now).unwrap();
        let mode = fs::metadata(folder.path().join(FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn the_strong_settings_are_what_the_documentation_says() {
        let (memory, passes, lanes) = Hashing::STRONG;
        assert_eq!((memory, passes, lanes), (65536, 3, 1));
        assert!(Params::new(memory, passes, lanes, None).is_ok());
    }

    #[test]
    fn setup_codes_are_twenty_easy_characters_in_groups_of_five_and_differ() {
        let one = SetupCode::generate().unwrap();
        let two = SetupCode::generate().unwrap();
        assert_ne!(one.expose(), two.expose());
        let groups: Vec<&str> = one.expose().split('-').collect();
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|group| group.len() == 5));
        assert!(one.expose().chars().all(|c| c == '-' || "ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(c)));
    }
}
