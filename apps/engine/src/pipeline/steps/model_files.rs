//! The embedding model's files, downloaded once into the data folder the first time they are needed.
//!
//! They come from Hugging Face at a pinned revision ([`QWEN3`]): a newer upload cannot change the vectors of a
//! library under it. Each file is checked against its size and SHA-256 before it is used, so a file cut short or
//! altered on the way is never loaded. A download that stops (the person cancels, the network drops, the app is
//! closed) leaves its `.part` file, and the next one goes on from where it stopped (an HTTP range request); a
//! server that does not take ranges sends the whole file again.
//!
//! `HF_ENDPOINT`, when set, is asked instead of `https://huggingface.co` (a mirror), as Hugging Face's own tools
//! do; it is the person's own setting, so its host is reached whatever its address. Where it redirects to (the
//! files are served from a content network) must be https on the public internet. A copy already in the Hugging
//! Face cache (`HF_HUB_CACHE`, `HF_HOME/hub`, or `~/.cache/huggingface/hub`) at the same revision is used when
//! it checks out, and nothing is downloaded.
//!
//! Every request is bounded: no more bytes than the file has, and [`PATIENCE`] in all.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use url::Url;

use crate::jobs::Tracker;
use crate::pipeline::steps::{Count, StepOutcome, Unit};

/// The longest one request may take: a slow connection gets through 1.2 GB in this time.
pub const PATIENCE: Duration = Duration::from_secs(3 * 60 * 60);
/// Redirects followed before giving up.
const MOST_HOPS: usize = 6;
/// Bytes read at a time, between which a cancel is looked at.
const PIECE: usize = 1 << 20;

/// One file of a model's repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// A model's repository at one revision, and the files the engine needs of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelRepo {
    /// The model's name, as the settings and the library call it.
    pub model: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [ModelFile],
}

/// `Qwen/Qwen3-Embedding-0.6B` exported to ONNX (see `model/embed.rs`), half precision.
pub const QWEN3: ModelRepo = ModelRepo {
    model: "Qwen/Qwen3-Embedding-0.6B",
    repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX",
    revision: "c25a394dd583836952667c12f008335071b3f43d",
    files: &[
        ModelFile {
            path: "tokenizer.json",
            bytes: 11_423_705,
            sha256: "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a",
        },
        ModelFile {
            path: "onnx/model_fp16.onnx",
            bytes: 583_522,
            sha256: "f376cf065d912675dd559270e19361367ebc28c938b9811dd61d5834c78d0834",
        },
        ModelFile {
            path: "onnx/model_fp16.onnx_data",
            bytes: 1_199_927_296,
            sha256: "06fbfded30166e3a33fe5252d7002ed1c370a4ec1bf45aba778cdf6cf7d31439",
        },
    ],
};

impl ModelRepo {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    /// Where the engine keeps it: `<data>/models/<repo, "/" as "--">/<revision>`.
    pub fn folder(&self, data: &Path) -> PathBuf {
        data.join("models").join(self.repo.replace('/', "--")).join(self.revision)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error(
        "the embedding model's files could not be downloaded from {host} (is this computer offline?): {reason}. \
         What was downloaded is kept; run again to go on"
    )]
    Unreachable { host: String, reason: String },
    #[error("{0} answered with HTTP {1} for the embedding model's files")]
    Refused(String, u16),
    #[error("{0} is not an address the embedding model may be downloaded from")]
    NotAllowed(String),
    #[error("{0} redirects too many times")]
    TooManyRedirects(String),
    #[error("{path} came with {got} bytes where {expected} were expected; run again to go on")]
    WrongSize { path: String, got: u64, expected: u64 },
    #[error("{path} is not the file it should be (its SHA-256 differs); it was removed and will be downloaded again")]
    WrongHash { path: String },
    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("stopped while downloading the embedding model; what was downloaded is kept")]
    Cancelled,
}

/// An answer to a request: its status, where it redirects, and its body to read.
pub struct Reply {
    pub status: u16,
    pub location: Option<String>,
    /// For a part of the file (206): the byte it starts at, from its `Content-Range`.
    pub range_start: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// How the model's files are asked for: over the network ([`Network`]), or a test's canned bytes. An
/// implementation never follows a redirect (it is returned, so that every hop is checked).
pub trait Download: Send + Sync {
    /// `url`, from byte `from` on (a range request when `from` is not 0). `mirror`: this is the first request,
    /// to the mirror `HF_ENDPOINT` names, which may be reached on the person's own network; every other request
    /// (huggingface.co, and wherever a redirect points) reaches only public addresses.
    fn get(&self, url: &str, from: u64, mirror: bool) -> Result<Reply, String>;
}

/// The first byte of a `Content-Range: bytes 123-456/789`.
fn range_start(header: &str) -> Option<u64> {
    header.trim().strip_prefix("bytes")?.trim().split('-').next()?.trim().parse().ok()
}

/// The model's files over the network, through `ureq` and rustls.
pub struct Network {
    /// For the content network and huggingface.co: public addresses only.
    public: ureq::Agent,
    /// For the mirror `HF_ENDPOINT` names, which may be on the person's own network.
    mirror: ureq::Agent,
}

impl Default for Network {
    fn default() -> Self {
        let config = || {
            let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::Rustls).build();
            ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .user_agent(crate::sources::USER_AGENT)
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_global(Some(PATIENCE))
                .tls_config(tls)
                .build()
        };
        let connector = ureq::unversioned::transport::DefaultConnector::new();
        let public = ureq::Agent::with_parts(config(), connector, crate::sources::PublicOnly::default());
        Self { public, mirror: ureq::Agent::new_with_config(config()) }
    }
}

impl Download for Network {
    fn get(&self, url: &str, from: u64, mirror: bool) -> Result<Reply, String> {
        let agent = if mirror { &self.mirror } else { &self.public };
        let mut request = agent.get(url);
        if from > 0 {
            request = request.header("Range", format!("bytes={from}-"));
        }
        let response = request.call().map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let (location, range) = (header("location"), header("content-range"));
        let body: Box<dyn Read + Send> = Box::new(response.into_body().into_reader());
        Ok(Reply { status, location, range_start: range.as_deref().and_then(range_start), body })
    }
}

/// The mirror `HF_ENDPOINT` names, if it is set to an address.
fn mirror() -> Option<Url> {
    std::env::var("HF_ENDPOINT").ok().and_then(|text| Url::parse(text.trim().trim_end_matches('/')).ok())
}

fn same_host(base: &Url, address: &str) -> bool {
    Url::parse(address).is_ok_and(|address| address.host_str() == base.host_str() && address.port() == base.port())
}

/// Where a file of the repository is asked for first: `<endpoint>/<repo>/resolve/<revision>/<path>`.
pub fn file_url(endpoint: &Url, repo: &ModelRepo, file: &ModelFile) -> String {
    let base = endpoint.as_str().trim_end_matches('/');
    format!("{base}/{}/resolve/{}/{}", repo.repo, repo.revision, file.path)
}

fn file_error(path: &Path) -> impl FnOnce(io::Error) -> ModelError + '_ {
    move |source| ModelError::File { path: path.to_path_buf(), source }
}

/// The SHA-256 of a file, in hex.
fn sha256_of(path: &Path) -> Result<String, ModelError> {
    let mut file = File::open(path).map_err(file_error(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; PIECE];
    loop {
        let read = file.read(&mut buffer).map_err(file_error(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn size_of_file(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Is `path` the file, by its size and hash?
fn checks_out(path: &Path, file: &ModelFile) -> Result<bool, ModelError> {
    Ok(path.is_file() && size_of_file(path) == file.bytes && sha256_of(path)? == file.sha256)
}

/// The Hugging Face cache's copy of the repository at its revision, when there is one.
fn in_hugging_face_cache(repo: &ModelRepo) -> Option<PathBuf> {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
    let hub = variable("HF_HUB_CACHE")
        .map(PathBuf::from)
        .or_else(|| variable("HF_HOME").map(|home| Path::new(&home).join("hub")))
        .or_else(|| {
            variable("USERPROFILE")
                .or_else(|| variable("HOME"))
                .map(|home| Path::new(&home).join(".cache").join("huggingface").join("hub"))
        })?;
    let folder = hub.join(format!("models--{}", repo.repo.replace('/', "--"))).join("snapshots").join(repo.revision);
    folder.is_dir().then_some(folder)
}

/// Download one file into `place`, going on from its `.part` file; `progress` is told the bytes of it there are.
fn download_file(
    download: &dyn Download,
    first: &str,
    allowed_first: &dyn Fn(&Url) -> bool,
    place: &Path,
    file: &ModelFile,
    tracker: &Tracker<'_>,
    progress: &dyn Fn(u64),
) -> Result<(), ModelError> {
    let part = place.with_file_name(format!(
        "{}.part",
        place.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    if let Some(folder) = place.parent() {
        fs::create_dir_all(folder).map_err(file_error(folder))?;
    }
    let mirrored = mirror().is_some();
    let mut have = size_of_file(&part);
    if have > file.bytes {
        fs::remove_file(&part).map_err(file_error(&part))?;
        have = 0;
    }
    if have < file.bytes {
        let mut address = Url::parse(first).map_err(|_| ModelError::NotAllowed(first.to_owned()))?;
        let mut hops = 0;
        let reply = loop {
            let allowed = if hops == 0 { allowed_first(&address) } else { crate::sources::any_https(&address) };
            if !allowed {
                return Err(ModelError::NotAllowed(address.to_string()));
            }
            let host = address.host_str().unwrap_or("").to_owned();
            let reply = download
                .get(address.as_str(), have, hops == 0 && mirrored)
                .map_err(|reason| ModelError::Unreachable { host, reason })?;
            if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
                hops += 1;
                let next = reply.location.ok_or_else(|| ModelError::TooManyRedirects(address.to_string()))?;
                if hops > MOST_HOPS {
                    return Err(ModelError::TooManyRedirects(address.to_string()));
                }
                address = address.join(&next).map_err(|_| ModelError::NotAllowed(next.clone()))?;
                continue;
            }
            break reply;
        };
        let (append, start) = match reply.status {
            206 if reply.range_start == Some(have) => (true, have),
            206 => {
                // a part that does not start where the file stopped cannot be appended: start again next time
                fs::remove_file(&part).map_err(file_error(&part))?;
                let host = address.host_str().unwrap_or("").to_owned();
                let reason = format!("it sent a part starting at {:?}, not at byte {have}", reply.range_start);
                return Err(ModelError::Unreachable { host, reason });
            }
            200 => (false, 0),
            status => return Err(ModelError::Refused(address.host_str().unwrap_or("").to_owned(), status)),
        };
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .map_err(file_error(&part))?;
        let mut written = start;
        progress(written);
        // never more than the file has: one byte more is read only to tell "too large"
        let mut body = reply.body.take(file.bytes - start + 1);
        let mut buffer = vec![0_u8; PIECE];
        loop {
            if tracker.cancelled() {
                out.flush().map_err(file_error(&part))?;
                return Err(ModelError::Cancelled);
            }
            let read = match body.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let host = address.host_str().unwrap_or("").to_owned();
                    return Err(ModelError::Unreachable { host, reason: error.to_string() });
                }
            };
            written += read as u64;
            if written > file.bytes {
                drop(out);
                let _ = fs::remove_file(&part);
                return Err(ModelError::WrongSize { path: file.path.to_owned(), got: written, expected: file.bytes });
            }
            out.write_all(&buffer[..read]).map_err(file_error(&part))?;
            progress(written);
        }
        out.flush().map_err(file_error(&part))?;
        if written != file.bytes {
            return Err(ModelError::WrongSize { path: file.path.to_owned(), got: written, expected: file.bytes });
        }
    }
    if sha256_of(&part)? != file.sha256 {
        let _ = fs::remove_file(&part);
        return Err(ModelError::WrongHash { path: file.path.to_owned() });
    }
    fs::rename(&part, place).map_err(file_error(place))
}

/// The folder holding the repository's files, downloading what is missing. Answers what the step came to.
pub fn ensure(
    repo: &ModelRepo,
    data: &Path,
    download: &dyn Download,
    tracker: &Tracker<'_>,
    count: &dyn Fn(Count),
) -> Result<(PathBuf, StepOutcome), ModelError> {
    let folder = repo.folder(data);
    let here = |file: &ModelFile| {
        let place = folder.join(file.path);
        place.is_file() && size_of_file(&place) == file.bytes // checked by hash when it was put there
    };
    if repo.files.iter().all(here) {
        return Ok((folder, StepOutcome::Kept(format!("{} at {}", repo.repo, &repo.revision[..8]))));
    }
    if let Some(cached) = in_hugging_face_cache(repo) {
        let mut every = true;
        for file in repo.files {
            every &= checks_out(&cached.join(file.path), file)?;
        }
        if every {
            tracker.log("  found in the Hugging Face cache: nothing to download");
            return Ok((cached, StepOutcome::Kept("found in the Hugging Face cache".to_owned())));
        }
    }
    let endpoint =
        mirror().unwrap_or_else(|| Url::parse("https://huggingface.co").unwrap_or_else(|_| unreachable_url()));
    let mirrored = mirror().is_some();
    let allowed_first = |address: &Url| {
        if mirrored { same_host(&endpoint, address.as_str()) } else { crate::sources::any_https(address) }
    };
    let total = repo.total_bytes();
    let mut before: u64 = repo.files.iter().filter(|file| here(file)).map(|file| file.bytes).sum();
    tracker.log(&format!(
        "  downloading {} ({:.0} MB) from {} into the data folder, once",
        repo.repo,
        total as f64 / 1_048_576.0,
        endpoint.host_str().unwrap_or("")
    ));
    for file in repo.files.iter().filter(|file| !here(file)) {
        let place = folder.join(file.path);
        let shown = |bytes: u64| count(Count { done: before + bytes, of: total, unit: Unit::Bytes });
        download_file(download, &file_url(&endpoint, repo, file), &allowed_first, &place, file, tracker, &shown)?;
        before += file.bytes;
        tracker.log(&format!("    {} ({:.0} MB) checked", file.path, file.bytes as f64 / 1_048_576.0));
    }
    Ok((folder, StepOutcome::Done(format!("downloaded {:.0} MB", total as f64 / 1_048_576.0))))
}

/// An address that parses, for the impossible case that the default endpoint does not.
fn unreachable_url() -> Url {
    #[allow(clippy::unwrap_used)] // a fixed address that parses
    Url::parse("https://huggingface.co/").unwrap()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;
    use crate::jobs::Cancel;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// Serves files from memory, taking ranges unless told not to, and notes what was asked from where.
    pub(crate) struct Served {
        files: Vec<(String, Vec<u8>)>,
        ranges: bool,
        /// Bytes after which an answer stops, as a dropped connection would.
        cut: Mutex<Option<usize>>,
        /// Answer a range request with a part that says it starts at 0.
        wrong_range: Mutex<bool>,
        pub asked: Mutex<Vec<(String, u64)>>,
    }

    impl Served {
        pub(crate) fn new(files: Vec<(String, Vec<u8>)>, ranges: bool) -> Self {
            Self { files, ranges, cut: Mutex::new(None), wrong_range: Mutex::new(false), asked: Mutex::new(Vec::new()) }
        }
    }

    impl Download for Served {
        fn get(&self, url: &str, from: u64, mirror: bool) -> Result<Reply, String> {
            assert!(!mirror, "no mirror is set in the tests");
            self.asked.lock().unwrap().push((url.to_owned(), from));
            let empty = || -> Box<dyn Read + Send> { Box::new(Cursor::new(Vec::new())) };
            if let Some(path) = url.strip_prefix("https://huggingface.co/") {
                let location = Some(format!("https://cdn.example.org/{path}"));
                return Ok(Reply { status: 302, location, range_start: None, body: empty() });
            }
            let Some((_, bytes)) = self.files.iter().find(|(path, _)| url.ends_with(path.as_str())) else {
                return Ok(Reply { status: 404, location: None, range_start: None, body: empty() });
            };
            let wrong = *self.wrong_range.lock().unwrap();
            let (status, mut body, range_start) = if self.ranges && from > 0 {
                (206, bytes[from as usize..].to_vec(), Some(if wrong { 0 } else { from }))
            } else {
                (200, bytes.clone(), None)
            };
            if let Some(cut) = self.cut.lock().unwrap().take() {
                body.truncate(cut);
            }
            Ok(Reply { status, location: None, range_start, body: Box::new(Cursor::new(body)) })
        }
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn leaked(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// A small repository whose files are made-up bytes.
    fn repo(contents: &[(&'static str, Vec<u8>)]) -> ModelRepo {
        let files: Vec<ModelFile> = contents
            .iter()
            .map(|(path, bytes)| ModelFile { path, bytes: bytes.len() as u64, sha256: leaked(hex(bytes)) })
            .collect();
        ModelRepo { model: "test/model", repo: "test/model-onnx", revision: "0123456789abcdef", files: files.leak() }
    }

    fn quiet<T>(work: impl FnOnce(&Tracker<'_>) -> T, cancel: &Cancel) -> T {
        let log = |_: &str| {};
        let progress = |_: crate::jobs::Progress| {};
        work(&Tracker::new(&log, &progress, cancel))
    }

    #[test]
    fn the_files_are_downloaded_through_the_redirect_checked_and_then_kept() {
        let big: Vec<u8> = (0..3_000_000_u32).map(|i| (i % 251) as u8).collect();
        let contents = [("tokenizer.json", b"{}".to_vec()), ("onnx/model.onnx", big)];
        let repo = repo(&contents);
        let served = Served::new(contents.iter().map(|(p, b)| ((*p).to_owned(), b.clone())).collect(), true);
        let data = tempfile::tempdir().unwrap();
        let counts = Mutex::new(Vec::new());
        let cancel = Cancel::new();
        let (folder, outcome) =
            quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|c| counts.lock().unwrap().push(c)), &cancel)
                .unwrap();
        assert!(matches!(outcome, StepOutcome::Done(_)));
        assert_eq!(fs::read(folder.join("onnx/model.onnx")).unwrap(), contents[1].1);
        let last = *counts.lock().unwrap().last().unwrap();
        assert_eq!((last.done, last.of), (repo.total_bytes(), repo.total_bytes()));
        // asked of huggingface.co at the pinned revision, then of the content network it redirected to
        let asked = served.asked.lock().unwrap().clone();
        assert!(asked[0].0.ends_with("test/model-onnx/resolve/0123456789abcdef/tokenizer.json"), "{asked:?}");
        assert!(asked[1].0.starts_with("https://cdn.example.org/"));
        let again = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &cancel).unwrap();
        assert!(matches!(again.1, StepOutcome::Kept(_)));
        assert_eq!(served.asked.lock().unwrap().len(), asked.len(), "nothing asked again");
    }

    #[test]
    fn a_download_cut_short_goes_on_from_where_it_stopped() {
        let bytes: Vec<u8> = (0..2_500_000_u32).map(|i| (i % 7) as u8).collect();
        let contents = [("weights.bin", bytes.clone())];
        let repo = repo(&contents);
        let served = Served::new(vec![("weights.bin".into(), bytes.clone())], true);
        *served.cut.lock().unwrap() = Some(1_000_000);
        let data = tempfile::tempdir().unwrap();
        let cancel = Cancel::new();
        let first = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &cancel);
        assert!(matches!(first, Err(ModelError::WrongSize { got: 1_000_000, .. })), "{first:?}");
        let (folder, _) = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &cancel).unwrap();
        assert_eq!(fs::read(folder.join("weights.bin")).unwrap(), bytes);
        let froms: Vec<u64> = served.asked.lock().unwrap().iter().map(|(_, from)| *from).collect();
        assert_eq!(froms, [0, 0, 1_000_000, 1_000_000], "the second download asked from the byte it had");
    }

    #[test]
    fn a_part_that_does_not_start_where_the_file_stopped_is_not_appended() {
        let bytes: Vec<u8> = (0..60_000_u32).map(|i| (i % 13) as u8).collect();
        let repo = repo(&[("weights.bin", bytes.clone())]);
        let served = Served::new(vec![("weights.bin".into(), bytes.clone())], true);
        *served.wrong_range.lock().unwrap() = true;
        let data = tempfile::tempdir().unwrap();
        let part = repo.folder(data.path()).join("weights.bin.part");
        fs::create_dir_all(part.parent().unwrap()).unwrap();
        fs::write(&part, &bytes[..20_000]).unwrap();
        let error = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &Cancel::new()).unwrap_err();
        assert!(error.to_string().contains("not at byte 20000"), "{error}");
        assert!(!part.exists(), "started again from the first byte next time");
        let (folder, _) =
            quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &Cancel::new()).unwrap();
        assert_eq!(fs::read(folder.join("weights.bin")).unwrap(), bytes);
    }

    #[test]
    fn a_server_that_takes_no_ranges_sends_the_whole_file_again() {
        let bytes = vec![9_u8; 50_000];
        let repo = repo(&[("weights.bin", bytes.clone())]);
        let served = Served::new(vec![("weights.bin".into(), bytes.clone())], false);
        let data = tempfile::tempdir().unwrap();
        let place = repo.folder(data.path()).join("weights.bin.part");
        fs::create_dir_all(place.parent().unwrap()).unwrap();
        fs::write(&place, vec![9_u8; 20_000]).unwrap();
        let (folder, _) =
            quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &Cancel::new()).unwrap();
        assert_eq!(fs::read(folder.join("weights.bin")).unwrap(), bytes);
    }

    #[test]
    fn a_file_that_is_not_what_it_should_be_is_removed_and_never_used() {
        let repo = repo(&[("weights.bin", b"the right bytes".to_vec())]);
        let served = Served::new(vec![("weights.bin".into(), b"the wrong bytes".to_vec())], true);
        let data = tempfile::tempdir().unwrap();
        let error = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &Cancel::new()).unwrap_err();
        assert!(matches!(error, ModelError::WrongHash { .. }), "{error}");
        let folder = repo.folder(data.path());
        assert!(!folder.join("weights.bin").exists() && !folder.join("weights.bin.part").exists());
    }

    #[test]
    fn offline_is_said_clearly_and_a_cancel_keeps_what_was_downloaded() {
        struct Offline;
        impl Download for Offline {
            fn get(&self, _: &str, _: u64, _: bool) -> Result<Reply, String> {
                Err("no route to host".to_owned())
            }
        }
        let repo = repo(&[("weights.bin", vec![1; 10])]);
        let data = tempfile::tempdir().unwrap();
        let error =
            quiet(|tracker| ensure(&repo, data.path(), &Offline, tracker, &|_| {}), &Cancel::new()).unwrap_err();
        assert!(error.to_string().contains("offline"), "{error}");
        let bytes = vec![3_u8; 3 * PIECE];
        let repo = self::repo(&[("weights.bin", bytes.clone())]);
        let served = Served::new(vec![("weights.bin".into(), bytes)], true);
        let cancel = Cancel::new();
        cancel.cancel();
        let error = quiet(|tracker| ensure(&repo, data.path(), &served, tracker, &|_| {}), &cancel).unwrap_err();
        assert!(matches!(error, ModelError::Cancelled));
    }

    #[test]
    fn the_pinned_model_is_the_one_the_encoder_loads() {
        let paths: Vec<&str> = QWEN3.files.iter().map(|file| file.path).collect();
        assert_eq!(paths, ["tokenizer.json", "onnx/model_fp16.onnx", "onnx/model_fp16.onnx_data"]);
        assert_eq!(QWEN3.revision.len(), 40);
        assert!(QWEN3.files.iter().all(|file| file.sha256.len() == 64));
        let endpoint = Url::parse("https://huggingface.co").unwrap();
        assert_eq!(
            file_url(&endpoint, &QWEN3, &QWEN3.files[0]),
            "https://huggingface.co/onnx-community/Qwen3-Embedding-0.6B-ONNX/resolve/c25a394dd583836952667c12f008335071b3f43d/tokenizer.json"
        );
    }
}
